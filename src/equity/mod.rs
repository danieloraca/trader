mod portfolio;
mod price_data;
mod simulation;
mod walk_forward;

use crate::decimal::Decimal;
use crate::error::{BotError, Result};
use serde::Deserialize;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::Path;

pub use portfolio::EquityPortfolioReport;
pub use simulation::EquityResearchReport;
pub use walk_forward::EquityWalkForwardReport;

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
    #[serde(default)]
    pub walk_forward: EquityWalkForwardConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EquityWalkForwardConfig {
    #[serde(default = "default_walk_forward_train_sessions")]
    pub train_sessions: usize,
    #[serde(default = "default_walk_forward_test_sessions")]
    pub test_sessions: usize,
    #[serde(default = "default_walk_forward_step_sessions")]
    pub step_sessions: usize,
    #[serde(default = "default_walk_forward_minimum_test_trades")]
    pub minimum_test_trades_per_window: usize,
    #[serde(default = "default_walk_forward_ma_fast_windows")]
    pub ma_fast_windows: Vec<usize>,
    #[serde(default = "default_walk_forward_ma_slow_windows")]
    pub ma_slow_windows: Vec<usize>,
    #[serde(default = "default_walk_forward_breakout_entry_windows")]
    pub breakout_entry_windows: Vec<usize>,
    #[serde(default = "default_walk_forward_breakout_exit_windows")]
    pub breakout_exit_windows: Vec<usize>,
}

impl Default for EquityWalkForwardConfig {
    fn default() -> Self {
        Self {
            train_sessions: default_walk_forward_train_sessions(),
            test_sessions: default_walk_forward_test_sessions(),
            step_sessions: default_walk_forward_step_sessions(),
            minimum_test_trades_per_window: default_walk_forward_minimum_test_trades(),
            ma_fast_windows: default_walk_forward_ma_fast_windows(),
            ma_slow_windows: default_walk_forward_ma_slow_windows(),
            breakout_entry_windows: default_walk_forward_breakout_entry_windows(),
            breakout_exit_windows: default_walk_forward_breakout_exit_windows(),
        }
    }
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
    price_path: impl AsRef<Path>,
) -> Result<EquityResearchReport> {
    let config = load_config(config_path)?;
    let data = price_data::load(price_path)?;
    simulation::run(&config, &data)
}

pub fn run_walk_forward(
    config_path: impl AsRef<Path>,
    price_path: impl AsRef<Path>,
) -> Result<EquityWalkForwardReport> {
    let config = load_config(config_path)?;
    let data = price_data::load(price_path)?;
    walk_forward::run(&config, &data)
}

pub fn run_portfolio(config_path: impl AsRef<Path>) -> Result<EquityPortfolioReport> {
    portfolio::run(config_path)
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
        self.walk_forward.validate()?;
        Ok(())
    }
}

impl EquityWalkForwardConfig {
    fn validate(&self) -> Result<()> {
        if self.train_sessions == 0 || self.test_sessions == 0 || self.step_sessions == 0 {
            return Err(BotError::Config(
                "equity walk-forward train, test, and step sessions must be positive".to_string(),
            ));
        }
        if self.step_sessions < self.test_sessions {
            return Err(BotError::Config(
                "equity walk-forward step sessions must be at least the test sessions to keep held-out windows non-overlapping"
                    .to_string(),
            ));
        }
        if self.minimum_test_trades_per_window == 0 {
            return Err(BotError::Config(
                "equity walk-forward minimum test trades must be positive".to_string(),
            ));
        }
        for (name, values) in [
            ("MA fast", self.ma_fast_windows.as_slice()),
            ("MA slow", self.ma_slow_windows.as_slice()),
            ("breakout entry", self.breakout_entry_windows.as_slice()),
            ("breakout exit", self.breakout_exit_windows.as_slice()),
        ] {
            if values.is_empty() || values.contains(&0) {
                return Err(BotError::Config(format!(
                    "equity walk-forward {name} windows must be non-empty and positive"
                )));
            }
        }
        if !self
            .ma_fast_windows
            .iter()
            .any(|fast| self.ma_slow_windows.iter().any(|slow| fast < slow))
        {
            return Err(BotError::Config(
                "equity walk-forward needs at least one MA fast window below a slow window"
                    .to_string(),
            ));
        }
        if !self
            .breakout_entry_windows
            .iter()
            .any(|entry| self.breakout_exit_windows.iter().any(|exit| exit < entry))
        {
            return Err(BotError::Config(
                "equity walk-forward needs at least one breakout exit window below an entry window"
                    .to_string(),
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

fn default_walk_forward_train_sessions() -> usize {
    756
}

fn default_walk_forward_test_sessions() -> usize {
    252
}

fn default_walk_forward_step_sessions() -> usize {
    252
}

fn default_walk_forward_minimum_test_trades() -> usize {
    1
}

fn default_walk_forward_ma_fast_windows() -> Vec<usize> {
    vec![20, 50, 100]
}

fn default_walk_forward_ma_slow_windows() -> Vec<usize> {
    vec![100, 150, 200, 250]
}

fn default_walk_forward_breakout_entry_windows() -> Vec<usize> {
    vec![20, 55, 100]
}

fn default_walk_forward_breakout_exit_windows() -> Vec<usize> {
    vec![10, 20, 50]
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
        assert_eq!(config.walk_forward.train_sessions, 756);
        assert_eq!(config.walk_forward.test_sessions, 252);
        fs::remove_file(path).expect("config should remove");
    }
}
