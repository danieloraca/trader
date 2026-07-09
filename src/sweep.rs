use crate::backtest::{self, BacktestReport};
use crate::candles;
use crate::config::{Config, StrategyKind};
use crate::decimal::Decimal;
use crate::error::{BotError, Result};
use rusqlite::{Connection, params};
use std::cmp::Ordering;
use std::fmt::{Display, Formatter};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const BUY_THRESHOLDS_BPS: [i64; 6] = [3, 5, 8, 10, 15, 20];
const SELL_THRESHOLDS_BPS: [i64; 7] = [-3, -5, -8, -10, -15, -20, -30];
const QUANTITY_MICRO_UNITS: [i64; 4] = [500, 1_000, 2_000, 5_000];
const CANDLE_INTERVAL_SECONDS: [i64; 2] = [60, 300];
const FAST_WINDOWS: [usize; 4] = [3, 5, 8, 10];
const SLOW_WINDOWS: [usize; 4] = [15, 30, 60, 120];
const RSI_WINDOWS: [usize; 3] = [7, 14, 21];
const RSI_OVERSOLD_THRESHOLDS: [u8; 3] = [25, 30, 35];
const RSI_OVERBOUGHT_THRESHOLDS: [u8; 3] = [65, 70, 75];
const CANDLE_QUANTITY_MICRO_UNITS: [i64; 3] = [500, 1_000, 2_000];
const TRAIN_SPLIT_BPS: usize = 7_000;
const MIN_TEST_FILLS: usize = 3;

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
    pub recorded_at_ms: i64,
    pub result_count: usize,
    pub skipped_under_warmed_count: usize,
    pub results: Vec<CandleSweepResult>,
}

#[derive(Debug, Clone)]
pub struct CandleSweepResult {
    pub strategy_kind: String,
    pub parameter_summary: String,
    pub interval_seconds: i64,
    pub candle_count: usize,
    pub train_candle_count: usize,
    pub test_candle_count: usize,
    pub fast_window: usize,
    pub slow_window: usize,
    pub quantity_base: Decimal,
    pub train_profit_loss_quote: Decimal,
    pub train_return_pct: f64,
    pub train_buy_and_hold_delta_quote: Decimal,
    pub train_max_drawdown_pct: f64,
    pub train_filled_order_count: usize,
    pub train_rejected_order_count: usize,
    pub train_buy_count: usize,
    pub train_sell_count: usize,
    pub train_exposure_pct: f64,
    pub train_final_base_balance: Decimal,
    pub test_profit_loss_quote: Decimal,
    pub test_return_pct: f64,
    pub test_buy_and_hold_delta_quote: Decimal,
    pub test_max_drawdown_pct: f64,
    pub test_filled_order_count: usize,
    pub test_rejected_order_count: usize,
    pub test_buy_count: usize,
    pub test_sell_count: usize,
    pub test_exposure_pct: f64,
    pub test_final_base_balance: Decimal,
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

    results.sort_by(compare_sweep_results);

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
        let (train_closes, test_closes) = split_train_test(&candle_closes);

        for fast_window in FAST_WINDOWS {
            for slow_window in SLOW_WINDOWS {
                if fast_window >= slow_window {
                    continue;
                }

                if train_closes.len() < slow_window + 1 || test_closes.len() < slow_window + 1 {
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

                    let train_report = backtest::run_from_prices(&candidate, train_closes.clone())?;
                    let test_report = backtest::run_from_prices(&candidate, test_closes.clone())?;
                    results.push(CandleSweepResult::from_report(
                        "ma",
                        &format!("{fast_window}/{slow_window}"),
                        interval_seconds,
                        candles.len(),
                        train_closes.len(),
                        test_closes.len(),
                        fast_window,
                        slow_window,
                        Decimal::from_micro_units(quantity_micro_units),
                        &train_report,
                        &test_report,
                    ));
                }
            }
        }

        for rsi_window in RSI_WINDOWS {
            if train_closes.len() < rsi_window + 2 || test_closes.len() < rsi_window + 2 {
                skipped_under_warmed_count += RSI_OVERSOLD_THRESHOLDS.len()
                    * RSI_OVERBOUGHT_THRESHOLDS.len()
                    * CANDLE_QUANTITY_MICRO_UNITS.len();
                continue;
            }

            for oversold_threshold in RSI_OVERSOLD_THRESHOLDS {
                for overbought_threshold in RSI_OVERBOUGHT_THRESHOLDS {
                    if oversold_threshold >= overbought_threshold {
                        continue;
                    }

                    for quantity_micro_units in CANDLE_QUANTITY_MICRO_UNITS {
                        let mut candidate = config.clone();
                        candidate.strategy.kind = StrategyKind::RsiMeanReversion;
                        candidate.strategy.rsi_mean_reversion.window = rsi_window;
                        candidate.strategy.rsi_mean_reversion.oversold_threshold =
                            oversold_threshold;
                        candidate.strategy.rsi_mean_reversion.overbought_threshold =
                            overbought_threshold;
                        candidate.strategy.rsi_mean_reversion.quantity_base =
                            Decimal::from_micro_units(quantity_micro_units);
                        candidate.backtest.trade_log_csv_path = None;

                        let train_report =
                            backtest::run_from_prices(&candidate, train_closes.clone())?;
                        let test_report =
                            backtest::run_from_prices(&candidate, test_closes.clone())?;
                        results.push(CandleSweepResult::from_report(
                            "rsi",
                            &format!("{rsi_window}:{oversold_threshold}/{overbought_threshold}"),
                            interval_seconds,
                            candles.len(),
                            train_closes.len(),
                            test_closes.len(),
                            rsi_window,
                            0,
                            Decimal::from_micro_units(quantity_micro_units),
                            &train_report,
                            &test_report,
                        ));
                    }
                }
            }
        }
    }

    results.sort_by(compare_candle_sweep_results);

    let report = CandleSweepReport {
        sqlite_path: sqlite_path.to_string(),
        recorded_at_ms: now_ms()?,
        result_count: results.len(),
        skipped_under_warmed_count,
        results,
    };
    save_candle_sweep_report(sqlite_path, &config.bot.symbol, &report)?;

    Ok(report)
}

fn compare_sweep_results(lhs: &SweepResult, rhs: &SweepResult) -> Ordering {
    rhs.net_profit_loss_quote
        .cmp(&lhs.net_profit_loss_quote)
        .then_with(|| lhs.max_drawdown_pct.total_cmp(&rhs.max_drawdown_pct))
        .then_with(|| rhs.filled_order_count.cmp(&lhs.filled_order_count))
}

fn compare_candle_sweep_results(lhs: &CandleSweepResult, rhs: &CandleSweepResult) -> Ordering {
    let lhs_is_candidate = is_candidate(lhs);
    let rhs_is_candidate = is_candidate(rhs);
    let lhs_traded = lhs.train_filled_order_count > 0 && lhs.test_filled_order_count > 0;
    let rhs_traded = rhs.train_filled_order_count > 0 && rhs.test_filled_order_count > 0;
    let lhs_has_enough_test_fills = lhs.test_filled_order_count >= MIN_TEST_FILLS;
    let rhs_has_enough_test_fills = rhs.test_filled_order_count >= MIN_TEST_FILLS;

    rhs_is_candidate
        .cmp(&lhs_is_candidate)
        .then_with(|| rhs_has_enough_test_fills.cmp(&lhs_has_enough_test_fills))
        .then_with(|| rhs_traded.cmp(&lhs_traded))
        .then_with(|| {
            rhs.test_buy_and_hold_delta_quote
                .cmp(&lhs.test_buy_and_hold_delta_quote)
        })
        .then_with(|| {
            rhs.train_buy_and_hold_delta_quote
                .cmp(&lhs.train_buy_and_hold_delta_quote)
        })
        .then_with(|| rhs.test_profit_loss_quote.cmp(&lhs.test_profit_loss_quote))
        .then_with(|| {
            rhs.train_profit_loss_quote
                .cmp(&lhs.train_profit_loss_quote)
        })
        .then_with(|| {
            lhs.test_max_drawdown_pct
                .total_cmp(&rhs.test_max_drawdown_pct)
        })
        .then_with(|| {
            rhs.test_filled_order_count
                .cmp(&lhs.test_filled_order_count)
        })
}

fn is_candidate(result: &CandleSweepResult) -> bool {
    result.test_filled_order_count >= MIN_TEST_FILLS
        && result.test_profit_loss_quote > Decimal::ZERO
        && result.test_buy_and_hold_delta_quote > Decimal::ZERO
}

fn quality_label(result: &CandleSweepResult) -> &'static str {
    if is_candidate(result) {
        "candidate"
    } else if result.test_filled_order_count >= MIN_TEST_FILLS {
        "ok"
    } else {
        "thin"
    }
}

fn split_train_test(prices: &[Decimal]) -> (Vec<Decimal>, Vec<Decimal>) {
    let split_index = (prices.len() * TRAIN_SPLIT_BPS) / 10_000;
    let split_index = split_index.clamp(1, prices.len().saturating_sub(1));
    (
        prices[..split_index].to_vec(),
        prices[split_index..].to_vec(),
    )
}

fn save_candle_sweep_report(
    sqlite_path: &str,
    symbol: &str,
    report: &CandleSweepReport,
) -> Result<()> {
    let mut connection = Connection::open(sqlite_path).map_err(|error| {
        BotError::Storage(format!(
            "failed to open sqlite for strategy research persistence {sqlite_path}: {error}"
        ))
    })?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|error| {
            BotError::Storage(format!("failed to set sqlite busy timeout: {error}"))
        })?;
    connection
        .execute_batch(
            "
            CREATE TABLE IF NOT EXISTS strategy_research_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                recorded_at_ms INTEGER NOT NULL,
                kind TEXT NOT NULL,
                symbol TEXT NOT NULL,
                runnable_count INTEGER NOT NULL,
                skipped_under_warmed_count INTEGER NOT NULL,
                train_split_bps INTEGER NOT NULL DEFAULT 7000,
                min_test_fills INTEGER NOT NULL DEFAULT 3
            );

            CREATE TABLE IF NOT EXISTS strategy_research_results (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id INTEGER NOT NULL,
                rank INTEGER NOT NULL,
                strategy_kind TEXT NOT NULL DEFAULT 'ma',
                parameter_summary TEXT NOT NULL DEFAULT '',
                interval_seconds INTEGER NOT NULL,
                candle_count INTEGER NOT NULL,
                train_candle_count INTEGER NOT NULL DEFAULT 0,
                test_candle_count INTEGER NOT NULL DEFAULT 0,
                fast_window INTEGER NOT NULL,
                slow_window INTEGER NOT NULL,
                quantity_base_micro_units INTEGER NOT NULL,
                pnl_micro_units INTEGER NOT NULL,
                return_pct REAL NOT NULL,
                buy_and_hold_delta_micro_units INTEGER NOT NULL,
                max_drawdown_pct REAL NOT NULL,
                filled_order_count INTEGER NOT NULL,
                rejected_order_count INTEGER NOT NULL,
                buy_count INTEGER NOT NULL,
                sell_count INTEGER NOT NULL,
                exposure_pct REAL NOT NULL,
                final_base_micro_units INTEGER NOT NULL,
                train_pnl_micro_units INTEGER NOT NULL DEFAULT 0,
                train_return_pct REAL NOT NULL DEFAULT 0,
                train_buy_and_hold_delta_micro_units INTEGER NOT NULL DEFAULT 0,
                train_max_drawdown_pct REAL NOT NULL DEFAULT 0,
                train_filled_order_count INTEGER NOT NULL DEFAULT 0,
                train_rejected_order_count INTEGER NOT NULL DEFAULT 0,
                train_buy_count INTEGER NOT NULL DEFAULT 0,
                train_sell_count INTEGER NOT NULL DEFAULT 0,
                train_exposure_pct REAL NOT NULL DEFAULT 0,
                train_final_base_micro_units INTEGER NOT NULL DEFAULT 0,
                test_pnl_micro_units INTEGER NOT NULL DEFAULT 0,
                test_return_pct REAL NOT NULL DEFAULT 0,
                test_buy_and_hold_delta_micro_units INTEGER NOT NULL DEFAULT 0,
                test_max_drawdown_pct REAL NOT NULL DEFAULT 0,
                test_filled_order_count INTEGER NOT NULL DEFAULT 0,
                test_rejected_order_count INTEGER NOT NULL DEFAULT 0,
                test_buy_count INTEGER NOT NULL DEFAULT 0,
                test_sell_count INTEGER NOT NULL DEFAULT 0,
                test_exposure_pct REAL NOT NULL DEFAULT 0,
                test_final_base_micro_units INTEGER NOT NULL DEFAULT 0
            );
            ",
        )
        .map_err(|error| {
            BotError::Storage(format!(
                "failed to migrate strategy research tables: {error}"
            ))
        })?;
    ensure_strategy_research_schema(&connection)?;

    let transaction = connection.transaction().map_err(|error| {
        BotError::Storage(format!(
            "failed to start strategy research transaction: {error}"
        ))
    })?;
    transaction
        .execute(
            "
            INSERT INTO strategy_research_runs (
                recorded_at_ms,
                kind,
                symbol,
                runnable_count,
                skipped_under_warmed_count,
                train_split_bps,
                min_test_fills
            )
            VALUES (?1, 'candle_ma_sweep_train_test', ?2, ?3, ?4, ?5, ?6)
            ",
            params![
                report.recorded_at_ms,
                symbol,
                usize_to_i64(report.result_count, "runnable count")?,
                usize_to_i64(
                    report.skipped_under_warmed_count,
                    "skipped under-warmed count"
                )?,
                usize_to_i64(TRAIN_SPLIT_BPS, "train split bps")?,
                usize_to_i64(MIN_TEST_FILLS, "minimum test fills")?,
            ],
        )
        .map_err(|error| {
            BotError::Storage(format!("failed to save strategy research run: {error}"))
        })?;
    let run_id = transaction.last_insert_rowid();

    for (rank, result) in report.results.iter().enumerate() {
        transaction
            .execute(
                "
                INSERT INTO strategy_research_results (
                    run_id,
                    rank,
                    strategy_kind,
                    parameter_summary,
                    interval_seconds,
                    candle_count,
                    train_candle_count,
                    test_candle_count,
                    fast_window,
                    slow_window,
                    quantity_base_micro_units,
                    pnl_micro_units,
                    return_pct,
                    buy_and_hold_delta_micro_units,
                    max_drawdown_pct,
                    filled_order_count,
                    rejected_order_count,
                    buy_count,
                    sell_count,
                    exposure_pct,
                    final_base_micro_units,
                    train_pnl_micro_units,
                    train_return_pct,
                    train_buy_and_hold_delta_micro_units,
                    train_max_drawdown_pct,
                    train_filled_order_count,
                    train_rejected_order_count,
                    train_buy_count,
                    train_sell_count,
                    train_exposure_pct,
                    train_final_base_micro_units,
                    test_pnl_micro_units,
                    test_return_pct,
                    test_buy_and_hold_delta_micro_units,
                    test_max_drawdown_pct,
                    test_filled_order_count,
                    test_rejected_order_count,
                    test_buy_count,
                    test_sell_count,
                    test_exposure_pct,
                    test_final_base_micro_units
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37, ?38, ?39, ?40, ?41)
                ",
                params![
                    run_id,
                    usize_to_i64(rank + 1, "strategy research rank")?,
                    result.strategy_kind.as_str(),
                    result.parameter_summary.as_str(),
                    result.interval_seconds,
                    usize_to_i64(result.candle_count, "candle count")?,
                    usize_to_i64(result.train_candle_count, "train candle count")?,
                    usize_to_i64(result.test_candle_count, "test candle count")?,
                    usize_to_i64(result.fast_window, "fast window")?,
                    usize_to_i64(result.slow_window, "slow window")?,
                    result.quantity_base.micro_units(),
                    result.train_profit_loss_quote.micro_units(),
                    result.train_return_pct,
                    result.train_buy_and_hold_delta_quote.micro_units(),
                    result.train_max_drawdown_pct,
                    usize_to_i64(result.train_filled_order_count, "filled order count")?,
                    usize_to_i64(result.train_rejected_order_count, "rejected order count")?,
                    usize_to_i64(result.train_buy_count, "buy count")?,
                    usize_to_i64(result.train_sell_count, "sell count")?,
                    result.train_exposure_pct,
                    result.train_final_base_balance.micro_units(),
                    result.train_profit_loss_quote.micro_units(),
                    result.train_return_pct,
                    result.train_buy_and_hold_delta_quote.micro_units(),
                    result.train_max_drawdown_pct,
                    usize_to_i64(result.train_filled_order_count, "train filled order count")?,
                    usize_to_i64(result.train_rejected_order_count, "train rejected order count")?,
                    usize_to_i64(result.train_buy_count, "train buy count")?,
                    usize_to_i64(result.train_sell_count, "train sell count")?,
                    result.train_exposure_pct,
                    result.train_final_base_balance.micro_units(),
                    result.test_profit_loss_quote.micro_units(),
                    result.test_return_pct,
                    result.test_buy_and_hold_delta_quote.micro_units(),
                    result.test_max_drawdown_pct,
                    usize_to_i64(result.test_filled_order_count, "test filled order count")?,
                    usize_to_i64(result.test_rejected_order_count, "test rejected order count")?,
                    usize_to_i64(result.test_buy_count, "test buy count")?,
                    usize_to_i64(result.test_sell_count, "test sell count")?,
                    result.test_exposure_pct,
                    result.test_final_base_balance.micro_units(),
                ],
            )
            .map_err(|error| {
                BotError::Storage(format!("failed to save strategy research result: {error}"))
            })?;
    }

    transaction.commit().map_err(|error| {
        BotError::Storage(format!(
            "failed to commit strategy research transaction: {error}"
        ))
    })?;

    Ok(())
}

fn usize_to_i64(value: usize, label: &str) -> Result<i64> {
    i64::try_from(value).map_err(|_| BotError::Storage(format!("{label} is too large to store")))
}

fn ensure_strategy_research_schema(connection: &Connection) -> Result<()> {
    add_column_if_missing(
        connection,
        "strategy_research_runs",
        "train_split_bps",
        "INTEGER NOT NULL DEFAULT 7000",
    )?;
    add_column_if_missing(
        connection,
        "strategy_research_runs",
        "min_test_fills",
        "INTEGER NOT NULL DEFAULT 3",
    )?;

    for (column, definition) in [
        ("train_candle_count", "INTEGER NOT NULL DEFAULT 0"),
        ("test_candle_count", "INTEGER NOT NULL DEFAULT 0"),
        ("strategy_kind", "TEXT NOT NULL DEFAULT 'ma'"),
        ("parameter_summary", "TEXT NOT NULL DEFAULT ''"),
        ("train_pnl_micro_units", "INTEGER NOT NULL DEFAULT 0"),
        ("train_return_pct", "REAL NOT NULL DEFAULT 0"),
        (
            "train_buy_and_hold_delta_micro_units",
            "INTEGER NOT NULL DEFAULT 0",
        ),
        ("train_max_drawdown_pct", "REAL NOT NULL DEFAULT 0"),
        ("train_filled_order_count", "INTEGER NOT NULL DEFAULT 0"),
        ("train_rejected_order_count", "INTEGER NOT NULL DEFAULT 0"),
        ("train_buy_count", "INTEGER NOT NULL DEFAULT 0"),
        ("train_sell_count", "INTEGER NOT NULL DEFAULT 0"),
        ("train_exposure_pct", "REAL NOT NULL DEFAULT 0"),
        ("train_final_base_micro_units", "INTEGER NOT NULL DEFAULT 0"),
        ("test_pnl_micro_units", "INTEGER NOT NULL DEFAULT 0"),
        ("test_return_pct", "REAL NOT NULL DEFAULT 0"),
        (
            "test_buy_and_hold_delta_micro_units",
            "INTEGER NOT NULL DEFAULT 0",
        ),
        ("test_max_drawdown_pct", "REAL NOT NULL DEFAULT 0"),
        ("test_filled_order_count", "INTEGER NOT NULL DEFAULT 0"),
        ("test_rejected_order_count", "INTEGER NOT NULL DEFAULT 0"),
        ("test_buy_count", "INTEGER NOT NULL DEFAULT 0"),
        ("test_sell_count", "INTEGER NOT NULL DEFAULT 0"),
        ("test_exposure_pct", "REAL NOT NULL DEFAULT 0"),
        ("test_final_base_micro_units", "INTEGER NOT NULL DEFAULT 0"),
    ] {
        add_column_if_missing(connection, "strategy_research_results", column, definition)?;
    }

    Ok(())
}

fn add_column_if_missing(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    if column_exists(connection, table, column)? {
        return Ok(());
    }

    connection
        .execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )
        .map_err(|error| {
            BotError::Storage(format!("failed to add {table}.{column} column: {error}"))
        })?;

    Ok(())
}

fn column_exists(connection: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|error| BotError::Storage(format!("failed to inspect schema: {error}")))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| BotError::Storage(format!("failed to read schema: {error}")))?;

    for name in columns {
        if name.map_err(|error| BotError::Storage(format!("failed to read column: {error}")))?
            == column
        {
            return Ok(true);
        }
    }

    Ok(false)
}

fn now_ms() -> Result<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .map_err(|error| BotError::Storage(format!("system clock is before unix epoch: {error}")))
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
        strategy_kind: &str,
        parameter_summary: &str,
        interval_seconds: i64,
        candle_count: usize,
        train_candle_count: usize,
        test_candle_count: usize,
        fast_window: usize,
        slow_window: usize,
        quantity_base: Decimal,
        train_report: &BacktestReport,
        test_report: &BacktestReport,
    ) -> Self {
        Self {
            strategy_kind: strategy_kind.to_string(),
            parameter_summary: parameter_summary.to_string(),
            interval_seconds,
            candle_count,
            train_candle_count,
            test_candle_count,
            fast_window,
            slow_window,
            quantity_base,
            train_profit_loss_quote: train_report.profit_loss_quote,
            train_return_pct: train_report.return_pct,
            train_buy_and_hold_delta_quote: train_report.profit_loss_quote
                - train_report.buy_and_hold_profit_loss_quote,
            train_max_drawdown_pct: train_report.max_drawdown_pct,
            train_filled_order_count: train_report.filled_order_count,
            train_rejected_order_count: train_report.rejected_order_count,
            train_buy_count: train_report.buy_count,
            train_sell_count: train_report.sell_count,
            train_exposure_pct: train_report.exposure_pct,
            train_final_base_balance: train_report.final_base_balance,
            test_profit_loss_quote: test_report.profit_loss_quote,
            test_return_pct: test_report.return_pct,
            test_buy_and_hold_delta_quote: test_report.profit_loss_quote
                - test_report.buy_and_hold_profit_loss_quote,
            test_max_drawdown_pct: test_report.max_drawdown_pct,
            test_filled_order_count: test_report.filled_order_count,
            test_rejected_order_count: test_report.rejected_order_count,
            test_buy_count: test_report.buy_count,
            test_sell_count: test_report.sell_count,
            test_exposure_pct: test_report.exposure_pct,
            test_final_base_balance: test_report.final_base_balance,
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
        writeln!(f, "Minimum test fills for ranking: {MIN_TEST_FILLS}")?;
        if self.results.is_empty() {
            writeln!(
                f,
                "No runnable combinations yet. Let market data collect longer, then rerun the sweep."
            )?;
            return Ok(());
        }
        writeln!(
            f,
            "{:>8} {:>7} {:>7} {:>7} {:>8} {:>12} {:>8} {:>8} {:>12} {:>12} {:>12} {:>12} {:>7} {:>7} {:>7} {:>7}",
            "interval",
            "candles",
            "train",
            "test",
            "strategy",
            "params",
            "qty",
            "quality",
            "train_pnl",
            "test_pnl",
            "train_alpha",
            "test_alpha",
            "tr_fill",
            "te_fill",
            "tr_b/s",
            "te_b/s"
        )?;

        for result in self.results.iter().take(25) {
            writeln!(
                f,
                "{:>7}s {:>7} {:>7} {:>7} {:>8} {:>12} {:>8} {:>8} {:>12} {:>12} {:>12} {:>12} {:>7} {:>7} {:>3}/{:<3} {:>3}/{:<3}",
                result.interval_seconds,
                result.candle_count,
                result.train_candle_count,
                result.test_candle_count,
                result.strategy_kind,
                result.parameter_summary,
                result.quantity_base,
                quality_label(result),
                result.train_profit_loss_quote,
                result.test_profit_loss_quote,
                result.train_buy_and_hold_delta_quote,
                result.test_buy_and_hold_delta_quote,
                result.train_filled_order_count,
                result.test_filled_order_count,
                result.train_buy_count,
                result.train_sell_count,
                result.test_buy_count,
                result.test_sell_count,
            )?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CandleSweepResult, MIN_TEST_FILLS, compare_candle_sweep_results, run, run_candles,
    };
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

    fn candle_result(test_pnl: &str, test_alpha: &str, test_fills: usize) -> CandleSweepResult {
        CandleSweepResult {
            strategy_kind: "test".to_string(),
            parameter_summary: "x".to_string(),
            interval_seconds: 60,
            candle_count: 100,
            train_candle_count: 70,
            test_candle_count: 30,
            fast_window: 1,
            slow_window: 2,
            quantity_base: decimal("0.001"),
            train_profit_loss_quote: decimal("1"),
            train_return_pct: 0.01,
            train_buy_and_hold_delta_quote: decimal("1"),
            train_max_drawdown_pct: 0.0,
            train_filled_order_count: 3,
            train_rejected_order_count: 0,
            train_buy_count: 2,
            train_sell_count: 1,
            train_exposure_pct: 10.0,
            train_final_base_balance: Decimal::ZERO,
            test_profit_loss_quote: decimal(test_pnl),
            test_return_pct: 0.01,
            test_buy_and_hold_delta_quote: decimal(test_alpha),
            test_max_drawdown_pct: 0.0,
            test_filled_order_count: test_fills,
            test_rejected_order_count: 0,
            test_buy_count: 2,
            test_sell_count: 1,
            test_exposure_pct: 10.0,
            test_final_base_balance: Decimal::ZERO,
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
    fn ranks_candidate_rows_before_non_candidates() {
        let candidate = candle_result("1", "1", MIN_TEST_FILLS);
        let no_profit = candle_result("-1", "10", MIN_TEST_FILLS);
        let no_alpha = candle_result("10", "-1", MIN_TEST_FILLS);
        let thin = candle_result("10", "10", MIN_TEST_FILLS - 1);

        assert_eq!(
            compare_candle_sweep_results(&candidate, &no_profit),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_candle_sweep_results(&candidate, &no_alpha),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_candle_sweep_results(&candidate, &thin),
            std::cmp::Ordering::Less
        );
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

        assert_eq!(report.result_count, report.results.len());
        assert!(report.result_count > 24);
        assert!(report.skipped_under_warmed_count > 0);
        assert!(report.recorded_at_ms > 0);
        assert!(report.results.iter().all(|result| result.candle_count > 0));
        assert!(
            report
                .results
                .iter()
                .any(|result| result.strategy_kind == "rsi")
        );

        let connection = Connection::open(&path).expect("database should open");
        let saved_runs: i64 = connection
            .query_row("SELECT COUNT(*) FROM strategy_research_runs", [], |row| {
                row.get(0)
            })
            .expect("research runs should count");
        let saved_results: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM strategy_research_results",
                [],
                |row| row.get(0),
            )
            .expect("research results should count");
        let min_test_fills: i64 = connection
            .query_row(
                "SELECT min_test_fills FROM strategy_research_runs ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("min test fills should save");
        assert_eq!(saved_runs, 1);
        assert_eq!(saved_results, report.result_count as i64);
        assert_eq!(min_test_fills, 3);
        drop(connection);

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

        assert!(report.result_count < 258);
        assert!(report.skipped_under_warmed_count > 0);
        assert!(
            report
                .results
                .iter()
                .all(|result| result.train_candle_count > 0 && result.test_candle_count > 0)
        );

        fs::remove_file(path).expect("test database should be removed");
    }

    #[test]
    fn ranks_candle_sweep_rows_with_trades_before_no_trade_rows() {
        let path = db_path("sqlite-candles-ranking");
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
            let cycle = index % 30;
            let price_micro_units = if cycle < 15 {
                100_000_000 + (cycle * 500_000)
            } else {
                107_500_000 - ((cycle - 15) * 500_000)
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
        let first_zero_fill_index = report
            .results
            .iter()
            .position(|result| {
                result.train_filled_order_count == 0 || result.test_filled_order_count == 0
            })
            .unwrap_or(report.results.len());

        assert!(
            report.results[..first_zero_fill_index]
                .iter()
                .all(|result| result.train_filled_order_count > 0
                    && result.test_filled_order_count > 0)
        );
        assert!(
            report.results[first_zero_fill_index..]
                .iter()
                .all(|result| result.train_filled_order_count == 0
                    || result.test_filled_order_count == 0)
        );

        fs::remove_file(path).expect("test database should be removed");
    }

    #[test]
    fn ranks_candle_sweep_rows_with_enough_test_fills_first() {
        let path = db_path("sqlite-candles-min-test-fills");
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

        for index in 0..360_i64 {
            let cycle = index % 24;
            let price_micro_units = if cycle < 12 {
                100_000_000 + (cycle * 750_000)
            } else {
                109_000_000 - ((cycle - 12) * 750_000)
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
        let first_thin_index = report
            .results
            .iter()
            .position(|result| result.test_filled_order_count < MIN_TEST_FILLS)
            .unwrap_or(report.results.len());

        assert!(
            report.results[..first_thin_index]
                .iter()
                .all(|result| result.test_filled_order_count >= MIN_TEST_FILLS)
        );
        assert!(
            report.results[first_thin_index..]
                .iter()
                .all(|result| result.test_filled_order_count < MIN_TEST_FILLS)
        );

        fs::remove_file(path).expect("test database should be removed");
    }
}
