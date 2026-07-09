use crate::error::{BotError, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub bot: BotConfig,
    pub risk: RiskConfig,
    pub telemetry: TelemetryConfig,
}

#[derive(Debug, Clone)]
pub struct BotConfig {
    pub symbol: String,
    pub quote_currency: String,
    pub base_currency: String,
    pub paper_starting_quote_balance: f64,
}

#[derive(Debug, Clone)]
pub struct RiskConfig {
    pub max_order_quote_value: f64,
    pub max_position_base: f64,
}

#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    pub verbose: bool,
}

impl Config {
    pub fn load() -> Result<Self> {
        let config = Self {
            bot: BotConfig {
                symbol: "BTC-USD".to_string(),
                quote_currency: "USD".to_string(),
                base_currency: "BTC".to_string(),
                paper_starting_quote_balance: 10_000.0,
            },
            risk: RiskConfig {
                max_order_quote_value: 500.0,
                max_position_base: 0.25,
            },
            telemetry: TelemetryConfig { verbose: true },
        };

        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.bot.symbol.trim().is_empty() {
            return Err(BotError::Config("symbol must not be empty".to_string()));
        }

        if self.bot.paper_starting_quote_balance <= 0.0 {
            return Err(BotError::Config(
                "paper starting quote balance must be positive".to_string(),
            ));
        }

        if self.risk.max_order_quote_value <= 0.0 {
            return Err(BotError::Config(
                "max order quote value must be positive".to_string(),
            ));
        }

        if self.risk.max_position_base <= 0.0 {
            return Err(BotError::Config(
                "max position base must be positive".to_string(),
            ));
        }

        Ok(())
    }
}
