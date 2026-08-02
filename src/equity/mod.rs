mod csv_data;
mod simulation;

use crate::decimal::Decimal;
use crate::error::{BotError, Result};
use serde::Deserialize;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::Path;

pub use simulation::EquityResearchReport;

#[derive(Debug, Clone, Deserialize)]
struct EquityResearchFile {
    equity_research: EquityResearchConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EquityResearchConfig {
    pub instrument: Instrument,
    pub initial_cash: Decimal,
    #[serde(default = "default_commission_per_order")]
    pub commission_per_order: Decimal,
    #[serde(default)]
    pub commission_bps: i64,
    #[serde(default = "default_spread_bps")]
    pub spread_bps: i64,
    #[serde(default = "default_slippage_bps")]
    pub slippage_bps: i64,
    #[serde(default)]
    pub fx_bps: i64,
    #[serde(default)]
    pub allow_fractional_shares: bool,
    #[serde(default)]
    pub prices_are_adjusted: bool,
    #[serde(default = "default_annual_trading_days")]
    pub annual_trading_days: usize,
    #[serde(default)]
    pub annual_risk_free_rate_pct: f64,
    #[serde(default = "default_dca_amount")]
    pub monthly_dca_amount: Decimal,
    #[serde(default = "default_ma_fast_window")]
    pub ma_fast_window: usize,
    #[serde(default = "default_ma_slow_window")]
    pub ma_slow_window: usize,
    #[serde(default = "default_breakout_entry_window")]
    pub breakout_entry_window: usize,
    #[serde(default = "default_breakout_exit_window")]
    pub breakout_exit_window: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Instrument {
    pub symbol: String,
    pub asset_class: AssetClass,
    pub exchange: String,
    pub currency: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetClass {
    Equity,
    Etf,
}

impl Display for AssetClass {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Equity => formatter.write_str("equity"),
            Self::Etf => formatter.write_str("ETF"),
        }
    }
}

pub fn run(
    config_path: impl AsRef<Path>,
    csv_path: impl AsRef<Path>,
) -> Result<EquityResearchReport> {
    let config = load_config(config_path)?;
    let bars = csv_data::load(csv_path)?;
    simulation::run(&config, &bars)
}

fn load_config(path: impl AsRef<Path>) -> Result<EquityResearchConfig> {
    let path = path.as_ref();
    let contents = fs::read_to_string(path).map_err(|error| {
        BotError::Config(format!(
            "failed to read equity research config {}: {error}",
            path.to_string_lossy()
        ))
    })?;
    let file: EquityResearchFile = toml::from_str(&contents).map_err(|error| {
        BotError::Config(format!("failed to parse equity research config: {error}"))
    })?;
    file.equity_research.validate()?;
    Ok(file.equity_research)
}

impl EquityResearchConfig {
    fn validate(&self) -> Result<()> {
        for (label, value) in [
            ("instrument symbol", self.instrument.symbol.as_str()),
            ("instrument exchange", self.instrument.exchange.as_str()),
            ("instrument currency", self.instrument.currency.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(BotError::Config(format!("{label} must not be empty")));
            }
        }

        if self.initial_cash <= Decimal::ZERO {
            return Err(BotError::Config(
                "equity research initial cash must be positive".to_string(),
            ));
        }
        if self.commission_per_order < Decimal::ZERO {
            return Err(BotError::Config(
                "commission per order must not be negative".to_string(),
            ));
        }
        if [
            self.commission_bps,
            self.spread_bps,
            self.slippage_bps,
            self.fx_bps,
        ]
        .into_iter()
        .any(|value| value < 0)
        {
            return Err(BotError::Config(
                "equity research costs must not be negative".to_string(),
            ));
        }
        if self.annual_trading_days == 0 {
            return Err(BotError::Config(
                "annual trading days must be positive".to_string(),
            ));
        }
        if !self.annual_risk_free_rate_pct.is_finite() || self.annual_risk_free_rate_pct <= -100.0 {
            return Err(BotError::Config(
                "annual risk-free rate must be finite and greater than -100%".to_string(),
            ));
        }
        if self.monthly_dca_amount <= Decimal::ZERO {
            return Err(BotError::Config(
                "monthly DCA amount must be positive".to_string(),
            ));
        }
        if self.ma_fast_window == 0
            || self.ma_slow_window == 0
            || self.ma_fast_window >= self.ma_slow_window
        {
            return Err(BotError::Config(
                "equity MA windows must satisfy 0 < fast < slow".to_string(),
            ));
        }
        if self.breakout_entry_window == 0 || self.breakout_exit_window == 0 {
            return Err(BotError::Config(
                "breakout entry and exit windows must be positive".to_string(),
            ));
        }
        Ok(())
    }
}

fn default_commission_per_order() -> Decimal {
    Decimal::from_micro_units(1_000_000)
}

fn default_spread_bps() -> i64 {
    10
}

fn default_slippage_bps() -> i64 {
    5
}

fn default_annual_trading_days() -> usize {
    252
}

fn default_dca_amount() -> Decimal {
    Decimal::from_micro_units(500_000_000)
}

fn default_ma_fast_window() -> usize {
    50
}

fn default_ma_slow_window() -> usize {
    200
}

fn default_breakout_entry_window() -> usize {
    55
}

fn default_breakout_exit_window() -> usize {
    20
}

#[cfg(test)]
mod tests {
    use super::load_config;
    use std::fs;

    #[test]
    fn loads_standalone_equity_config() {
        let path = std::env::temp_dir().join("trader-equity-config.toml");
        fs::write(
            &path,
            r#"
[equity_research]
initial_cash = 10000
commission_per_order = 1
ma_fast_window = 20
ma_slow_window = 50

[equity_research.instrument]
symbol = "VWRP.L"
asset_class = "etf"
exchange = "LSE"
currency = "GBP"
"#,
        )
        .expect("config should write");

        let config = load_config(&path).expect("config should load");

        assert_eq!(config.instrument.symbol, "VWRP.L");
        assert_eq!(config.initial_cash.to_string(), "10000");
        assert_eq!(config.ma_slow_window, 50);
        fs::remove_file(path).expect("config should remove");
    }
}
