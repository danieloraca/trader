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
    #[serde(default)]
    pub backtest: BacktestConfig,
    #[serde(default)]
    pub exchange: ExchangeConfig,
    pub market_data: MarketDataConfig,
    pub risk: RiskConfig,
    #[serde(default)]
    pub strategy: StrategyConfig,
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
pub struct BacktestConfig {
    #[serde(default = "default_backtest_fee_bps")]
    pub fee_bps: i64,
    #[serde(default = "default_backtest_slippage_bps")]
    pub slippage_bps: i64,
    #[serde(default)]
    pub trade_log_csv_path: Option<String>,
}

impl Default for BacktestConfig {
    fn default() -> Self {
        Self {
            fee_bps: default_backtest_fee_bps(),
            slippage_bps: default_backtest_slippage_bps(),
            trade_log_csv_path: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExchangeConfig {
    #[serde(default)]
    pub kind: ExchangeKind,
    #[serde(default)]
    pub kraken: KrakenConfig,
}

impl Default for ExchangeConfig {
    fn default() -> Self {
        Self {
            kind: ExchangeKind::Paper,
            kraken: KrakenConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExchangeKind {
    Paper,
    Kraken,
}

impl Default for ExchangeKind {
    fn default() -> Self {
        Self::Paper
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct KrakenConfig {
    #[serde(default = "default_kraken_base_url")]
    pub base_url: String,
    #[serde(default = "default_kraken_pair")]
    pub pair: String,
    #[serde(default = "default_kraken_api_key_env")]
    pub api_key_env: String,
    #[serde(default = "default_kraken_api_secret_env")]
    pub api_secret_env: String,
    #[serde(default)]
    pub enable_order_placement: bool,
}

impl Default for KrakenConfig {
    fn default() -> Self {
        Self {
            base_url: default_kraken_base_url(),
            pair: default_kraken_pair(),
            api_key_env: default_kraken_api_key_env(),
            api_secret_env: default_kraken_api_secret_env(),
            enable_order_placement: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MarketDataConfig {
    #[serde(default)]
    pub kind: MarketDataKind,
    #[serde(default)]
    pub replay_prices: Vec<Decimal>,
    #[serde(default = "default_market_data_idle_sleep_ms")]
    pub idle_sleep_ms: u64,
    #[serde(default)]
    pub kraken: KrakenMarketDataConfig,
}

impl Default for MarketDataConfig {
    fn default() -> Self {
        Self {
            kind: MarketDataKind::Replay,
            replay_prices: Vec::new(),
            idle_sleep_ms: default_market_data_idle_sleep_ms(),
            kraken: KrakenMarketDataConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MarketDataKind {
    Replay,
    KrakenTicker,
}

impl Default for MarketDataKind {
    fn default() -> Self {
        Self::Replay
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct KrakenMarketDataConfig {
    #[serde(default = "default_kraken_base_url")]
    pub base_url: String,
    #[serde(default = "default_kraken_pair")]
    pub pair: String,
    #[serde(default = "default_kraken_ticker_poll_interval_ms")]
    pub poll_interval_ms: u64,
}

impl Default for KrakenMarketDataConfig {
    fn default() -> Self {
        Self {
            base_url: default_kraken_base_url(),
            pair: default_kraken_pair(),
            poll_interval_ms: default_kraken_ticker_poll_interval_ms(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RiskConfig {
    pub max_order_quote_value: Decimal,
    pub max_position_base: Decimal,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StrategyConfig {
    #[serde(default)]
    pub kind: StrategyKind,
    #[serde(default)]
    pub simple_momentum: SimpleMomentumConfig,
}

impl Default for StrategyConfig {
    fn default() -> Self {
        Self {
            kind: StrategyKind::SimpleMomentum,
            simple_momentum: SimpleMomentumConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StrategyKind {
    SimpleMomentum,
}

impl Default for StrategyKind {
    fn default() -> Self {
        Self::SimpleMomentum
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SimpleMomentumConfig {
    #[serde(default = "default_simple_momentum_buy_threshold_bps")]
    pub buy_threshold_bps: i64,
    #[serde(default = "default_simple_momentum_sell_threshold_bps")]
    pub sell_threshold_bps: i64,
    #[serde(default = "default_simple_momentum_buy_quantity_base")]
    pub buy_quantity_base: Decimal,
    #[serde(default = "default_simple_momentum_sell_quantity_base")]
    pub sell_quantity_base: Decimal,
}

impl Default for SimpleMomentumConfig {
    fn default() -> Self {
        Self {
            buy_threshold_bps: default_simple_momentum_buy_threshold_bps(),
            sell_threshold_bps: default_simple_momentum_sell_threshold_bps(),
            buy_quantity_base: default_simple_momentum_buy_quantity_base(),
            sell_quantity_base: default_simple_momentum_sell_quantity_base(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    pub sqlite_path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelemetryConfig {
    pub verbose: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeCommand {
    Run,
    Backtest,
    BacktestSqlite,
    SweepSqlite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeOptions {
    pub config_path: String,
    pub command: RuntimeCommand,
    pub backtest_sqlite_path: Option<String>,
    pub sweep_sqlite_path: Option<String>,
}

impl Config {
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

        if self.backtest.fee_bps < 0 {
            return Err(BotError::Config(
                "backtest fee bps must not be negative".to_string(),
            ));
        }

        if self.backtest.slippage_bps < 0 {
            return Err(BotError::Config(
                "backtest slippage bps must not be negative".to_string(),
            ));
        }

        if self.exchange.kind == ExchangeKind::Kraken {
            if self.exchange.kraken.base_url.trim().is_empty() {
                return Err(BotError::Config(
                    "kraken base url must not be empty".to_string(),
                ));
            }

            if self.exchange.kraken.pair.trim().is_empty() {
                return Err(BotError::Config(
                    "kraken pair must not be empty".to_string(),
                ));
            }

            if self.exchange.kraken.api_key_env.trim().is_empty() {
                return Err(BotError::Config(
                    "kraken api key env var name must not be empty".to_string(),
                ));
            }

            if self.exchange.kraken.api_secret_env.trim().is_empty() {
                return Err(BotError::Config(
                    "kraken api secret env var name must not be empty".to_string(),
                ));
            }
        }

        if self.market_data.kind == MarketDataKind::Replay
            && self.market_data.replay_prices.is_empty()
        {
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

        if self.market_data.kind == MarketDataKind::KrakenTicker {
            if self.market_data.kraken.base_url.trim().is_empty() {
                return Err(BotError::Config(
                    "kraken market data base url must not be empty".to_string(),
                ));
            }

            if self.market_data.kraken.pair.trim().is_empty() {
                return Err(BotError::Config(
                    "kraken market data pair must not be empty".to_string(),
                ));
            }

            if self.market_data.kraken.poll_interval_ms == 0 {
                return Err(BotError::Config(
                    "kraken market data poll interval must be positive".to_string(),
                ));
            }
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

        if self.strategy.simple_momentum.buy_threshold_bps <= 0 {
            return Err(BotError::Config(
                "simple momentum buy threshold must be positive".to_string(),
            ));
        }

        if self.strategy.simple_momentum.sell_threshold_bps >= 0 {
            return Err(BotError::Config(
                "simple momentum sell threshold must be negative".to_string(),
            ));
        }

        if self.strategy.simple_momentum.buy_quantity_base <= Decimal::ZERO {
            return Err(BotError::Config(
                "simple momentum buy quantity must be positive".to_string(),
            ));
        }

        if self.strategy.simple_momentum.sell_quantity_base <= Decimal::ZERO {
            return Err(BotError::Config(
                "simple momentum sell quantity must be positive".to_string(),
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

impl RuntimeOptions {
    pub fn from_runtime() -> Result<Self> {
        Self::from_args_and_env(env::args(), env::var(CONFIG_ENV_VAR).ok())
    }

    fn from_args_and_env(
        args: impl IntoIterator<Item = String>,
        env_config_path: Option<String>,
    ) -> Result<Self> {
        let mut args = args.into_iter();
        let _program = args.next();
        let mut config_path = env_config_path.unwrap_or_else(|| DEFAULT_CONFIG_PATH.to_string());
        let mut command = RuntimeCommand::Run;
        let mut backtest_sqlite_path = None;
        let mut sweep_sqlite_path = None;

        while let Some(arg) = args.next() {
            if arg == "--config" {
                let Some(path) = args.next() else {
                    return Err(BotError::Config("--config requires a path".to_string()));
                };
                config_path = path;
            } else if arg == "--backtest" {
                command = RuntimeCommand::Backtest;
            } else if arg == "--backtest-sqlite" {
                let Some(path) = args.next() else {
                    return Err(BotError::Config(
                        "--backtest-sqlite requires a sqlite path".to_string(),
                    ));
                };
                command = RuntimeCommand::BacktestSqlite;
                backtest_sqlite_path = Some(path);
            } else if arg == "--sweep-sqlite" {
                let Some(path) = args.next() else {
                    return Err(BotError::Config(
                        "--sweep-sqlite requires a sqlite path".to_string(),
                    ));
                };
                command = RuntimeCommand::SweepSqlite;
                sweep_sqlite_path = Some(path);
            } else {
                return Err(BotError::Config(format!("unknown argument: {arg}")));
            }
        }

        Ok(Self {
            config_path,
            command,
            backtest_sqlite_path,
            sweep_sqlite_path,
        })
    }
}

fn default_kraken_base_url() -> String {
    "https://api.kraken.com".to_string()
}

fn default_backtest_fee_bps() -> i64 {
    26
}

fn default_backtest_slippage_bps() -> i64 {
    5
}

fn default_kraken_pair() -> String {
    "XBTUSD".to_string()
}

fn default_kraken_api_key_env() -> String {
    "KRAKEN_API_KEY".to_string()
}

fn default_kraken_api_secret_env() -> String {
    "KRAKEN_API_SECRET".to_string()
}

fn default_kraken_ticker_poll_interval_ms() -> u64 {
    5_000
}

fn default_market_data_idle_sleep_ms() -> u64 {
    1_000
}

fn default_simple_momentum_buy_threshold_bps() -> i64 {
    50
}

fn default_simple_momentum_sell_threshold_bps() -> i64 {
    -100
}

fn default_simple_momentum_buy_quantity_base() -> Decimal {
    Decimal::from_micro_units(10_000)
}

fn default_simple_momentum_sell_quantity_base() -> Decimal {
    Decimal::from_micro_units(5_000)
}

#[cfg(test)]
fn config_path_from_args_and_env(
    args: impl IntoIterator<Item = String>,
    env_config_path: Option<String>,
) -> Result<String> {
    RuntimeOptions::from_args_and_env(args, env_config_path).map(|options| options.config_path)
}

#[cfg(test)]
mod tests {
    use super::{Config, RuntimeCommand, RuntimeOptions, config_path_from_args_and_env};

    const VALID_CONFIG: &str = r#"
[bot]
symbol = "BTC-USD"
base_currency = "BTC"
quote_currency = "USD"
paper_starting_quote_balance = 10000.0

[backtest]
fee_bps = 26
slippage_bps = 5
trade_log_csv_path = "data/backtest-trades.csv"

[exchange]
kind = "paper"

[exchange.kraken]
base_url = "https://api.kraken.com"
pair = "XBTUSD"
api_key_env = "KRAKEN_API_KEY"
api_secret_env = "KRAKEN_API_SECRET"
enable_order_placement = false

[market_data]
kind = "replay"
replay_prices = [100.0, 101.0, 102.0, 101.5, 99.0]
idle_sleep_ms = 1000

[market_data.kraken]
base_url = "https://api.kraken.com"
pair = "XBTUSD"
poll_interval_ms = 5000

[risk]
max_order_quote_value = 500.0
max_position_base = 0.25

[strategy]
kind = "simple_momentum"

[strategy.simple_momentum]
buy_threshold_bps = 50
sell_threshold_bps = -100
buy_quantity_base = 0.01
sell_quantity_base = 0.005

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
        assert_eq!(config.backtest.fee_bps, 26);
        assert_eq!(config.backtest.slippage_bps, 5);
        assert_eq!(
            config.backtest.trade_log_csv_path.as_deref(),
            Some("data/backtest-trades.csv")
        );
        assert_eq!(config.exchange.kind, super::ExchangeKind::Paper);
        assert_eq!(config.exchange.kraken.pair, "XBTUSD");
        assert!(!config.exchange.kraken.enable_order_placement);
        assert_eq!(config.market_data.kind, super::MarketDataKind::Replay);
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
        assert_eq!(config.market_data.kraken.pair, "XBTUSD");
        assert_eq!(config.market_data.kraken.poll_interval_ms, 5_000);
        assert_eq!(config.risk.max_order_quote_value.to_string(), "500");
        assert_eq!(config.risk.max_position_base.to_string(), "0.25");
        assert_eq!(config.strategy.kind, super::StrategyKind::SimpleMomentum);
        assert_eq!(config.strategy.simple_momentum.buy_threshold_bps, 50);
        assert_eq!(config.strategy.simple_momentum.sell_threshold_bps, -100);
        assert_eq!(
            config
                .strategy
                .simple_momentum
                .buy_quantity_base
                .to_string(),
            "0.01"
        );
        assert_eq!(
            config
                .strategy
                .simple_momentum
                .sell_quantity_base
                .to_string(),
            "0.005"
        );
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
    fn allows_empty_replay_prices_for_kraken_ticker_market_data() {
        let valid_config = VALID_CONFIG
            .replace("kind = \"replay\"", "kind = \"kraken_ticker\"")
            .replace(
                "replay_prices = [100.0, 101.0, 102.0, 101.5, 99.0]",
                "replay_prices = []",
            );
        let config = Config::from_toml_str(&valid_config).expect("config should parse");

        assert_eq!(config.market_data.kind, super::MarketDataKind::KrakenTicker);
        assert!(config.market_data.replay_prices.is_empty());
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
    fn accepts_backtest_sqlite_argument() {
        let options = RuntimeOptions::from_args_and_env(
            [
                "trader".to_string(),
                "--config".to_string(),
                "/etc/trader.toml".to_string(),
                "--backtest-sqlite".to_string(),
                "/var/lib/trader/trader.sqlite".to_string(),
            ],
            None,
        )
        .expect("runtime options should parse");

        assert_eq!(options.config_path, "/etc/trader.toml");
        assert_eq!(options.command, RuntimeCommand::BacktestSqlite);
        assert_eq!(
            options.backtest_sqlite_path.as_deref(),
            Some("/var/lib/trader/trader.sqlite")
        );
    }

    #[test]
    fn accepts_sweep_sqlite_argument() {
        let options = RuntimeOptions::from_args_and_env(
            [
                "trader".to_string(),
                "--config".to_string(),
                "/etc/trader.toml".to_string(),
                "--sweep-sqlite".to_string(),
                "/var/lib/trader/trader.sqlite".to_string(),
            ],
            None,
        )
        .expect("runtime options should parse");

        assert_eq!(options.config_path, "/etc/trader.toml");
        assert_eq!(options.command, RuntimeCommand::SweepSqlite);
        assert_eq!(
            options.sweep_sqlite_path.as_deref(),
            Some("/var/lib/trader/trader.sqlite")
        );
    }

    #[test]
    fn rejects_missing_config_path_argument_value() {
        let error =
            config_path_from_args_and_env(["trader".to_string(), "--config".to_string()], None)
                .expect_err("path should fail");

        assert!(error.to_string().contains("--config requires a path"));
    }
}
