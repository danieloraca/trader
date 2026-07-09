use crate::decimal::Decimal;
use crate::error::{BotError, Result};
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::Path;

const DEFAULT_CONFIG_PATH: &str = "config/trader.toml";
const CONFIG_ENV_VAR: &str = "TRADER_CONFIG";

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub bot: BotConfig,
    pub market_data: MarketDataConfig,
    pub risk: RiskConfig,
    pub storage: StorageConfig,
    pub telemetry: TelemetryConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BotConfig {
    pub symbol: String,
    pub quote_currency: String,
    pub base_currency: String,
    pub paper_starting_quote_balance: Decimal,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MarketDataConfig {
    pub replay_prices: Vec<Decimal>,
    pub idle_sleep_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RiskConfig {
    pub max_order_quote_value: Decimal,
    pub max_position_base: Decimal,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    pub sqlite_path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelemetryConfig {
    pub verbose: bool,
}

impl Config {
    pub fn load_from_runtime() -> Result<Self> {
        let path = config_path_from_args_and_env(env::args(), env::var(CONFIG_ENV_VAR).ok())?;
        Self::load_from_path(path)
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

        if self.bot.paper_starting_quote_balance <= Decimal::ZERO {
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
            .any(|price| *price <= Decimal::ZERO)
        {
            return Err(BotError::Config(
                "market data replay prices must be positive finite values".to_string(),
            ));
        }

        if self.market_data.idle_sleep_ms == 0 {
            return Err(BotError::Config(
                "market data idle sleep must be positive".to_string(),
            ));
        }

        if self.risk.max_order_quote_value <= Decimal::ZERO {
            return Err(BotError::Config(
                "max order quote value must be positive".to_string(),
            ));
        }

        if self.risk.max_position_base <= Decimal::ZERO {
            return Err(BotError::Config(
                "max position base must be positive".to_string(),
            ));
        }

        if self.storage.sqlite_path.trim().is_empty() {
            return Err(BotError::Config(
                "sqlite path must not be empty".to_string(),
            ));
        }

        Ok(())
    }
}

fn config_path_from_args_and_env(
    args: impl IntoIterator<Item = String>,
    env_config_path: Option<String>,
) -> Result<String> {
    let mut args = args.into_iter();
    let _program = args.next();
    let mut config_path = env_config_path.unwrap_or_else(|| DEFAULT_CONFIG_PATH.to_string());

    while let Some(arg) = args.next() {
        if arg == "--config" {
            let Some(path) = args.next() else {
                return Err(BotError::Config("--config requires a path".to_string()));
            };
            config_path = path;
        } else {
            return Err(BotError::Config(format!("unknown argument: {arg}")));
        }
    }

    Ok(config_path)
}

#[cfg(test)]
mod tests {
    use super::{Config, config_path_from_args_and_env};

    const VALID_CONFIG: &str = r#"
[bot]
symbol = "BTC-USD"
base_currency = "BTC"
quote_currency = "USD"
paper_starting_quote_balance = 10000.0

[market_data]
replay_prices = [100.0, 101.0, 102.0, 101.5, 99.0]
idle_sleep_ms = 1000

[risk]
max_order_quote_value = 500.0
max_position_base = 0.25

[storage]
sqlite_path = "data/trader.sqlite"

[telemetry]
verbose = true
"#;

    #[test]
    fn parses_valid_config() {
        let config = Config::from_toml_str(VALID_CONFIG).expect("config should parse");

        assert_eq!(config.bot.symbol, "BTC-USD");
        assert_eq!(config.bot.base_currency, "BTC");
        assert_eq!(config.bot.quote_currency, "USD");
        assert_eq!(config.bot.paper_starting_quote_balance.to_string(), "10000");
        assert_eq!(
            config
                .market_data
                .replay_prices
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            vec!["100", "101", "102", "101.5", "99"]
        );
        assert_eq!(config.market_data.idle_sleep_ms, 1_000);
        assert_eq!(config.risk.max_order_quote_value.to_string(), "500");
        assert_eq!(config.risk.max_position_base.to_string(), "0.25");
        assert_eq!(config.storage.sqlite_path, "data/trader.sqlite");
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

    #[test]
    fn rejects_zero_market_data_idle_sleep() {
        let invalid_config = VALID_CONFIG.replace("idle_sleep_ms = 1000", "idle_sleep_ms = 0");
        let error = Config::from_toml_str(&invalid_config).expect_err("config should fail");

        assert!(
            error
                .to_string()
                .contains("market data idle sleep must be positive")
        );
    }

    #[test]
    fn rejects_empty_sqlite_path() {
        let invalid_config =
            VALID_CONFIG.replace("sqlite_path = \"data/trader.sqlite\"", "sqlite_path = \"\"");
        let error = Config::from_toml_str(&invalid_config).expect_err("config should fail");

        assert!(error.to_string().contains("sqlite path must not be empty"));
    }

    #[test]
    fn uses_default_config_path_when_no_runtime_override_is_present() {
        let path = config_path_from_args_and_env(["trader".to_string()], None)
            .expect("path should resolve");

        assert_eq!(path, "config/trader.toml");
    }

    #[test]
    fn accepts_config_path_from_env() {
        let path = config_path_from_args_and_env(
            ["trader".to_string()],
            Some("/etc/trader-env.toml".to_string()),
        )
        .expect("path should resolve");

        assert_eq!(path, "/etc/trader-env.toml");
    }

    #[test]
    fn accepts_config_path_argument() {
        let path = config_path_from_args_and_env(
            [
                "trader".to_string(),
                "--config".to_string(),
                "/etc/trader.toml".to_string(),
            ],
            Some("/etc/trader-env.toml".to_string()),
        )
        .expect("path should resolve");

        assert_eq!(path, "/etc/trader.toml");
    }

    #[test]
    fn rejects_missing_config_path_argument_value() {
        let error =
            config_path_from_args_and_env(["trader".to_string(), "--config".to_string()], None)
                .expect_err("path should fail");

        assert!(error.to_string().contains("--config requires a path"));
    }
}
