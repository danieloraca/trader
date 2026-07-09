use crate::error::{BotError, Result};
use serde::Deserialize;
use std::fs;
use std::path::Path;

const DEFAULT_CONFIG_PATH: &str = "config/trader.toml";

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub bot: BotConfig,
    pub market_data: MarketDataConfig,
    pub risk: RiskConfig,
    pub telemetry: TelemetryConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BotConfig {
    pub symbol: String,
    pub quote_currency: String,
    pub base_currency: String,
    pub paper_starting_quote_balance: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MarketDataConfig {
    pub replay_prices: Vec<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RiskConfig {
    pub max_order_quote_value: f64,
    pub max_position_base: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelemetryConfig {
    pub verbose: bool,
}

impl Config {
    pub fn load() -> Result<Self> {
        Self::load_from_path(DEFAULT_CONFIG_PATH)
    }

    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let contents = fs::read_to_string(path).map_err(|error| {
            BotError::Config(format!(
                "failed to read {}: {error}",
                path.to_string_lossy()
            ))
        })?;

        Self::from_toml_str(&contents)
    }

    fn from_toml_str(contents: &str) -> Result<Self> {
        let config: Self = toml::from_str(contents)
            .map_err(|error| BotError::Config(format!("failed to parse config: {error}")))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.bot.symbol.trim().is_empty() {
            return Err(BotError::Config("symbol must not be empty".to_string()));
        }

        if self.bot.base_currency.trim().is_empty() {
            return Err(BotError::Config(
                "base currency must not be empty".to_string(),
            ));
        }

        if self.bot.quote_currency.trim().is_empty() {
            return Err(BotError::Config(
                "quote currency must not be empty".to_string(),
            ));
        }

        if !self.bot.paper_starting_quote_balance.is_finite()
            || self.bot.paper_starting_quote_balance <= 0.0
        {
            return Err(BotError::Config(
                "paper starting quote balance must be positive".to_string(),
            ));
        }

        if self.market_data.replay_prices.is_empty() {
            return Err(BotError::Config(
                "market data replay prices must not be empty".to_string(),
            ));
        }

        if self
            .market_data
            .replay_prices
            .iter()
            .any(|price| !price.is_finite() || *price <= 0.0)
        {
            return Err(BotError::Config(
                "market data replay prices must be positive finite values".to_string(),
            ));
        }

        if !self.risk.max_order_quote_value.is_finite() || self.risk.max_order_quote_value <= 0.0 {
            return Err(BotError::Config(
                "max order quote value must be positive".to_string(),
            ));
        }

        if !self.risk.max_position_base.is_finite() || self.risk.max_position_base <= 0.0 {
            return Err(BotError::Config(
                "max position base must be positive".to_string(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Config;

    const VALID_CONFIG: &str = r#"
[bot]
symbol = "BTC-USD"
base_currency = "BTC"
quote_currency = "USD"
paper_starting_quote_balance = 10000.0

[market_data]
replay_prices = [100.0, 101.0, 102.0, 101.5, 99.0]

[risk]
max_order_quote_value = 500.0
max_position_base = 0.25

[telemetry]
verbose = true
"#;

    #[test]
    fn parses_valid_config() {
        let config = Config::from_toml_str(VALID_CONFIG).expect("config should parse");

        assert_eq!(config.bot.symbol, "BTC-USD");
        assert_eq!(config.bot.base_currency, "BTC");
        assert_eq!(config.bot.quote_currency, "USD");
        assert_eq!(config.bot.paper_starting_quote_balance, 10_000.0);
        assert_eq!(
            config.market_data.replay_prices,
            vec![100.0, 101.0, 102.0, 101.5, 99.0]
        );
        assert_eq!(config.risk.max_order_quote_value, 500.0);
        assert_eq!(config.risk.max_position_base, 0.25);
        assert!(config.telemetry.verbose);
    }

    #[test]
    fn rejects_empty_symbol() {
        let invalid_config = VALID_CONFIG.replace("BTC-USD", "");
        let error = Config::from_toml_str(&invalid_config).expect_err("config should fail");

        assert!(error.to_string().contains("symbol must not be empty"));
    }

    #[test]
    fn rejects_non_positive_order_limit() {
        let invalid_config = VALID_CONFIG.replace(
            "max_order_quote_value = 500.0",
            "max_order_quote_value = 0.0",
        );
        let error = Config::from_toml_str(&invalid_config).expect_err("config should fail");

        assert!(
            error
                .to_string()
                .contains("max order quote value must be positive")
        );
    }

    #[test]
    fn rejects_empty_replay_prices() {
        let invalid_config = VALID_CONFIG.replace(
            "replay_prices = [100.0, 101.0, 102.0, 101.5, 99.0]",
            "replay_prices = []",
        );
        let error = Config::from_toml_str(&invalid_config).expect_err("config should fail");

        assert!(
            error
                .to_string()
                .contains("replay prices must not be empty")
        );
    }

    #[test]
    fn rejects_non_positive_replay_price() {
        let invalid_config = VALID_CONFIG.replace(
            "replay_prices = [100.0, 101.0, 102.0, 101.5, 99.0]",
            "replay_prices = [100.0, 0.0]",
        );
        let error = Config::from_toml_str(&invalid_config).expect_err("config should fail");

        assert!(
            error
                .to_string()
                .contains("replay prices must be positive finite values")
        );
    }
}
