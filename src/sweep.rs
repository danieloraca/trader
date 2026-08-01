use crate::backtest::{self, BacktestReport, TradeRecord};
use crate::candles;
use crate::config::{Config, ExchangeKind, StrategyKind};
use crate::decimal::Decimal;
use crate::error::{BotError, Result};
use crate::orders::Side;
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
const RSI_REGIME_WINDOWS: [usize; 2] = [60, 120];
const RSI_EXIT_PROFILES: [RsiExitProfile; 3] = [
    RsiExitProfile::new("tight", 100, 60, 12),
    RsiExitProfile::new("balanced", 200, 100, 24),
    RsiExitProfile::new("wide", 400, 200, 48),
];
const MANAGED_RSI_COOLDOWN_EVENTS: [usize; 2] = [1, 3];
const CANDLE_QUANTITY_MICRO_UNITS: [i64; 3] = [500, 1_000, 2_000];
const DCA_PERIODS: [usize; 3] = [5, 15, 30];
const TREND_WINDOWS: [usize; 3] = [15, 30, 60];
const BREAKOUT_WINDOWS: [usize; 3] = [15, 30, 60];
const TRAIN_SPLIT_BPS: usize = 7_000;
const MIN_TEST_FILLS: usize = 3;
const MAX_CANDIDATE_TRAIN_LOSS_QUOTE: i64 = 10;
const MICRO_UNITS_PER_UNIT: i64 = 1_000_000;
#[cfg(test)]
const MAX_CANDLE_SWEEP_COMBINATIONS: usize = CANDLE_INTERVAL_SECONDS.len()
    * ((FAST_WINDOWS.len() * SLOW_WINDOWS.len() * CANDLE_QUANTITY_MICRO_UNITS.len())
        + (RSI_WINDOWS.len()
            * RSI_OVERSOLD_THRESHOLDS.len()
            * RSI_OVERBOUGHT_THRESHOLDS.len()
            * CANDLE_QUANTITY_MICRO_UNITS.len())
        + (RSI_WINDOWS.len()
            * RSI_OVERSOLD_THRESHOLDS.len()
            * RSI_OVERBOUGHT_THRESHOLDS.len()
            * RSI_EXIT_PROFILES.len()
            * MANAGED_RSI_COOLDOWN_EVENTS.len()
            * CANDLE_QUANTITY_MICRO_UNITS.len())
        + (RSI_WINDOWS.len()
            * RSI_OVERSOLD_THRESHOLDS.len()
            * RSI_OVERBOUGHT_THRESHOLDS.len()
            * RSI_REGIME_WINDOWS.len()
            * RSI_EXIT_PROFILES.len()
            * CANDLE_QUANTITY_MICRO_UNITS.len())
        + 1
        + CANDLE_QUANTITY_MICRO_UNITS.len()
        + (DCA_PERIODS.len() * CANDLE_QUANTITY_MICRO_UNITS.len())
        + (TREND_WINDOWS.len() * DCA_PERIODS.len() * CANDLE_QUANTITY_MICRO_UNITS.len())
        + (BREAKOUT_WINDOWS.len() * CANDLE_QUANTITY_MICRO_UNITS.len()));

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
    pub train_capital_matched_delta_quote: Decimal,
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
    pub test_capital_matched_delta_quote: Decimal,
    pub test_max_drawdown_pct: f64,
    pub test_filled_order_count: usize,
    pub test_rejected_order_count: usize,
    pub test_buy_count: usize,
    pub test_sell_count: usize,
    pub test_exposure_pct: f64,
    pub test_final_base_balance: Decimal,
}

#[derive(Debug, Clone)]
pub struct WalkForwardReport {
    pub sqlite_path: String,
    pub result_count: usize,
    pub skipped_under_warmed_count: usize,
    pub results: Vec<WalkForwardResult>,
}

#[derive(Debug, Clone)]
pub struct WalkForwardResult {
    pub strategy_kind: String,
    pub parameter_summary: String,
    pub cost_profile: String,
    pub assumed_fee_bps: i64,
    pub assumed_slippage_bps: i64,
    pub interval_seconds: i64,
    pub candle_count: usize,
    pub train_window_candles: usize,
    pub test_window_candles: usize,
    pub quantity_base: Decimal,
    pub window_count: usize,
    pub candidate_window_count: usize,
    pub profitable_window_count: usize,
    pub total_test_profit_loss_quote: Decimal,
    pub average_test_profit_loss_quote: Decimal,
    pub average_test_gross_profit_loss_quote: Decimal,
    pub average_test_fee_quote: Decimal,
    pub average_test_slippage_quote: Decimal,
    pub worst_test_profit_loss_quote: Decimal,
    pub average_test_alpha_quote: Decimal,
    pub average_test_match_quote: Decimal,
    pub worst_test_drawdown_pct: f64,
    pub total_test_filled_order_count: usize,
    pub total_test_buy_count: usize,
    pub total_test_sell_count: usize,
    pub take_profit_exit_count: usize,
    pub stop_loss_exit_count: usize,
    pub max_holding_exit_count: usize,
    pub regime_exit_count: usize,
}

#[derive(Debug, Clone, Copy, Default)]
struct WalkForwardWindowDiagnostics {
    gross_profit_loss_quote: Decimal,
    fees_quote: Decimal,
    slippage_quote: Decimal,
    take_profit_exit_count: usize,
    stop_loss_exit_count: usize,
    max_holding_exit_count: usize,
    regime_exit_count: usize,
}

impl WalkForwardWindowDiagnostics {
    fn from_report(report: &BacktestReport) -> Self {
        let mut diagnostics = Self {
            gross_profit_loss_quote: gross_profit_loss_quote(
                report.profit_loss_quote,
                report.total_fees_quote,
                report.total_slippage_quote,
            ),
            fees_quote: report.total_fees_quote,
            slippage_quote: report.total_slippage_quote,
            ..Self::default()
        };

        for trade in &report.trades {
            diagnostics.record_exit_reason(&trade.reason);
        }

        diagnostics
    }

    fn record_exit_reason(&mut self, reason: &str) {
        if reason.starts_with("take profit at") {
            self.take_profit_exit_count += 1;
        } else if reason.starts_with("stop loss at") {
            self.stop_loss_exit_count += 1;
        } else if reason.starts_with("maximum holding period") {
            self.max_holding_exit_count += 1;
        } else if reason.contains("regime invalidated") {
            self.regime_exit_count += 1;
        }
    }
}

fn gross_profit_loss_quote(
    net_profit_loss_quote: Decimal,
    fees_quote: Decimal,
    slippage_quote: Decimal,
) -> Decimal {
    net_profit_loss_quote + fees_quote + slippage_quote
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
                        *train_closes
                            .last()
                            .expect("train closes should not be empty"),
                        *test_closes.last().expect("test closes should not be empty"),
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
                            *train_closes
                                .last()
                                .expect("train closes should not be empty"),
                            *test_closes.last().expect("test closes should not be empty"),
                        ));
                    }
                }
            }
        }

        if config.exchange.kind == ExchangeKind::PaperFutures {
            for rsi_window in RSI_WINDOWS {
                if train_closes.len() < rsi_window + 2 || test_closes.len() < rsi_window + 2 {
                    skipped_under_warmed_count += RSI_OVERSOLD_THRESHOLDS.len()
                        * RSI_OVERBOUGHT_THRESHOLDS.len()
                        * RSI_EXIT_PROFILES.len()
                        * MANAGED_RSI_COOLDOWN_EVENTS.len()
                        * CANDLE_QUANTITY_MICRO_UNITS.len();
                    continue;
                }

                for oversold_threshold in RSI_OVERSOLD_THRESHOLDS {
                    for overbought_threshold in RSI_OVERBOUGHT_THRESHOLDS {
                        if oversold_threshold >= overbought_threshold {
                            continue;
                        }

                        for exit_profile in RSI_EXIT_PROFILES {
                            for cooldown_events in MANAGED_RSI_COOLDOWN_EVENTS {
                                for quantity_micro_units in CANDLE_QUANTITY_MICRO_UNITS {
                                    let mut candidate = config.clone();
                                    candidate.strategy.kind = StrategyKind::ManagedRsi;
                                    candidate.strategy.managed_rsi.window = rsi_window;
                                    candidate.strategy.managed_rsi.oversold_threshold =
                                        oversold_threshold;
                                    candidate.strategy.managed_rsi.overbought_threshold =
                                        overbought_threshold;
                                    candidate.strategy.managed_rsi.quantity_base =
                                        Decimal::from_micro_units(quantity_micro_units);
                                    apply_managed_rsi_profile(
                                        &mut candidate,
                                        exit_profile,
                                        cooldown_events,
                                    );
                                    candidate.strategy.managed_rsi.direction =
                                        candidate.strategy.rsi_mean_reversion.direction;
                                    candidate.backtest.trade_log_csv_path = None;

                                    let train_report = backtest::run_from_prices(
                                        &candidate,
                                        train_closes.clone(),
                                    )?;
                                    let test_report =
                                        backtest::run_from_prices(&candidate, test_closes.clone())?;
                                    results.push(CandleSweepResult::from_report(
                                        "managed_rsi",
                                        &format!(
                                            "{rsi_window}:{oversold_threshold}/{overbought_threshold}@{}/cd{cooldown_events}",
                                            exit_profile.label
                                        ),
                                        interval_seconds,
                                        candles.len(),
                                        train_closes.len(),
                                        test_closes.len(),
                                        rsi_window,
                                        0,
                                        Decimal::from_micro_units(quantity_micro_units),
                                        &train_report,
                                        &test_report,
                                        *train_closes
                                            .last()
                                            .expect("train closes should not be empty"),
                                        *test_closes
                                            .last()
                                            .expect("test closes should not be empty"),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }

        for rsi_window in RSI_WINDOWS {
            for regime_window in RSI_REGIME_WINDOWS {
                let required_closes = (rsi_window + 1).max(regime_window + 1);
                if train_closes.len() < required_closes || test_closes.len() < required_closes {
                    skipped_under_warmed_count += RSI_OVERSOLD_THRESHOLDS.len()
                        * RSI_OVERBOUGHT_THRESHOLDS.len()
                        * RSI_EXIT_PROFILES.len()
                        * CANDLE_QUANTITY_MICRO_UNITS.len();
                    continue;
                }

                for oversold_threshold in RSI_OVERSOLD_THRESHOLDS {
                    for overbought_threshold in RSI_OVERBOUGHT_THRESHOLDS {
                        for exit_profile in RSI_EXIT_PROFILES {
                            for quantity_micro_units in CANDLE_QUANTITY_MICRO_UNITS {
                                let mut candidate = config.clone();
                                candidate.strategy.kind = StrategyKind::RsiRegime;
                                candidate.strategy.rsi_regime.window = rsi_window;
                                candidate.strategy.rsi_regime.oversold_threshold =
                                    oversold_threshold;
                                candidate.strategy.rsi_regime.overbought_threshold =
                                    overbought_threshold;
                                candidate.strategy.rsi_regime.regime_window = regime_window;
                                candidate.strategy.rsi_regime.quantity_base =
                                    Decimal::from_micro_units(quantity_micro_units);
                                apply_rsi_exit_profile(&mut candidate, exit_profile);
                                candidate.strategy.rsi_regime.direction =
                                    candidate.strategy.rsi_mean_reversion.direction;
                                candidate.backtest.trade_log_csv_path = None;

                                let train_report =
                                    backtest::run_from_prices(&candidate, train_closes.clone())?;
                                let test_report =
                                    backtest::run_from_prices(&candidate, test_closes.clone())?;
                                results.push(CandleSweepResult::from_report(
                                "rsi_regime",
                                &format!(
                                    "{rsi_window}:{oversold_threshold}/{overbought_threshold}@{regime_window}/{}",
                                    exit_profile.label
                                ),
                                interval_seconds,
                                candles.len(),
                                train_closes.len(),
                                test_closes.len(),
                                rsi_window,
                                regime_window,
                                Decimal::from_micro_units(quantity_micro_units),
                                &train_report,
                                &test_report,
                                *train_closes.last().expect("train closes should not be empty"),
                                *test_closes.last().expect("test closes should not be empty"),
                            ));
                            }
                        }
                    }
                }
            }
        }

        for breakout_window in BREAKOUT_WINDOWS {
            if train_closes.len() < breakout_window + 1 || test_closes.len() < breakout_window + 1 {
                skipped_under_warmed_count += CANDLE_QUANTITY_MICRO_UNITS.len();
                continue;
            }

            for quantity_micro_units in CANDLE_QUANTITY_MICRO_UNITS {
                let mut candidate = config.clone();
                candidate.strategy.kind = StrategyKind::Breakout;
                candidate.strategy.breakout.window = breakout_window;
                candidate.strategy.breakout.quantity_base =
                    Decimal::from_micro_units(quantity_micro_units);
                candidate.backtest.trade_log_csv_path = None;

                let train_report = backtest::run_from_prices(&candidate, train_closes.clone())?;
                let test_report = backtest::run_from_prices(&candidate, test_closes.clone())?;
                results.push(CandleSweepResult::from_report(
                    "breakout",
                    &breakout_window.to_string(),
                    interval_seconds,
                    candles.len(),
                    train_closes.len(),
                    test_closes.len(),
                    breakout_window,
                    0,
                    Decimal::from_micro_units(quantity_micro_units),
                    &train_report,
                    &test_report,
                    *train_closes
                        .last()
                        .expect("train closes should not be empty"),
                    *test_closes.last().expect("test closes should not be empty"),
                ));
            }
        }

        results.push(candle_baseline_result(
            config,
            "hold_all",
            "all-in",
            interval_seconds,
            candles.len(),
            &train_closes,
            &test_closes,
            0,
            0,
            Decimal::ZERO,
            BaselinePlan::HoldAll,
        )?);

        for quantity_micro_units in CANDLE_QUANTITY_MICRO_UNITS {
            let quantity_base = Decimal::from_micro_units(quantity_micro_units);
            results.push(candle_baseline_result(
                config,
                "hold_fixed",
                "first",
                interval_seconds,
                candles.len(),
                &train_closes,
                &test_closes,
                0,
                0,
                quantity_base,
                BaselinePlan::HoldFixed { quantity_base },
            )?);
        }

        for period in DCA_PERIODS {
            for quantity_micro_units in CANDLE_QUANTITY_MICRO_UNITS {
                let quantity_base = Decimal::from_micro_units(quantity_micro_units);
                results.push(candle_baseline_result(
                    config,
                    "dca",
                    &format!("every-{period}"),
                    interval_seconds,
                    candles.len(),
                    &train_closes,
                    &test_closes,
                    period,
                    0,
                    quantity_base,
                    BaselinePlan::Dca {
                        period,
                        quantity_base,
                    },
                )?);
            }
        }

        for trend_window in TREND_WINDOWS {
            if train_closes.len() < trend_window || test_closes.len() < trend_window {
                skipped_under_warmed_count += DCA_PERIODS.len() * CANDLE_QUANTITY_MICRO_UNITS.len();
                continue;
            }

            for period in DCA_PERIODS {
                for quantity_micro_units in CANDLE_QUANTITY_MICRO_UNITS {
                    let quantity_base = Decimal::from_micro_units(quantity_micro_units);
                    results.push(candle_baseline_result(
                        config,
                        "trend_dca",
                        &format!("{trend_window}/every-{period}"),
                        interval_seconds,
                        candles.len(),
                        &train_closes,
                        &test_closes,
                        trend_window,
                        period,
                        quantity_base,
                        BaselinePlan::TrendDca {
                            trend_window,
                            period,
                            quantity_base,
                        },
                    )?);
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

pub fn run_walk_forward(config: &Config, sqlite_path: &str) -> Result<WalkForwardReport> {
    let recorded_prices =
        backtest::load_recorded_prices_from_sqlite(sqlite_path, &config.bot.symbol)?;
    if recorded_prices.is_empty() {
        return Err(BotError::Config(
            "walk-forward price source is empty".to_string(),
        ));
    }

    let mut results = Vec::new();
    let mut skipped_under_warmed_count = 0_usize;
    let cost_profile_count = walk_forward_cost_profiles(config).len();

    for interval_seconds in CANDLE_INTERVAL_SECONDS {
        let interval_ms = interval_seconds * 1_000;
        let candles = candles::aggregate_prices_to_candles(&recorded_prices, interval_ms)?;
        let candle_closes = candles
            .iter()
            .map(|candle| candle.close)
            .collect::<Vec<_>>();
        let Some(plan) = walk_forward_plan(candle_closes.len()) else {
            skipped_under_warmed_count += walk_forward_strategy_count(config) * cost_profile_count;
            continue;
        };

        for fast_window in FAST_WINDOWS {
            for slow_window in SLOW_WINDOWS {
                if fast_window >= slow_window {
                    continue;
                }

                if plan.train_window_candles < slow_window + 1
                    || plan.test_window_candles < slow_window + 1
                {
                    skipped_under_warmed_count +=
                        CANDLE_QUANTITY_MICRO_UNITS.len() * cost_profile_count;
                    continue;
                }

                for quantity_micro_units in CANDLE_QUANTITY_MICRO_UNITS {
                    results.extend(walk_forward_strategy_results(
                        config,
                        "ma",
                        &format!("{fast_window}/{slow_window}"),
                        interval_seconds,
                        &candle_closes,
                        plan,
                        fast_window,
                        slow_window,
                        Decimal::from_micro_units(quantity_micro_units),
                    )?);
                }
            }
        }

        for rsi_window in RSI_WINDOWS {
            if plan.train_window_candles < rsi_window + 2
                || plan.test_window_candles < rsi_window + 2
            {
                skipped_under_warmed_count += RSI_OVERSOLD_THRESHOLDS.len()
                    * RSI_OVERBOUGHT_THRESHOLDS.len()
                    * CANDLE_QUANTITY_MICRO_UNITS.len()
                    * cost_profile_count;
                continue;
            }

            for oversold_threshold in RSI_OVERSOLD_THRESHOLDS {
                for overbought_threshold in RSI_OVERBOUGHT_THRESHOLDS {
                    if oversold_threshold >= overbought_threshold {
                        continue;
                    }

                    for quantity_micro_units in CANDLE_QUANTITY_MICRO_UNITS {
                        results.extend(walk_forward_strategy_results(
                            config,
                            "rsi",
                            &format!("{rsi_window}:{oversold_threshold}/{overbought_threshold}"),
                            interval_seconds,
                            &candle_closes,
                            plan,
                            rsi_window,
                            0,
                            Decimal::from_micro_units(quantity_micro_units),
                        )?);
                    }
                }
            }
        }

        if config.exchange.kind == ExchangeKind::PaperFutures {
            for rsi_window in RSI_WINDOWS {
                if plan.train_window_candles < rsi_window + 2
                    || plan.test_window_candles < rsi_window + 2
                {
                    skipped_under_warmed_count += RSI_OVERSOLD_THRESHOLDS.len()
                        * RSI_OVERBOUGHT_THRESHOLDS.len()
                        * RSI_EXIT_PROFILES.len()
                        * MANAGED_RSI_COOLDOWN_EVENTS.len()
                        * CANDLE_QUANTITY_MICRO_UNITS.len()
                        * cost_profile_count;
                    continue;
                }

                for oversold_threshold in RSI_OVERSOLD_THRESHOLDS {
                    for overbought_threshold in RSI_OVERBOUGHT_THRESHOLDS {
                        if oversold_threshold >= overbought_threshold {
                            continue;
                        }

                        for exit_profile in RSI_EXIT_PROFILES {
                            for cooldown_events in MANAGED_RSI_COOLDOWN_EVENTS {
                                for quantity_micro_units in CANDLE_QUANTITY_MICRO_UNITS {
                                    results.extend(walk_forward_strategy_results(
                                        config,
                                        "managed_rsi",
                                        &format!(
                                            "{rsi_window}:{oversold_threshold}/{overbought_threshold}@{}/cd{cooldown_events}",
                                            exit_profile.label
                                        ),
                                        interval_seconds,
                                        &candle_closes,
                                        plan,
                                        rsi_window,
                                        0,
                                        Decimal::from_micro_units(quantity_micro_units),
                                    )?);
                                }
                            }
                        }
                    }
                }
            }
        }

        for rsi_window in RSI_WINDOWS {
            for regime_window in RSI_REGIME_WINDOWS {
                let required_closes = (rsi_window + 1).max(regime_window + 1);
                if plan.train_window_candles < required_closes
                    || plan.test_window_candles < required_closes
                {
                    skipped_under_warmed_count += RSI_OVERSOLD_THRESHOLDS.len()
                        * RSI_OVERBOUGHT_THRESHOLDS.len()
                        * RSI_EXIT_PROFILES.len()
                        * CANDLE_QUANTITY_MICRO_UNITS.len()
                        * cost_profile_count;
                    continue;
                }

                for oversold_threshold in RSI_OVERSOLD_THRESHOLDS {
                    for overbought_threshold in RSI_OVERBOUGHT_THRESHOLDS {
                        for exit_profile in RSI_EXIT_PROFILES {
                            for quantity_micro_units in CANDLE_QUANTITY_MICRO_UNITS {
                                results.extend(walk_forward_strategy_results(
                                config,
                                "rsi_regime",
                                &format!(
                                    "{rsi_window}:{oversold_threshold}/{overbought_threshold}@{regime_window}/{}",
                                    exit_profile.label
                                ),
                                interval_seconds,
                                &candle_closes,
                                plan,
                                rsi_window,
                                regime_window,
                                Decimal::from_micro_units(quantity_micro_units),
                            )?);
                            }
                        }
                    }
                }
            }
        }

        for breakout_window in BREAKOUT_WINDOWS {
            if plan.train_window_candles < breakout_window + 1
                || plan.test_window_candles < breakout_window + 1
            {
                skipped_under_warmed_count +=
                    CANDLE_QUANTITY_MICRO_UNITS.len() * cost_profile_count;
                continue;
            }

            for quantity_micro_units in CANDLE_QUANTITY_MICRO_UNITS {
                results.extend(walk_forward_strategy_results(
                    config,
                    "breakout",
                    &breakout_window.to_string(),
                    interval_seconds,
                    &candle_closes,
                    plan,
                    breakout_window,
                    0,
                    Decimal::from_micro_units(quantity_micro_units),
                )?);
            }
        }
    }

    results.sort_by(compare_walk_forward_results);

    Ok(WalkForwardReport {
        sqlite_path: sqlite_path.to_string(),
        result_count: results.len(),
        skipped_under_warmed_count,
        results,
    })
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
    let lhs_profitable = lhs.test_profit_loss_quote > Decimal::ZERO;
    let rhs_profitable = rhs.test_profit_loss_quote > Decimal::ZERO;
    let lhs_has_alpha = lhs.test_buy_and_hold_delta_quote > Decimal::ZERO;
    let rhs_has_alpha = rhs.test_buy_and_hold_delta_quote > Decimal::ZERO;
    let lhs_has_matched_alpha = lhs.test_capital_matched_delta_quote > Decimal::ZERO;
    let rhs_has_matched_alpha = rhs.test_capital_matched_delta_quote > Decimal::ZERO;
    let lhs_train_loss_ok = train_loss_is_acceptable(lhs);
    let rhs_train_loss_ok = train_loss_is_acceptable(rhs);

    rhs_is_candidate
        .cmp(&lhs_is_candidate)
        .then_with(|| rhs_has_enough_test_fills.cmp(&lhs_has_enough_test_fills))
        .then_with(|| rhs_profitable.cmp(&lhs_profitable))
        .then_with(|| rhs_has_alpha.cmp(&lhs_has_alpha))
        .then_with(|| rhs_has_matched_alpha.cmp(&lhs_has_matched_alpha))
        .then_with(|| rhs_train_loss_ok.cmp(&lhs_train_loss_ok))
        .then_with(|| rhs_traded.cmp(&lhs_traded))
        .then_with(|| rhs.test_profit_loss_quote.cmp(&lhs.test_profit_loss_quote))
        .then_with(|| {
            rhs.test_capital_matched_delta_quote
                .cmp(&lhs.test_capital_matched_delta_quote)
        })
        .then_with(|| {
            rhs.test_buy_and_hold_delta_quote
                .cmp(&lhs.test_buy_and_hold_delta_quote)
        })
        .then_with(|| {
            rhs.train_buy_and_hold_delta_quote
                .cmp(&lhs.train_buy_and_hold_delta_quote)
        })
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

fn compare_walk_forward_results(lhs: &WalkForwardResult, rhs: &WalkForwardResult) -> Ordering {
    let lhs_is_candidate = is_walk_forward_candidate(lhs);
    let rhs_is_candidate = is_walk_forward_candidate(rhs);
    let lhs_consistency = lhs.candidate_window_count * 10_000 / lhs.window_count.max(1);
    let rhs_consistency = rhs.candidate_window_count * 10_000 / rhs.window_count.max(1);

    rhs_is_candidate
        .cmp(&lhs_is_candidate)
        .then_with(|| rhs_consistency.cmp(&lhs_consistency))
        .then_with(|| {
            rhs.average_test_profit_loss_quote
                .cmp(&lhs.average_test_profit_loss_quote)
        })
        .then_with(|| {
            rhs.worst_test_profit_loss_quote
                .cmp(&lhs.worst_test_profit_loss_quote)
        })
        .then_with(|| {
            rhs.average_test_match_quote
                .cmp(&lhs.average_test_match_quote)
        })
        .then_with(|| {
            lhs.worst_test_drawdown_pct
                .total_cmp(&rhs.worst_test_drawdown_pct)
        })
        .then_with(|| {
            rhs.total_test_filled_order_count
                .cmp(&lhs.total_test_filled_order_count)
        })
}

fn best_walk_forward_results_by_family_and_cost(
    results: &[WalkForwardResult],
) -> Vec<&WalkForwardResult> {
    let mut leaders: Vec<&WalkForwardResult> = Vec::new();

    for result in results {
        if let Some(index) = leaders.iter().position(|leader| {
            leader.strategy_kind == result.strategy_kind
                && leader.cost_profile == result.cost_profile
        }) {
            if compare_walk_forward_results(result, leaders[index]) == Ordering::Less {
                leaders[index] = result;
            }
        } else {
            leaders.push(result);
        }
    }

    leaders.sort_by(|lhs, rhs| {
        lhs.strategy_kind
            .cmp(&rhs.strategy_kind)
            .then_with(|| lhs.cost_profile.cmp(&rhs.cost_profile))
    });
    leaders
}

fn is_candidate(result: &CandleSweepResult) -> bool {
    result.test_filled_order_count >= MIN_TEST_FILLS
        && result.test_profit_loss_quote > Decimal::ZERO
        && result.test_buy_and_hold_delta_quote > Decimal::ZERO
        && result.test_capital_matched_delta_quote > Decimal::ZERO
        && train_loss_is_acceptable(result)
}

fn is_walk_forward_candidate(result: &WalkForwardResult) -> bool {
    result.window_count >= 3
        && result.candidate_window_count * 10 >= result.window_count * 7
        && result.average_test_profit_loss_quote > Decimal::ZERO
        && result.worst_test_profit_loss_quote > Decimal::ZERO
        && result.average_test_alpha_quote > Decimal::ZERO
        && result.average_test_match_quote > Decimal::ZERO
        && result.total_test_filled_order_count >= MIN_TEST_FILLS * result.window_count
}

fn train_loss_is_acceptable(result: &CandleSweepResult) -> bool {
    result.train_profit_loss_quote
        >= Decimal::from_micro_units(-MAX_CANDIDATE_TRAIN_LOSS_QUOTE * MICRO_UNITS_PER_UNIT)
}

fn walk_forward_quality_label(result: &WalkForwardResult) -> &'static str {
    if is_walk_forward_candidate(result) {
        "candidate"
    } else if result.average_test_profit_loss_quote > Decimal::ZERO
        && result.average_test_match_quote > Decimal::ZERO
    {
        "watch"
    } else {
        "ok"
    }
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

#[derive(Debug, Clone, Copy)]
struct WalkForwardPlan {
    train_window_candles: usize,
    test_window_candles: usize,
    step_candles: usize,
}

fn walk_forward_plan(candle_count: usize) -> Option<WalkForwardPlan> {
    if candle_count < 80 {
        return None;
    }

    let test_window_candles = (candle_count / 10).max(30);
    let train_window_candles = test_window_candles * 3;
    if candle_count < train_window_candles + test_window_candles {
        return None;
    }

    Some(WalkForwardPlan {
        train_window_candles,
        test_window_candles,
        step_candles: test_window_candles,
    })
}

fn walk_forward_strategy_count(config: &Config) -> usize {
    let base_count = (FAST_WINDOWS
        .iter()
        .flat_map(|fast| SLOW_WINDOWS.iter().map(move |slow| (*fast, *slow)))
        .filter(|(fast, slow)| fast < slow)
        .count()
        * CANDLE_QUANTITY_MICRO_UNITS.len())
        + (RSI_WINDOWS.len()
            * RSI_OVERSOLD_THRESHOLDS.len()
            * RSI_OVERBOUGHT_THRESHOLDS.len()
            * CANDLE_QUANTITY_MICRO_UNITS.len())
        + (RSI_WINDOWS.len()
            * RSI_OVERSOLD_THRESHOLDS.len()
            * RSI_OVERBOUGHT_THRESHOLDS.len()
            * RSI_REGIME_WINDOWS.len()
            * RSI_EXIT_PROFILES.len()
            * CANDLE_QUANTITY_MICRO_UNITS.len())
        + (BREAKOUT_WINDOWS.len() * CANDLE_QUANTITY_MICRO_UNITS.len());
    let managed_rsi_count = RSI_WINDOWS.len()
        * RSI_OVERSOLD_THRESHOLDS.len()
        * RSI_OVERBOUGHT_THRESHOLDS.len()
        * RSI_EXIT_PROFILES.len()
        * MANAGED_RSI_COOLDOWN_EVENTS.len()
        * CANDLE_QUANTITY_MICRO_UNITS.len();

    base_count
        + if config.exchange.kind == ExchangeKind::PaperFutures {
            managed_rsi_count
        } else {
            0
        }
}

fn walk_forward_windows(
    candle_count: usize,
    plan: WalkForwardPlan,
) -> impl Iterator<Item = (usize, usize, usize)> {
    let mut train_start = 0_usize;
    std::iter::from_fn(move || {
        let train_end = train_start + plan.train_window_candles;
        let test_end = train_end + plan.test_window_candles;
        if test_end > candle_count {
            return None;
        }
        let window = (train_start, train_end, test_end);
        train_start += plan.step_candles;
        Some(window)
    })
}

#[derive(Debug, Clone, Copy)]
struct WalkForwardCostProfile {
    label: &'static str,
    fee_bps: i64,
    slippage_bps: i64,
}

fn walk_forward_cost_profiles(config: &Config) -> Vec<WalkForwardCostProfile> {
    if config.exchange.kind != crate::config::ExchangeKind::PaperFutures {
        let (fee_bps, slippage_bps) = config.backtest.execution_costs(config.exchange.kind);
        return vec![WalkForwardCostProfile {
            label: "configured",
            fee_bps,
            slippage_bps,
        }];
    }

    let base = WalkForwardCostProfile {
        label: "base",
        fee_bps: config.backtest.futures_fee_bps,
        slippage_bps: config.backtest.futures_slippage_bps,
    };
    let stress = WalkForwardCostProfile {
        label: "stress",
        fee_bps: config.backtest.futures_stress_fee_bps,
        slippage_bps: config.backtest.futures_stress_slippage_bps,
    };
    if base.fee_bps == stress.fee_bps && base.slippage_bps == stress.slippage_bps {
        vec![base]
    } else {
        vec![base, stress]
    }
}

#[allow(clippy::too_many_arguments)]
fn walk_forward_strategy_results(
    config: &Config,
    strategy_kind: &str,
    parameter_summary: &str,
    interval_seconds: i64,
    closes: &[Decimal],
    plan: WalkForwardPlan,
    fast_window: usize,
    slow_window: usize,
    quantity_base: Decimal,
) -> Result<Vec<WalkForwardResult>> {
    walk_forward_cost_profiles(config)
        .into_iter()
        .map(|profile| {
            let mut scenario_config = config.clone();
            if scenario_config.exchange.kind == crate::config::ExchangeKind::PaperFutures {
                scenario_config.backtest.futures_fee_bps = profile.fee_bps;
                scenario_config.backtest.futures_slippage_bps = profile.slippage_bps;
            } else {
                scenario_config.backtest.fee_bps = profile.fee_bps;
                scenario_config.backtest.slippage_bps = profile.slippage_bps;
            }
            let mut result = walk_forward_strategy_result(
                &scenario_config,
                strategy_kind,
                parameter_summary,
                interval_seconds,
                closes,
                plan,
                fast_window,
                slow_window,
                quantity_base,
            )?;
            result.cost_profile = profile.label.to_string();
            result.assumed_fee_bps = profile.fee_bps;
            result.assumed_slippage_bps = profile.slippage_bps;
            Ok(result)
        })
        .collect()
}

fn walk_forward_strategy_result(
    config: &Config,
    strategy_kind: &str,
    parameter_summary: &str,
    interval_seconds: i64,
    closes: &[Decimal],
    plan: WalkForwardPlan,
    fast_window: usize,
    slow_window: usize,
    quantity_base: Decimal,
) -> Result<WalkForwardResult> {
    let mut candidate_window_count = 0_usize;
    let mut profitable_window_count = 0_usize;
    let mut total_test_profit_loss_quote = Decimal::ZERO;
    let mut total_test_gross_profit_loss_quote = Decimal::ZERO;
    let mut total_test_fees_quote = Decimal::ZERO;
    let mut total_test_slippage_quote = Decimal::ZERO;
    let mut total_test_alpha_quote = Decimal::ZERO;
    let mut total_test_match_quote = Decimal::ZERO;
    let mut worst_test_profit_loss_quote: Option<Decimal> = None;
    let mut worst_test_drawdown_pct = 0.0_f64;
    let mut total_test_filled_order_count = 0_usize;
    let mut total_test_buy_count = 0_usize;
    let mut total_test_sell_count = 0_usize;
    let mut take_profit_exit_count = 0_usize;
    let mut stop_loss_exit_count = 0_usize;
    let mut max_holding_exit_count = 0_usize;
    let mut regime_exit_count = 0_usize;
    let mut window_count = 0_usize;

    for (train_start, train_end, test_end) in walk_forward_windows(closes.len(), plan) {
        let train_closes = closes[train_start..train_end].to_vec();
        let test_closes = closes[train_end..test_end].to_vec();
        let (result, diagnostics) = strategy_candle_result(
            config,
            strategy_kind,
            parameter_summary,
            interval_seconds,
            closes.len(),
            &train_closes,
            &test_closes,
            fast_window,
            slow_window,
            quantity_base,
        )?;

        if is_candidate(&result) {
            candidate_window_count += 1;
        }
        if result.test_profit_loss_quote > Decimal::ZERO {
            profitable_window_count += 1;
        }

        total_test_profit_loss_quote += result.test_profit_loss_quote;
        total_test_gross_profit_loss_quote += diagnostics.gross_profit_loss_quote;
        total_test_fees_quote += diagnostics.fees_quote;
        total_test_slippage_quote += diagnostics.slippage_quote;
        total_test_alpha_quote += result.test_buy_and_hold_delta_quote;
        total_test_match_quote += result.test_capital_matched_delta_quote;
        worst_test_profit_loss_quote = Some(
            worst_test_profit_loss_quote
                .map(|worst| worst.min(result.test_profit_loss_quote))
                .unwrap_or(result.test_profit_loss_quote),
        );
        if result.test_max_drawdown_pct > worst_test_drawdown_pct {
            worst_test_drawdown_pct = result.test_max_drawdown_pct;
        }
        total_test_filled_order_count += result.test_filled_order_count;
        total_test_buy_count += result.test_buy_count;
        total_test_sell_count += result.test_sell_count;
        take_profit_exit_count += diagnostics.take_profit_exit_count;
        stop_loss_exit_count += diagnostics.stop_loss_exit_count;
        max_holding_exit_count += diagnostics.max_holding_exit_count;
        regime_exit_count += diagnostics.regime_exit_count;
        window_count += 1;
    }

    let window_count_decimal =
        Decimal::from_micro_units(window_count as i64 * MICRO_UNITS_PER_UNIT);
    let average_test_profit_loss_quote = total_test_profit_loss_quote / window_count_decimal;
    let average_test_gross_profit_loss_quote =
        total_test_gross_profit_loss_quote / window_count_decimal;
    let average_test_fee_quote = total_test_fees_quote / window_count_decimal;
    let average_test_slippage_quote = total_test_slippage_quote / window_count_decimal;
    let average_test_alpha_quote = total_test_alpha_quote / window_count_decimal;
    let average_test_match_quote = total_test_match_quote / window_count_decimal;

    Ok(WalkForwardResult {
        strategy_kind: strategy_kind.to_string(),
        parameter_summary: parameter_summary.to_string(),
        cost_profile: "configured".to_string(),
        assumed_fee_bps: config.backtest.execution_costs(config.exchange.kind).0,
        assumed_slippage_bps: config.backtest.execution_costs(config.exchange.kind).1,
        interval_seconds,
        candle_count: closes.len(),
        train_window_candles: plan.train_window_candles,
        test_window_candles: plan.test_window_candles,
        quantity_base,
        window_count,
        candidate_window_count,
        profitable_window_count,
        total_test_profit_loss_quote,
        average_test_profit_loss_quote,
        average_test_gross_profit_loss_quote,
        average_test_fee_quote,
        average_test_slippage_quote,
        worst_test_profit_loss_quote: worst_test_profit_loss_quote.unwrap_or(Decimal::ZERO),
        average_test_alpha_quote,
        average_test_match_quote,
        worst_test_drawdown_pct,
        total_test_filled_order_count,
        total_test_buy_count,
        total_test_sell_count,
        take_profit_exit_count,
        stop_loss_exit_count,
        max_holding_exit_count,
        regime_exit_count,
    })
}

fn strategy_candle_result(
    config: &Config,
    strategy_kind: &str,
    parameter_summary: &str,
    interval_seconds: i64,
    candle_count: usize,
    train_closes: &[Decimal],
    test_closes: &[Decimal],
    fast_window: usize,
    slow_window: usize,
    quantity_base: Decimal,
) -> Result<(CandleSweepResult, WalkForwardWindowDiagnostics)> {
    let mut candidate = config.clone();
    candidate.backtest.trade_log_csv_path = None;

    match strategy_kind {
        "ma" => {
            candidate.strategy.kind = StrategyKind::MovingAverageCrossover;
            candidate.strategy.moving_average_crossover.fast_window = fast_window;
            candidate.strategy.moving_average_crossover.slow_window = slow_window;
            candidate.strategy.moving_average_crossover.quantity_base = quantity_base;
        }
        "rsi" => {
            let (window, oversold_threshold, overbought_threshold) =
                parse_rsi_parameter_summary(parameter_summary)?;
            candidate.strategy.kind = StrategyKind::RsiMeanReversion;
            candidate.strategy.rsi_mean_reversion.window = window;
            candidate.strategy.rsi_mean_reversion.oversold_threshold = oversold_threshold;
            candidate.strategy.rsi_mean_reversion.overbought_threshold = overbought_threshold;
            candidate.strategy.rsi_mean_reversion.quantity_base = quantity_base;
        }
        "managed_rsi" => {
            let (window, oversold_threshold, overbought_threshold, exit_profile, cooldown_events) =
                parse_managed_rsi_parameter_summary(parameter_summary)?;
            candidate.strategy.kind = StrategyKind::ManagedRsi;
            candidate.strategy.managed_rsi.window = window;
            candidate.strategy.managed_rsi.oversold_threshold = oversold_threshold;
            candidate.strategy.managed_rsi.overbought_threshold = overbought_threshold;
            candidate.strategy.managed_rsi.quantity_base = quantity_base;
            apply_managed_rsi_profile(&mut candidate, exit_profile, cooldown_events);
            candidate.strategy.managed_rsi.direction =
                candidate.strategy.rsi_mean_reversion.direction;
        }
        "rsi_regime" => {
            let (window, oversold_threshold, overbought_threshold, regime_window, exit_profile) =
                parse_rsi_regime_parameter_summary(parameter_summary)?;
            candidate.strategy.kind = StrategyKind::RsiRegime;
            candidate.strategy.rsi_regime.window = window;
            candidate.strategy.rsi_regime.oversold_threshold = oversold_threshold;
            candidate.strategy.rsi_regime.overbought_threshold = overbought_threshold;
            candidate.strategy.rsi_regime.regime_window = regime_window;
            candidate.strategy.rsi_regime.quantity_base = quantity_base;
            apply_rsi_exit_profile(&mut candidate, exit_profile);
            candidate.strategy.rsi_regime.direction =
                candidate.strategy.rsi_mean_reversion.direction;
        }
        "breakout" => {
            let breakout_window = parameter_summary.parse::<usize>().map_err(|error| {
                BotError::Config(format!(
                    "invalid breakout parameter summary {parameter_summary}: {error}"
                ))
            })?;
            candidate.strategy.kind = StrategyKind::Breakout;
            candidate.strategy.breakout.window = breakout_window;
            candidate.strategy.breakout.quantity_base = quantity_base;
        }
        _ => {
            return Err(BotError::Config(format!(
                "unsupported walk-forward strategy kind: {strategy_kind}"
            )));
        }
    }

    let train_report = backtest::run_from_prices(&candidate, train_closes.to_vec())?;
    let test_report = backtest::run_from_prices(&candidate, test_closes.to_vec())?;
    let diagnostics = WalkForwardWindowDiagnostics::from_report(&test_report);
    let result = CandleSweepResult::from_report(
        strategy_kind,
        parameter_summary,
        interval_seconds,
        candle_count,
        train_closes.len(),
        test_closes.len(),
        fast_window,
        slow_window,
        quantity_base,
        &train_report,
        &test_report,
        *train_closes
            .last()
            .expect("train closes should not be empty"),
        *test_closes.last().expect("test closes should not be empty"),
    );
    Ok((result, diagnostics))
}

fn parse_rsi_parameter_summary(parameter_summary: &str) -> Result<(usize, u8, u8)> {
    let Some((window, thresholds)) = parameter_summary.split_once(':') else {
        return Err(BotError::Config(format!(
            "invalid RSI parameter summary: {parameter_summary}"
        )));
    };
    let Some((oversold, overbought)) = thresholds.split_once('/') else {
        return Err(BotError::Config(format!(
            "invalid RSI parameter summary: {parameter_summary}"
        )));
    };

    let window = window.parse::<usize>().map_err(|error| {
        BotError::Config(format!(
            "invalid RSI window in parameter summary {parameter_summary}: {error}"
        ))
    })?;
    let oversold = oversold.parse::<u8>().map_err(|error| {
        BotError::Config(format!(
            "invalid RSI oversold threshold in parameter summary {parameter_summary}: {error}"
        ))
    })?;
    let overbought = overbought.parse::<u8>().map_err(|error| {
        BotError::Config(format!(
            "invalid RSI overbought threshold in parameter summary {parameter_summary}: {error}"
        ))
    })?;

    Ok((window, oversold, overbought))
}

fn parse_rsi_regime_parameter_summary(
    parameter_summary: &str,
) -> Result<(usize, u8, u8, usize, RsiExitProfile)> {
    let Some((rsi_summary, regime_summary)) = parameter_summary.split_once('@') else {
        return Err(BotError::Config(format!(
            "invalid regime RSI parameter summary: {parameter_summary}"
        )));
    };
    let (window, oversold, overbought) = parse_rsi_parameter_summary(rsi_summary)?;
    let (regime_window, profile_label) = regime_summary
        .split_once('/')
        .unwrap_or((regime_summary, "balanced"));
    let regime_window = regime_window.parse::<usize>().map_err(|error| {
        BotError::Config(format!(
            "invalid regime window in parameter summary {parameter_summary}: {error}"
        ))
    })?;
    let exit_profile = RSI_EXIT_PROFILES
        .iter()
        .copied()
        .find(|profile| profile.label == profile_label)
        .ok_or_else(|| {
            BotError::Config(format!(
                "invalid regime RSI exit profile in parameter summary: {parameter_summary}"
            ))
        })?;
    Ok((window, oversold, overbought, regime_window, exit_profile))
}

fn parse_managed_rsi_parameter_summary(
    parameter_summary: &str,
) -> Result<(usize, u8, u8, RsiExitProfile, usize)> {
    let Some((rsi_summary, management_summary)) = parameter_summary.split_once('@') else {
        return Err(BotError::Config(format!(
            "invalid managed RSI parameter summary: {parameter_summary}"
        )));
    };
    let (window, oversold, overbought) = parse_rsi_parameter_summary(rsi_summary)?;
    let Some((profile_label, cooldown_label)) = management_summary.split_once('/') else {
        return Err(BotError::Config(format!(
            "invalid managed RSI management summary: {parameter_summary}"
        )));
    };
    let exit_profile = RSI_EXIT_PROFILES
        .iter()
        .copied()
        .find(|profile| profile.label == profile_label)
        .ok_or_else(|| {
            BotError::Config(format!(
                "invalid managed RSI exit profile in parameter summary: {parameter_summary}"
            ))
        })?;
    let cooldown_events = cooldown_label
        .strip_prefix("cd")
        .ok_or_else(|| {
            BotError::Config(format!(
                "invalid managed RSI cooldown in parameter summary: {parameter_summary}"
            ))
        })?
        .parse::<usize>()
        .map_err(|error| {
            BotError::Config(format!(
                "invalid managed RSI cooldown in parameter summary {parameter_summary}: {error}"
            ))
        })?;
    if !MANAGED_RSI_COOLDOWN_EVENTS.contains(&cooldown_events) {
        return Err(BotError::Config(format!(
            "unsupported managed RSI cooldown in parameter summary: {parameter_summary}"
        )));
    }

    Ok((window, oversold, overbought, exit_profile, cooldown_events))
}

#[derive(Debug, Clone, Copy)]
struct RsiExitProfile {
    label: &'static str,
    take_profit_bps: i64,
    stop_loss_bps: i64,
    max_holding_events: usize,
}

impl RsiExitProfile {
    const fn new(
        label: &'static str,
        take_profit_bps: i64,
        stop_loss_bps: i64,
        max_holding_events: usize,
    ) -> Self {
        Self {
            label,
            take_profit_bps,
            stop_loss_bps,
            max_holding_events,
        }
    }
}

fn apply_rsi_exit_profile(config: &mut Config, profile: RsiExitProfile) {
    config.strategy.rsi_regime.take_profit_bps = profile.take_profit_bps;
    config.strategy.rsi_regime.stop_loss_bps = profile.stop_loss_bps;
    config.strategy.rsi_regime.max_holding_events = profile.max_holding_events;
    config.strategy.rsi_regime.exit_on_regime_change = true;
}

fn apply_managed_rsi_profile(config: &mut Config, profile: RsiExitProfile, cooldown_events: usize) {
    config.strategy.managed_rsi.take_profit_bps = profile.take_profit_bps;
    config.strategy.managed_rsi.stop_loss_bps = profile.stop_loss_bps;
    config.strategy.managed_rsi.max_holding_events = profile.max_holding_events;
    config.strategy.managed_rsi.cooldown_events = cooldown_events;
}

fn split_train_test(prices: &[Decimal]) -> (Vec<Decimal>, Vec<Decimal>) {
    let split_index = (prices.len() * TRAIN_SPLIT_BPS) / 10_000;
    let split_index = split_index.clamp(1, prices.len().saturating_sub(1));
    (
        prices[..split_index].to_vec(),
        prices[split_index..].to_vec(),
    )
}

#[derive(Debug, Clone, Copy)]
enum BaselinePlan {
    HoldAll,
    HoldFixed {
        quantity_base: Decimal,
    },
    Dca {
        period: usize,
        quantity_base: Decimal,
    },
    TrendDca {
        trend_window: usize,
        period: usize,
        quantity_base: Decimal,
    },
}

#[derive(Debug, Clone, Copy)]
struct BaselinePortfolio {
    base_balance: Decimal,
    quote_balance: Decimal,
}

fn candle_baseline_result(
    config: &Config,
    strategy_kind: &str,
    parameter_summary: &str,
    interval_seconds: i64,
    candle_count: usize,
    train_closes: &[Decimal],
    test_closes: &[Decimal],
    fast_window: usize,
    slow_window: usize,
    quantity_base: Decimal,
    plan: BaselinePlan,
) -> Result<CandleSweepResult> {
    let train_report = run_baseline_from_prices(config, train_closes, plan)?;
    let test_report = run_baseline_from_prices(config, test_closes, plan)?;

    Ok(CandleSweepResult::from_report(
        strategy_kind,
        parameter_summary,
        interval_seconds,
        candle_count,
        train_closes.len(),
        test_closes.len(),
        fast_window,
        slow_window,
        quantity_base,
        &train_report,
        &test_report,
        *train_closes
            .last()
            .expect("train closes should not be empty"),
        *test_closes.last().expect("test closes should not be empty"),
    ))
}

fn run_baseline_from_prices(
    config: &Config,
    prices: &[Decimal],
    plan: BaselinePlan,
) -> Result<BacktestReport> {
    if prices.is_empty() {
        return Err(BotError::Config(
            "baseline backtest price source is empty".to_string(),
        ));
    }

    let mut portfolio = BaselinePortfolio {
        base_balance: Decimal::ZERO,
        quote_balance: config.bot.paper_starting_quote_balance,
    };
    let mut report = baseline_report(config, prices.len());
    let mut peak_value = report.initial_value_quote;
    let mut exposed_events = 0_usize;

    for (index, price) in prices.iter().copied().enumerate() {
        if portfolio.base_balance > Decimal::ZERO {
            exposed_events += 1;
        }

        if let Some(quantity_base) = baseline_buy_quantity(index, prices, &portfolio, plan, config)
        {
            report.signal_count += 1;
            match fill_baseline_buy(
                &mut portfolio,
                quantity_base,
                price,
                config.backtest.fee_bps,
                config.backtest.slippage_bps,
            ) {
                Ok(mut trade) => {
                    report.filled_order_count += 1;
                    report.buy_count += 1;
                    report.total_fees_quote += trade.fee_quote;
                    report.total_slippage_quote += trade.slippage_quote;
                    trade.event_index = index + 1;
                    report.trades.push(trade);
                }
                Err(BotError::Risk(_)) => {
                    report.rejected_order_count += 1;
                }
                Err(error) => return Err(error),
            }
        }

        let value = baseline_portfolio_value(&portfolio, price);
        if value > peak_value {
            peak_value = value;
        }
        if peak_value > Decimal::ZERO {
            let drawdown = (peak_value - value).ratio_to(peak_value) * 100.0;
            if drawdown > report.max_drawdown_pct {
                report.max_drawdown_pct = drawdown;
            }
        }
    }

    let first_price = prices[0];
    let last_price = prices[prices.len() - 1];
    report.final_base_balance = portfolio.base_balance;
    report.final_quote_balance = portfolio.quote_balance;
    report.final_value_quote = baseline_portfolio_value(&portfolio, last_price);
    report.profit_loss_quote = report.final_value_quote - report.initial_value_quote;
    report.return_pct = report
        .profit_loss_quote
        .ratio_to(report.initial_value_quote)
        * 100.0;
    report.buy_and_hold_value_quote = (report.initial_value_quote / first_price) * last_price;
    report.buy_and_hold_profit_loss_quote =
        report.buy_and_hold_value_quote - report.initial_value_quote;
    report.buy_and_hold_return_pct = report
        .buy_and_hold_profit_loss_quote
        .ratio_to(report.initial_value_quote)
        * 100.0;
    report.exposure_pct = exposed_events as f64 / prices.len() as f64 * 100.0;

    Ok(report)
}

fn baseline_report(config: &Config, event_count: usize) -> BacktestReport {
    BacktestReport {
        symbol: config.bot.symbol.clone(),
        event_count,
        signal_count: 0,
        filled_order_count: 0,
        rejected_order_count: 0,
        buy_count: 0,
        sell_count: 0,
        initial_value_quote: config.bot.paper_starting_quote_balance,
        final_value_quote: config.bot.paper_starting_quote_balance,
        profit_loss_quote: Decimal::ZERO,
        return_pct: 0.0,
        buy_and_hold_value_quote: config.bot.paper_starting_quote_balance,
        buy_and_hold_profit_loss_quote: Decimal::ZERO,
        buy_and_hold_return_pct: 0.0,
        max_drawdown_pct: 0.0,
        total_fees_quote: Decimal::ZERO,
        total_slippage_quote: Decimal::ZERO,
        average_trade_return_pct: 0.0,
        win_count: 0,
        loss_count: 0,
        exposure_pct: 0.0,
        final_base_balance: Decimal::ZERO,
        final_quote_balance: config.bot.paper_starting_quote_balance,
        trade_log_csv_path: None,
        trades: Vec::new(),
    }
}

fn baseline_buy_quantity(
    index: usize,
    prices: &[Decimal],
    portfolio: &BaselinePortfolio,
    plan: BaselinePlan,
    config: &Config,
) -> Option<Decimal> {
    match plan {
        BaselinePlan::HoldAll => {
            if index == 0 {
                Some(max_affordable_quantity(
                    portfolio.quote_balance,
                    prices[index],
                    config.backtest.fee_bps,
                    config.backtest.slippage_bps,
                ))
            } else {
                None
            }
        }
        BaselinePlan::HoldFixed { quantity_base } => (index == 0).then_some(quantity_base),
        BaselinePlan::Dca {
            period,
            quantity_base,
        } => (index % period == 0).then_some(quantity_base),
        BaselinePlan::TrendDca {
            trend_window,
            period,
            quantity_base,
        } => {
            if index % period == 0
                && index + 1 >= trend_window
                && prices[index] > simple_average(&prices[index + 1 - trend_window..=index])
            {
                Some(quantity_base)
            } else {
                None
            }
        }
    }
    .filter(|quantity_base| *quantity_base > Decimal::ZERO)
}

fn fill_baseline_buy(
    portfolio: &mut BaselinePortfolio,
    quantity_base: Decimal,
    price: Decimal,
    fee_bps: i64,
    slippage_bps: i64,
) -> Result<TradeRecord> {
    let slippage = bps_value(price, slippage_bps);
    let fill_price = price + slippage;
    let gross_quote_value = quantity_base * fill_price;
    let fee_quote = bps_value(gross_quote_value, fee_bps);
    let slippage_quote = quantity_base * slippage;
    let total_quote_cost = gross_quote_value + fee_quote;

    if portfolio.quote_balance < total_quote_cost {
        return Err(BotError::Risk(format!(
            "baseline rejected: insufficient quote balance for cost {total_quote_cost}"
        )));
    }

    portfolio.quote_balance -= total_quote_cost;
    portfolio.base_balance += quantity_base;

    Ok(TradeRecord {
        event_index: 0,
        side: Side::Buy,
        quantity_base,
        signal_price: price,
        fill_price,
        gross_quote_value,
        fee_quote,
        slippage_quote,
        equity_after: baseline_portfolio_value(portfolio, price),
        realized_pnl_quote: Decimal::ZERO,
        reason: "baseline buy".to_string(),
    })
}

fn max_affordable_quantity(
    quote_balance: Decimal,
    price: Decimal,
    fee_bps: i64,
    slippage_bps: i64,
) -> Decimal {
    let slippage = bps_value(price, slippage_bps);
    let fill_price = price + slippage;
    let total_cost_per_base = fill_price + bps_value(fill_price, fee_bps);

    if total_cost_per_base <= Decimal::ZERO {
        return Decimal::ZERO;
    }

    quote_balance / total_cost_per_base
}

fn baseline_portfolio_value(portfolio: &BaselinePortfolio, price: Decimal) -> Decimal {
    portfolio.quote_balance + (portfolio.base_balance * price)
}

fn simple_average(values: &[Decimal]) -> Decimal {
    let total = values
        .iter()
        .copied()
        .fold(Decimal::ZERO, |accumulator, value| accumulator + value);
    total / Decimal::from_micro_units(values.len() as i64 * 1_000_000)
}

fn bps_value(value: Decimal, bps: i64) -> Decimal {
    Decimal::from_micro_units(((value.micro_units() as i128 * bps as i128) / 10_000) as i64)
}

fn capital_matched_buy_hold_profit_loss(report: &BacktestReport, final_price: Decimal) -> Decimal {
    let buy_trades = report
        .trades
        .iter()
        .filter(|trade| trade.side == Side::Buy)
        .collect::<Vec<_>>();

    if buy_trades.is_empty() {
        return Decimal::ZERO;
    }

    if report.sell_count == 0 {
        let deployed_quote = buy_trades
            .iter()
            .map(|trade| trade.gross_quote_value + trade.fee_quote)
            .fold(Decimal::ZERO, |accumulator, value| accumulator + value);
        let first_buy = buy_trades[0];
        let first_buy_cost_per_base =
            (first_buy.gross_quote_value + first_buy.fee_quote) / first_buy.quantity_base;

        if first_buy_cost_per_base <= Decimal::ZERO {
            return Decimal::ZERO;
        }

        let benchmark_base = deployed_quote / first_buy_cost_per_base;
        return (benchmark_base * final_price) - deployed_quote;
    }

    buy_trades
        .into_iter()
        .map(|trade| {
            (trade.quantity_base * final_price) - (trade.gross_quote_value + trade.fee_quote)
        })
        .fold(Decimal::ZERO, |accumulator, value| accumulator + value)
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
                train_capital_matched_delta_micro_units INTEGER NOT NULL DEFAULT 0,
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
                test_capital_matched_delta_micro_units INTEGER NOT NULL DEFAULT 0,
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
                    train_capital_matched_delta_micro_units,
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
                    test_capital_matched_delta_micro_units,
                    test_max_drawdown_pct,
                    test_filled_order_count,
                    test_rejected_order_count,
                    test_buy_count,
                    test_sell_count,
                    test_exposure_pct,
                    test_final_base_micro_units
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37, ?38, ?39, ?40, ?41, ?42, ?43)
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
                    result.train_capital_matched_delta_quote.micro_units(),
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
                    result.test_capital_matched_delta_quote.micro_units(),
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
        (
            "train_capital_matched_delta_micro_units",
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
        (
            "test_capital_matched_delta_micro_units",
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
        train_final_price: Decimal,
        test_final_price: Decimal,
    ) -> Self {
        let train_capital_matched_profit_loss =
            capital_matched_buy_hold_profit_loss(train_report, train_final_price);
        let test_capital_matched_profit_loss =
            capital_matched_buy_hold_profit_loss(test_report, test_final_price);

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
            train_capital_matched_delta_quote: train_report.profit_loss_quote
                - train_capital_matched_profit_loss,
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
            test_capital_matched_delta_quote: test_report.profit_loss_quote
                - test_capital_matched_profit_loss,
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
        writeln!(f, "Candle strategy sweep report")?;
        writeln!(f, "SQLite source: {}", self.sqlite_path)?;
        writeln!(f, "Runnable combinations: {}", self.result_count)?;
        writeln!(
            f,
            "Skipped under-warmed combinations: {}",
            self.skipped_under_warmed_count
        )?;
        writeln!(f, "Minimum test fills for ranking: {MIN_TEST_FILLS}")?;
        writeln!(
            f,
            "Candidate requires test P/L > 0, test alpha > 0, test match > 0, and train P/L >= -{MAX_CANDIDATE_TRAIN_LOSS_QUOTE}"
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
            "{:>8} {:>7} {:>7} {:>7} {:>8} {:>12} {:>8} {:>8} {:>12} {:>12} {:>12} {:>12} {:>12} {:>12} {:>7} {:>7} {:>7} {:>7}",
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
            "train_match",
            "test_match",
            "tr_fill",
            "te_fill",
            "tr_b/s",
            "te_b/s"
        )?;

        for result in self.results.iter().take(25) {
            writeln!(
                f,
                "{:>7}s {:>7} {:>7} {:>7} {:>8} {:>12} {:>8} {:>8} {:>12} {:>12} {:>12} {:>12} {:>12} {:>12} {:>7} {:>7} {:>3}/{:<3} {:>3}/{:<3}",
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
                result.train_capital_matched_delta_quote,
                result.test_capital_matched_delta_quote,
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

impl Display for WalkForwardReport {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Walk-forward strategy report")?;
        writeln!(f, "SQLite source: {}", self.sqlite_path)?;
        writeln!(f, "Runnable combinations: {}", self.result_count)?;
        writeln!(
            f,
            "Skipped under-warmed combinations: {}",
            self.skipped_under_warmed_count
        )?;
        writeln!(
            f,
            "Candidate requires >=70% candidate windows, avg/worst test P/L > 0, avg alpha > 0, avg match > 0"
        )?;
        writeln!(
            f,
            "Cost profiles show fee/slippage bps per fill; perpetual funding is not modeled"
        )?;
        if self.results.is_empty() {
            writeln!(
                f,
                "No runnable walk-forward combinations yet. Let market data collect longer, then rerun."
            )?;
            return Ok(());
        }

        writeln!(f, "Best per strategy family and cost profile")?;
        writeln!(
            f,
            "{:>11} {:>7} {:>8} {:>8} {:>26} {:>8} {:>9} {:>9} {:>9} {:>12} {:>12} {:>12} {:>12} {:>8} {:>7} {:>15} {:>7}",
            "strategy",
            "cost",
            "fee/slip",
            "interval",
            "params",
            "qty",
            "quality",
            "cand_win",
            "prof_win",
            "avg_pnl",
            "worst_pnl",
            "avg_alpha",
            "avg_match",
            "worst_dd",
            "fills",
            "exits tp/sl/t/r",
            "b/s"
        )?;

        for result in best_walk_forward_results_by_family_and_cost(&self.results) {
            writeln!(
                f,
                "{:>11} {:>7} {:>3}/{:<4} {:>7}s {:>26} {:>8} {:>9} {:>3}/{:<5} {:>3}/{:<5} {:>12} {:>12} {:>12} {:>12} {:>7.2}% {:>7} {:>3}/{:<3}/{:<3}/{:<3} {:>3}/{:<3}",
                result.strategy_kind,
                result.cost_profile,
                result.assumed_fee_bps,
                result.assumed_slippage_bps,
                result.interval_seconds,
                result.parameter_summary,
                result.quantity_base,
                walk_forward_quality_label(result),
                result.candidate_window_count,
                result.window_count,
                result.profitable_window_count,
                result.window_count,
                result.average_test_profit_loss_quote,
                result.worst_test_profit_loss_quote,
                result.average_test_alpha_quote,
                result.average_test_match_quote,
                result.worst_test_drawdown_pct,
                result.total_test_filled_order_count,
                result.take_profit_exit_count,
                result.stop_loss_exit_count,
                result.max_holding_exit_count,
                result.regime_exit_count,
                result.total_test_buy_count,
                result.total_test_sell_count,
            )?;
        }

        writeln!(f, "Overall leaders")?;
        writeln!(
            f,
            "{:>8} {:>7} {:>7} {:>7} {:>10} {:>26} {:>8} {:>7} {:>8} {:>9} {:>9} {:>9} {:>12} {:>12} {:>12} {:>12} {:>12} {:>12} {:>12} {:>12} {:>8} {:>7} {:>15} {:>7}",
            "interval",
            "candles",
            "train",
            "test",
            "strategy",
            "params",
            "qty",
            "cost",
            "fee/slip",
            "quality",
            "cand_win",
            "prof_win",
            "avg_pnl",
            "worst_pnl",
            "avg_gross",
            "avg_fee",
            "avg_slip",
            "avg_alpha",
            "avg_match",
            "total_pnl",
            "worst_dd",
            "fills",
            "exits tp/sl/t/r",
            "b/s"
        )?;

        for result in self.results.iter().take(25) {
            writeln!(
                f,
                "{:>7}s {:>7} {:>7} {:>7} {:>10} {:>26} {:>8} {:>7} {:>3}/{:<4} {:>9} {:>3}/{:<5} {:>3}/{:<5} {:>12} {:>12} {:>12} {:>12} {:>12} {:>12} {:>12} {:>12} {:>7.2}% {:>7} {:>3}/{:<3}/{:<3}/{:<3} {:>3}/{:<3}",
                result.interval_seconds,
                result.candle_count,
                result.train_window_candles,
                result.test_window_candles,
                result.strategy_kind,
                result.parameter_summary,
                result.quantity_base,
                result.cost_profile,
                result.assumed_fee_bps,
                result.assumed_slippage_bps,
                walk_forward_quality_label(result),
                result.candidate_window_count,
                result.window_count,
                result.profitable_window_count,
                result.window_count,
                result.average_test_profit_loss_quote,
                result.worst_test_profit_loss_quote,
                result.average_test_gross_profit_loss_quote,
                result.average_test_fee_quote,
                result.average_test_slippage_quote,
                result.average_test_alpha_quote,
                result.average_test_match_quote,
                result.total_test_profit_loss_quote,
                result.worst_test_drawdown_pct,
                result.total_test_filled_order_count,
                result.take_profit_exit_count,
                result.stop_loss_exit_count,
                result.max_holding_exit_count,
                result.regime_exit_count,
                result.total_test_buy_count,
                result.total_test_sell_count,
            )?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BaselinePlan, CandleSweepResult, MAX_CANDLE_SWEEP_COMBINATIONS, MIN_TEST_FILLS,
        WalkForwardReport, WalkForwardResult, WalkForwardWindowDiagnostics,
        best_walk_forward_results_by_family_and_cost, capital_matched_buy_hold_profit_loss,
        compare_candle_sweep_results, compare_walk_forward_results, gross_profit_loss_quote,
        is_candidate, run, run_baseline_from_prices, run_candles, strategy_candle_result,
    };
    use crate::config::{
        BacktestConfig, BotConfig, Config, ExchangeConfig, ExchangeKind, MarketDataConfig,
        RiskConfig, StorageConfig, StrategyConfig, StrategyDirection, TelemetryConfig,
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
                futures_fee_bps: 5,
                futures_slippage_bps: 5,
                futures_stress_fee_bps: 5,
                futures_stress_slippage_bps: 10,
                trade_log_csv_path: None,
            },
            exchange: ExchangeConfig::default(),
            market_data: MarketDataConfig::default(),
            risk: RiskConfig {
                max_order_quote_value: decimal("500"),
                max_position_base: decimal("0.25"),
                allow_short: false,
                max_short_position_base: Decimal::ZERO,
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
            train_capital_matched_delta_quote: decimal("1"),
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
            test_capital_matched_delta_quote: decimal(test_alpha),
            test_max_drawdown_pct: 0.0,
            test_filled_order_count: test_fills,
            test_rejected_order_count: 0,
            test_buy_count: 2,
            test_sell_count: 1,
            test_exposure_pct: 10.0,
            test_final_base_balance: Decimal::ZERO,
        }
    }

    fn walk_forward_result() -> WalkForwardResult {
        WalkForwardResult {
            strategy_kind: "test".to_string(),
            parameter_summary: "x".to_string(),
            cost_profile: "base".to_string(),
            assumed_fee_bps: 5,
            assumed_slippage_bps: 5,
            interval_seconds: 300,
            candle_count: 1000,
            train_window_candles: 300,
            test_window_candles: 100,
            quantity_base: decimal("0.0005"),
            window_count: 7,
            candidate_window_count: 1,
            profitable_window_count: 2,
            total_test_profit_loss_quote: decimal("1"),
            average_test_profit_loss_quote: decimal("0.1"),
            average_test_gross_profit_loss_quote: decimal("0.2"),
            average_test_fee_quote: decimal("0.08"),
            average_test_slippage_quote: decimal("0.02"),
            worst_test_profit_loss_quote: decimal("-0.1"),
            average_test_alpha_quote: decimal("1"),
            average_test_match_quote: decimal("0.1"),
            worst_test_drawdown_pct: 0.01,
            total_test_filled_order_count: 21,
            total_test_buy_count: 11,
            total_test_sell_count: 10,
            take_profit_exit_count: 2,
            stop_loss_exit_count: 1,
            max_holding_exit_count: 3,
            regime_exit_count: 4,
        }
    }

    #[test]
    fn cost_diagnostics_do_not_change_walk_forward_ranking() {
        let baseline = walk_forward_result();
        let mut different_diagnostics = baseline.clone();
        different_diagnostics.average_test_gross_profit_loss_quote = decimal("999");
        different_diagnostics.average_test_fee_quote = decimal("998");
        different_diagnostics.average_test_slippage_quote = decimal("0.9");
        different_diagnostics.cost_profile = "stress".to_string();
        different_diagnostics.assumed_fee_bps = 99;
        different_diagnostics.assumed_slippage_bps = 99;
        different_diagnostics.take_profit_exit_count = 999;
        different_diagnostics.stop_loss_exit_count = 999;
        different_diagnostics.max_holding_exit_count = 999;
        different_diagnostics.regime_exit_count = 999;

        assert_eq!(
            compare_walk_forward_results(&baseline, &different_diagnostics),
            std::cmp::Ordering::Equal
        );
    }

    #[test]
    fn selects_best_walk_forward_result_for_each_family_and_cost() {
        let mut weaker_rsi = walk_forward_result();
        weaker_rsi.strategy_kind = "rsi".to_string();
        weaker_rsi.average_test_profit_loss_quote = decimal("1");

        let mut stronger_rsi = weaker_rsi.clone();
        stronger_rsi.average_test_profit_loss_quote = decimal("2");

        let mut stressed_rsi = stronger_rsi.clone();
        stressed_rsi.cost_profile = "stress".to_string();
        stressed_rsi.average_test_profit_loss_quote = decimal("0.5");

        let mut managed_rsi = walk_forward_result();
        managed_rsi.strategy_kind = "managed_rsi".to_string();

        let results = [weaker_rsi, stressed_rsi, managed_rsi, stronger_rsi];
        let leaders = best_walk_forward_results_by_family_and_cost(&results);

        assert_eq!(leaders.len(), 3);
        assert_eq!(leaders[0].strategy_kind, "managed_rsi");
        assert_eq!(leaders[1].strategy_kind, "rsi");
        assert_eq!(leaders[1].cost_profile, "base");
        assert_eq!(leaders[1].average_test_profit_loss_quote, decimal("2"));
        assert_eq!(leaders[2].cost_profile, "stress");
    }

    #[test]
    fn walk_forward_report_renders_family_leaders_before_overall_leaders() {
        let mut managed_rsi = walk_forward_result();
        managed_rsi.strategy_kind = "managed_rsi".to_string();
        managed_rsi.parameter_summary = "21:30/65@balanced/cd1".to_string();
        let report = WalkForwardReport {
            sqlite_path: "data/test.sqlite".to_string(),
            result_count: 1,
            skipped_under_warmed_count: 0,
            results: vec![managed_rsi],
        };

        let output = report.to_string();

        let family_position = output
            .find("Best per strategy family and cost profile")
            .expect("family heading should render");
        let overall_position = output
            .find("Overall leaders")
            .expect("overall heading should render");
        assert!(family_position < overall_position);
        assert!(output.contains("managed_rsi"));
        assert!(output.contains("exits tp/sl/t/r"));
    }

    #[test]
    fn gross_profit_loss_adds_back_fees_and_slippage() {
        assert_eq!(
            gross_profit_loss_quote(decimal("-1.78"), decimal("1.75"), decimal("0.35")),
            decimal("0.32")
        );
    }

    #[test]
    fn classifies_managed_exit_reasons() {
        let mut diagnostics = WalkForwardWindowDiagnostics::default();
        diagnostics.record_exit_reason("take profit at 210.00 bps (target 200)");
        diagnostics.record_exit_reason("stop loss at -110.00 bps (limit -100)");
        diagnostics.record_exit_reason("maximum holding period reached at 24 events");
        diagnostics.record_exit_reason("120-tick regime invalidated at MA 64000");
        diagnostics.record_exit_reason("RSI entry");

        assert_eq!(diagnostics.take_profit_exit_count, 1);
        assert_eq!(diagnostics.stop_loss_exit_count, 1);
        assert_eq!(diagnostics.max_holding_exit_count, 1);
        assert_eq!(diagnostics.regime_exit_count, 1);
    }

    #[test]
    fn futures_walk_forward_uses_base_and_stress_cost_profiles() {
        let mut config = config();
        config.exchange.kind = crate::config::ExchangeKind::PaperFutures;

        let profiles = super::walk_forward_cost_profiles(&config);

        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].label, "base");
        assert_eq!((profiles[0].fee_bps, profiles[0].slippage_bps), (5, 5));
        assert_eq!(profiles[1].label, "stress");
        assert_eq!((profiles[1].fee_bps, profiles[1].slippage_bps), (5, 10));
    }

    #[test]
    fn reconstructs_managed_rsi_for_walk_forward_windows() {
        let mut config = config();
        config.exchange.kind = ExchangeKind::PaperFutures;
        config.risk.allow_short = true;
        config.risk.max_short_position_base = decimal("0.25");
        config.strategy.rsi_mean_reversion.direction = StrategyDirection::LongShort;
        let train = ["100", "99", "98", "97", "99", "101", "102", "100"].map(decimal);
        let test = ["100", "101", "102", "103", "101", "99", "98", "100"].map(decimal);

        let (result, _) = strategy_candle_result(
            &config,
            "managed_rsi",
            "3:30/70@tight/cd1",
            60,
            train.len() + test.len(),
            &train,
            &test,
            3,
            0,
            decimal("0.0005"),
        )
        .expect("managed RSI window should run");

        assert_eq!(result.strategy_kind, "managed_rsi");
        assert_eq!(result.parameter_summary, "3:30/70@tight/cd1");
    }

    #[test]
    fn buy_only_capital_matched_benchmark_deploys_at_first_buy() {
        let report = run_baseline_from_prices(
            &config(),
            &[decimal("100"), decimal("110"), decimal("120")],
            BaselinePlan::Dca {
                period: 1,
                quantity_base: decimal("0.001"),
            },
        )
        .expect("baseline backtest should run");

        let matched_profit_loss = capital_matched_buy_hold_profit_loss(&report, decimal("120"));

        assert_eq!(report.buy_count, 3);
        assert_eq!(report.sell_count, 0);
        assert_ne!(matched_profit_loss, report.profit_loss_quote);
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
        let mut no_match = candle_result("10", "10", MIN_TEST_FILLS);
        no_match.test_capital_matched_delta_quote = decimal("-1");
        let mut bad_train = candle_result("10", "10", MIN_TEST_FILLS);
        bad_train.train_profit_loss_quote = decimal("-10.000001");
        let thin = candle_result("10", "10", MIN_TEST_FILLS - 1);

        assert!(is_candidate(&candidate));
        assert!(!is_candidate(&no_match));
        assert!(!is_candidate(&bad_train));

        assert_eq!(
            compare_candle_sweep_results(&candidate, &no_profit),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_candle_sweep_results(&candidate, &no_alpha),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_candle_sweep_results(&candidate, &no_match),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_candle_sweep_results(&candidate, &bad_train),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_candle_sweep_results(&candidate, &thin),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn ranks_profitable_non_candidates_before_unprofitable_defensive_rows() {
        let profitable_without_alpha = candle_result("1", "-10", MIN_TEST_FILLS);
        let unprofitable_with_alpha = candle_result("-1", "10", MIN_TEST_FILLS);

        assert_eq!(
            compare_candle_sweep_results(&profitable_without_alpha, &unprofitable_with_alpha),
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
        assert!(
            report
                .results
                .iter()
                .any(|result| result.strategy_kind == "breakout")
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

        assert!(report.result_count < MAX_CANDLE_SWEEP_COMBINATIONS);
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
    fn includes_baseline_strategy_families_in_candle_sweep() {
        let path = db_path("sqlite-candles-baselines");
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

        for index in 0..240_i64 {
            connection
                .execute(
                    "
                    INSERT INTO market_events (recorded_at_ms, symbol, price_micro_units)
                    VALUES (?1, 'BTC-USD', ?2)
                    ",
                    (index * 60_000, 100_000_000 + (index * 50_000)),
                )
                .expect("market event should insert");
        }
        drop(connection);

        let report = run_candles(&config(), path.to_str().expect("path should be utf8"))
            .expect("candle sweep should run");

        for strategy_kind in ["hold_all", "hold_fixed", "dca", "trend_dca"] {
            assert!(
                report
                    .results
                    .iter()
                    .any(|result| result.strategy_kind == strategy_kind),
                "expected {strategy_kind} rows"
            );
        }

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
