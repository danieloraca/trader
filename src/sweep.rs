use crate::backtest::{self, BacktestReport};
use crate::candles;
use crate::config::{Config, StrategyKind};
use crate::decimal::Decimal;
use crate::error::{BotError, Result};
use std::fmt::{Display, Formatter};

const BUY_THRESHOLDS_BPS: [i64; 6] = [3, 5, 8, 10, 15, 20];
const SELL_THRESHOLDS_BPS: [i64; 7] = [-3, -5, -8, -10, -15, -20, -30];
const QUANTITY_MICRO_UNITS: [i64; 4] = [500, 1_000, 2_000, 5_000];
const CANDLE_INTERVAL_SECONDS: [i64; 2] = [60, 300];
const FAST_WINDOWS: [usize; 4] = [3, 5, 8, 10];
const SLOW_WINDOWS: [usize; 4] = [15, 30, 60, 120];
const CANDLE_QUANTITY_MICRO_UNITS: [i64; 3] = [500, 1_000, 2_000];

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

#[derive(Debug, Clone)]
pub struct CandleSweepReport {
    pub sqlite_path: String,
    pub result_count: usize,
    pub skipped_under_warmed_count: usize,
    pub results: Vec<CandleSweepResult>,
}

#[derive(Debug, Clone)]
pub struct CandleSweepResult {
    pub interval_seconds: i64,
    pub candle_count: usize,
    pub fast_window: usize,
    pub slow_window: usize,
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

pub fn run_candles(config: &Config, sqlite_path: &str) -> Result<CandleSweepReport> {
    let recorded_prices =
        backtest::load_recorded_prices_from_sqlite(sqlite_path, &config.bot.symbol)?;
    if recorded_prices.is_empty() {
        return Err(BotError::Config(
            "candle sweep price source is empty".to_string(),
        ));
    }

    let mut results = Vec::new();
    let mut skipped_under_warmed_count = 0_usize;

    for interval_seconds in CANDLE_INTERVAL_SECONDS {
        let interval_ms = interval_seconds * 1_000;
        let candles = candles::aggregate_prices_to_candles(&recorded_prices, interval_ms)?;
        let candle_closes = candles
            .iter()
            .map(|candle| candle.close)
            .collect::<Vec<_>>();

        for fast_window in FAST_WINDOWS {
            for slow_window in SLOW_WINDOWS {
                if fast_window >= slow_window {
                    continue;
                }

                let minimum_candles = slow_window + 1;
                if candles.len() < minimum_candles {
                    skipped_under_warmed_count += CANDLE_QUANTITY_MICRO_UNITS.len();
                    continue;
                }

                for quantity_micro_units in CANDLE_QUANTITY_MICRO_UNITS {
                    let mut candidate = config.clone();
                    candidate.strategy.kind = StrategyKind::MovingAverageCrossover;
                    candidate.strategy.moving_average_crossover.fast_window = fast_window;
                    candidate.strategy.moving_average_crossover.slow_window = slow_window;
                    candidate.strategy.moving_average_crossover.quantity_base =
                        Decimal::from_micro_units(quantity_micro_units);
                    candidate.backtest.trade_log_csv_path = None;

                    let report = backtest::run_from_prices(&candidate, candle_closes.clone())?;
                    results.push(CandleSweepResult::from_report(
                        interval_seconds,
                        candles.len(),
                        fast_window,
                        slow_window,
                        Decimal::from_micro_units(quantity_micro_units),
                        &report,
                    ));
                }
            }
        }
    }

    results.sort_by(|lhs, rhs| {
        rhs.net_profit_loss_quote
            .cmp(&lhs.net_profit_loss_quote)
            .then_with(|| lhs.max_drawdown_pct.total_cmp(&rhs.max_drawdown_pct))
            .then_with(|| rhs.filled_order_count.cmp(&lhs.filled_order_count))
    });

    Ok(CandleSweepReport {
        sqlite_path: sqlite_path.to_string(),
        result_count: results.len(),
        skipped_under_warmed_count,
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

impl CandleSweepResult {
    fn from_report(
        interval_seconds: i64,
        candle_count: usize,
        fast_window: usize,
        slow_window: usize,
        quantity_base: Decimal,
        report: &BacktestReport,
    ) -> Self {
        Self {
            interval_seconds,
            candle_count,
            fast_window,
            slow_window,
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

impl Display for CandleSweepReport {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Candle MA sweep report")?;
        writeln!(f, "SQLite source: {}", self.sqlite_path)?;
        writeln!(f, "Runnable combinations: {}", self.result_count)?;
        writeln!(
            f,
            "Skipped under-warmed combinations: {}",
            self.skipped_under_warmed_count
        )?;
        if self.results.is_empty() {
            writeln!(
                f,
                "No runnable combinations yet. Let market data collect longer, then rerun the sweep."
            )?;
            return Ok(());
        }
        writeln!(
            f,
            "{:>8} {:>7} {:>4} {:>4} {:>8} {:>12} {:>8} {:>12} {:>8} {:>7} {:>7} {:>7} {:>9} {:>10}",
            "interval",
            "candles",
            "fast",
            "slow",
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
                "{:>7}s {:>7} {:>4} {:>4} {:>8} {:>12} {:>8.2} {:>12} {:>8.2} {:>7} {:>7} {:>3}/{:<3} {:>8.2}% {:>10}",
                result.interval_seconds,
                result.candle_count,
                result.fast_window,
                result.slow_window,
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
    use super::{run, run_candles};
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

    #[test]
    fn ranks_moving_average_combinations_from_sqlite_candles() {
        let path = db_path("sqlite-candles-source");
        let connection = Connection::open(&path).expect("database should open");
        connection
            .execute(
                "
                CREATE TABLE market_events (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    recorded_at_ms INTEGER NOT NULL,
                    symbol TEXT NOT NULL,
                    price_micro_units INTEGER NOT NULL
                )
                ",
                [],
            )
            .expect("market events table should create");

        for index in 0..180_i64 {
            let price_micro_units = if index < 90 {
                100_000_000 + (index * 100_000)
            } else {
                109_000_000 - ((index - 90) * 100_000)
            };
            connection
                .execute(
                    "
                    INSERT INTO market_events (recorded_at_ms, symbol, price_micro_units)
                    VALUES (?1, 'BTC-USD', ?2)
                    ",
                    (index * 60_000, price_micro_units),
                )
                .expect("market event should insert");
        }
        drop(connection);

        let report = run_candles(&config(), path.to_str().expect("path should be utf8"))
            .expect("candle sweep should run");

        assert_eq!(report.result_count, 72);
        assert_eq!(report.results.len(), 72);
        assert_eq!(report.skipped_under_warmed_count, 24);
        assert!(report.results.iter().all(|result| result.candle_count > 0));

        fs::remove_file(path).expect("test database should be removed");
    }

    #[test]
    fn skips_moving_average_combinations_without_enough_candles() {
        let path = db_path("sqlite-candles-short");
        let connection = Connection::open(&path).expect("database should open");
        connection
            .execute(
                "
                CREATE TABLE market_events (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    recorded_at_ms INTEGER NOT NULL,
                    symbol TEXT NOT NULL,
                    price_micro_units INTEGER NOT NULL
                )
                ",
                [],
            )
            .expect("market events table should create");

        for index in 0..20_i64 {
            connection
                .execute(
                    "
                    INSERT INTO market_events (recorded_at_ms, symbol, price_micro_units)
                    VALUES (?1, 'BTC-USD', ?2)
                    ",
                    (index * 60_000, 100_000_000 + (index * 100_000)),
                )
                .expect("market event should insert");
        }
        drop(connection);

        let report = run_candles(&config(), path.to_str().expect("path should be utf8"))
            .expect("candle sweep should run");

        assert!(report.result_count < 96);
        assert!(report.skipped_under_warmed_count > 0);
        assert!(
            report
                .results
                .iter()
                .all(|result| result.candle_count > result.slow_window)
        );

        fs::remove_file(path).expect("test database should be removed");
    }
}
