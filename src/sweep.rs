use crate::backtest::{self, BacktestReport};
use crate::config::Config;
use crate::decimal::Decimal;
use crate::error::Result;
use std::fmt::{Display, Formatter};

const BUY_THRESHOLDS_BPS: [i64; 6] = [3, 5, 8, 10, 15, 20];
const SELL_THRESHOLDS_BPS: [i64; 7] = [-3, -5, -8, -10, -15, -20, -30];
const QUANTITY_MICRO_UNITS: [i64; 4] = [500, 1_000, 2_000, 5_000];

#[derive(Debug, Clone)]
pub struct SweepReport {
    pub sqlite_path: String,
    pub result_count: usize,
    pub results: Vec<SweepResult>,
}

#[derive(Debug, Clone)]
pub struct SweepResult {
    pub buy_threshold_bps: i64,
    pub sell_threshold_bps: i64,
    pub quantity_base: Decimal,
    pub net_profit_loss_quote: Decimal,
    pub return_pct: f64,
    pub buy_and_hold_delta_quote: Decimal,
    pub max_drawdown_pct: f64,
    pub filled_order_count: usize,
    pub rejected_order_count: usize,
    pub buy_count: usize,
    pub sell_count: usize,
    pub exposure_pct: f64,
    pub final_base_balance: Decimal,
}

pub fn run(config: &Config, sqlite_path: &str) -> Result<SweepReport> {
    let prices = backtest::load_prices_from_sqlite(sqlite_path, &config.bot.symbol)?;
    let mut results = Vec::new();

    for buy_threshold_bps in BUY_THRESHOLDS_BPS {
        for sell_threshold_bps in SELL_THRESHOLDS_BPS {
            for quantity_micro_units in QUANTITY_MICRO_UNITS {
                let mut candidate = config.clone();
                candidate.strategy.simple_momentum.buy_threshold_bps = buy_threshold_bps;
                candidate.strategy.simple_momentum.sell_threshold_bps = sell_threshold_bps;
                candidate.strategy.simple_momentum.buy_quantity_base =
                    Decimal::from_micro_units(quantity_micro_units);
                candidate.strategy.simple_momentum.sell_quantity_base =
                    Decimal::from_micro_units(quantity_micro_units);
                candidate.backtest.trade_log_csv_path = None;

                let report = backtest::run_from_prices(&candidate, prices.clone())?;
                results.push(SweepResult::from_report(
                    buy_threshold_bps,
                    sell_threshold_bps,
                    Decimal::from_micro_units(quantity_micro_units),
                    &report,
                ));
            }
        }
    }

    results.sort_by(|lhs, rhs| {
        rhs.net_profit_loss_quote
            .cmp(&lhs.net_profit_loss_quote)
            .then_with(|| lhs.max_drawdown_pct.total_cmp(&rhs.max_drawdown_pct))
            .then_with(|| rhs.filled_order_count.cmp(&lhs.filled_order_count))
    });

    Ok(SweepReport {
        sqlite_path: sqlite_path.to_string(),
        result_count: results.len(),
        results,
    })
}

impl SweepResult {
    fn from_report(
        buy_threshold_bps: i64,
        sell_threshold_bps: i64,
        quantity_base: Decimal,
        report: &BacktestReport,
    ) -> Self {
        Self {
            buy_threshold_bps,
            sell_threshold_bps,
            quantity_base,
            net_profit_loss_quote: report.profit_loss_quote,
            return_pct: report.return_pct,
            buy_and_hold_delta_quote: report.profit_loss_quote
                - report.buy_and_hold_profit_loss_quote,
            max_drawdown_pct: report.max_drawdown_pct,
            filled_order_count: report.filled_order_count,
            rejected_order_count: report.rejected_order_count,
            buy_count: report.buy_count,
            sell_count: report.sell_count,
            exposure_pct: report.exposure_pct,
            final_base_balance: report.final_base_balance,
        }
    }
}

impl Display for SweepReport {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Sweep report")?;
        writeln!(f, "SQLite source: {}", self.sqlite_path)?;
        writeln!(f, "Combinations: {}", self.result_count)?;
        writeln!(
            f,
            "{:>4} {:>5} {:>8} {:>12} {:>8} {:>12} {:>8} {:>7} {:>7} {:>7} {:>9} {:>10}",
            "buy",
            "sell",
            "qty",
            "pnl",
            "ret%",
            "vs_hold",
            "dd%",
            "fills",
            "rej",
            "b/s",
            "exposure",
            "final_base"
        )?;

        for result in self.results.iter().take(25) {
            writeln!(
                f,
                "{:>4} {:>5} {:>8} {:>12} {:>8.2} {:>12} {:>8.2} {:>7} {:>7} {:>3}/{:<3} {:>8.2}% {:>10}",
                result.buy_threshold_bps,
                result.sell_threshold_bps,
                result.quantity_base,
                result.net_profit_loss_quote,
                result.return_pct,
                result.buy_and_hold_delta_quote,
                result.max_drawdown_pct,
                result.filled_order_count,
                result.rejected_order_count,
                result.buy_count,
                result.sell_count,
                result.exposure_pct,
                result.final_base_balance,
            )?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::run;
    use crate::config::{
        BacktestConfig, BotConfig, Config, ExchangeConfig, MarketDataConfig, RiskConfig,
        StorageConfig, StrategyConfig, TelemetryConfig,
    };
    use crate::decimal::Decimal;
    use rusqlite::Connection;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn decimal(value: &str) -> Decimal {
        Decimal::from_decimal_str(value).expect("decimal should parse")
    }

    fn db_path(name: &str) -> PathBuf {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_millis();
        std::env::temp_dir().join(format!("trader-sweep-{name}-{millis}.sqlite"))
    }

    fn config() -> Config {
        Config {
            bot: BotConfig {
                symbol: "BTC-USD".to_string(),
                base_currency: "BTC".to_string(),
                quote_currency: "USD".to_string(),
                paper_starting_quote_balance: decimal("10000"),
            },
            backtest: BacktestConfig {
                fee_bps: 26,
                slippage_bps: 5,
                trade_log_csv_path: None,
            },
            exchange: ExchangeConfig::default(),
            market_data: MarketDataConfig::default(),
            risk: RiskConfig {
                max_order_quote_value: decimal("500"),
                max_position_base: decimal("0.25"),
            },
            strategy: StrategyConfig::default(),
            storage: StorageConfig {
                sqlite_path: "data/test.sqlite".to_string(),
            },
            telemetry: TelemetryConfig { verbose: true },
        }
    }

    #[test]
    fn ranks_parameter_combinations_from_sqlite_events() {
        let path = db_path("sqlite-source");
        let connection = Connection::open(&path).expect("database should open");
        connection
            .execute_batch(
                "
                CREATE TABLE market_events (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    recorded_at_ms INTEGER NOT NULL,
                    symbol TEXT NOT NULL,
                    price_micro_units INTEGER NOT NULL
                );
                INSERT INTO market_events (recorded_at_ms, symbol, price_micro_units) VALUES
                    (1, 'BTC-USD', 100000000),
                    (2, 'BTC-USD', 101000000),
                    (3, 'BTC-USD', 102000000),
                    (4, 'BTC-USD', 101500000),
                    (5, 'BTC-USD', 99000000);
                ",
            )
            .expect("market events should insert");
        drop(connection);

        let report =
            run(&config(), path.to_str().expect("path should be utf8")).expect("sweep should run");

        assert_eq!(report.result_count, 168);
        assert_eq!(report.results.len(), 168);

        fs::remove_file(path).expect("test database should be removed");
    }
}
