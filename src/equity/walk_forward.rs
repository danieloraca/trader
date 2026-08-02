use super::price_data::{DailyBar, DailyPriceData, DailyPriceKind, InputFormat};
use super::simulation::{
    StrategyResult, evaluate_breakout, evaluate_buy_and_hold, evaluate_moving_average,
};
use super::{EquityResearchConfig, Instrument};
use crate::decimal::Decimal;
use crate::error::{BotError, Result};
use std::cmp::Ordering;
use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub struct EquityWalkForwardReport {
    instrument: Instrument,
    session_count: usize,
    first_date: String,
    last_date: String,
    input_format: InputFormat,
    data_kind: DailyPriceKind,
    price_column: String,
    skipped_missing_price_rows: usize,
    train_sessions: usize,
    test_sessions: usize,
    step_sessions: usize,
    minimum_test_trades_per_window: usize,
    skipped_under_warmed_combinations: usize,
    windows: Vec<WindowRange>,
    benchmark: BenchmarkResult,
    results: Vec<CombinationResult>,
}

#[derive(Debug, Clone)]
struct WindowRange {
    train_start: usize,
    train_end: usize,
    test_end: usize,
    train_start_date: String,
    train_end_date: String,
    test_start_date: String,
    test_end_date: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StrategyFamily {
    MovingAverage,
    Breakout,
}

impl Display for StrategyFamily {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MovingAverage => formatter.write_str("ma"),
            Self::Breakout => formatter.write_str("breakout"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ParameterSet {
    family: StrategyFamily,
    first: usize,
    second: usize,
}

impl ParameterSet {
    fn summary(self) -> String {
        format!("{}/{}", self.first, self.second)
    }

    fn evaluate(
        self,
        config: &EquityResearchConfig,
        bars: &[DailyBar],
        data_kind: DailyPriceKind,
        start_index: usize,
    ) -> StrategyResult {
        match self.family {
            StrategyFamily::MovingAverage => evaluate_moving_average(
                config,
                bars,
                data_kind,
                self.first,
                self.second,
                start_index,
            ),
            StrategyFamily::Breakout => evaluate_breakout(
                config,
                bars,
                data_kind,
                self.first,
                self.second,
                start_index,
            ),
        }
    }
}

#[derive(Debug)]
struct CombinationResult {
    parameters: ParameterSet,
    window_count: usize,
    profitable_window_count: usize,
    beat_hold_window_count: usize,
    active_window_count: usize,
    average_train_alpha_pct: f64,
    average_test_return_pct: f64,
    average_test_alpha_pct: f64,
    worst_test_return_pct: f64,
    average_test_sharpe: f64,
    worst_test_drawdown_pct: f64,
    average_test_turnover_pct: f64,
    total_test_trade_count: usize,
    average_test_fees: f64,
    average_test_friction: f64,
}

#[derive(Debug)]
struct BenchmarkResult {
    average_return_pct: f64,
    worst_return_pct: f64,
    average_sharpe: f64,
    worst_drawdown_pct: f64,
}

pub fn run(
    config: &EquityResearchConfig,
    data: &DailyPriceData,
) -> Result<EquityWalkForwardReport> {
    let settings = &config.walk_forward;
    let windows = walk_forward_windows(
        &data.bars,
        settings.train_sessions,
        settings.test_sessions,
        settings.step_sessions,
    );
    if windows.is_empty() {
        return Err(BotError::MarketData(format!(
            "equity walk-forward needs at least {} sessions, but the source contains {}",
            settings.train_sessions + settings.test_sessions,
            data.bars.len()
        )));
    }

    let (parameter_sets, skipped_under_warmed_combinations) = parameter_sets(config);
    if parameter_sets.is_empty() {
        return Err(BotError::Config(
            "all equity walk-forward combinations are under-warmed by the configured training window"
                .to_string(),
        ));
    }

    let benchmark = benchmark_result(config, data, &windows);
    let mut results = parameter_sets
        .into_iter()
        .map(|parameters| evaluate_combination(config, data, &windows, parameters))
        .collect::<Vec<_>>();
    results.sort_by(compare_results);

    Ok(EquityWalkForwardReport {
        instrument: config.instrument.clone(),
        session_count: data.bars.len(),
        first_date: data.bars[0].date_text.clone(),
        last_date: data.bars[data.bars.len() - 1].date_text.clone(),
        input_format: data.input_format,
        data_kind: data.kind,
        price_column: data.price_column.clone(),
        skipped_missing_price_rows: data.skipped_missing_price_rows,
        train_sessions: settings.train_sessions,
        test_sessions: settings.test_sessions,
        step_sessions: settings.step_sessions,
        minimum_test_trades_per_window: settings.minimum_test_trades_per_window,
        skipped_under_warmed_combinations,
        windows,
        benchmark,
        results,
    })
}

fn walk_forward_windows(
    bars: &[DailyBar],
    train_sessions: usize,
    test_sessions: usize,
    step_sessions: usize,
) -> Vec<WindowRange> {
    let mut windows = Vec::new();
    let mut train_start = 0;
    while train_start + train_sessions + test_sessions <= bars.len() {
        let train_end = train_start + train_sessions;
        let test_end = train_end + test_sessions;
        windows.push(WindowRange {
            train_start,
            train_end,
            test_end,
            train_start_date: bars[train_start].date_text.clone(),
            train_end_date: bars[train_end - 1].date_text.clone(),
            test_start_date: bars[train_end].date_text.clone(),
            test_end_date: bars[test_end - 1].date_text.clone(),
        });
        train_start += step_sessions;
    }
    windows
}

fn parameter_sets(config: &EquityResearchConfig) -> (Vec<ParameterSet>, usize) {
    let settings = &config.walk_forward;
    let mut parameters = Vec::new();
    let mut skipped = 0;

    for fast in &settings.ma_fast_windows {
        for slow in &settings.ma_slow_windows {
            if fast >= slow {
                continue;
            }
            if *slow >= settings.train_sessions {
                skipped += 1;
                continue;
            }
            parameters.push(ParameterSet {
                family: StrategyFamily::MovingAverage,
                first: *fast,
                second: *slow,
            });
        }
    }
    for entry in &settings.breakout_entry_windows {
        for exit in &settings.breakout_exit_windows {
            if exit >= entry {
                continue;
            }
            if (*entry).max(*exit) >= settings.train_sessions {
                skipped += 1;
                continue;
            }
            parameters.push(ParameterSet {
                family: StrategyFamily::Breakout,
                first: *entry,
                second: *exit,
            });
        }
    }

    (parameters, skipped)
}

fn benchmark_result(
    config: &EquityResearchConfig,
    data: &DailyPriceData,
    windows: &[WindowRange],
) -> BenchmarkResult {
    let mut returns = Vec::with_capacity(windows.len());
    let mut sharpes = Vec::with_capacity(windows.len());
    let mut worst_drawdown_pct = 0.0_f64;
    for window in windows {
        let context = &data.bars[window.train_start..window.test_end];
        let result = evaluate_buy_and_hold(
            config,
            context,
            data.kind,
            window.train_end - window.train_start,
        );
        returns.push(result.return_pct);
        sharpes.push(result.sharpe_ratio);
        worst_drawdown_pct = worst_drawdown_pct.max(result.max_drawdown_pct);
    }
    BenchmarkResult {
        average_return_pct: average(&returns),
        worst_return_pct: minimum(&returns),
        average_sharpe: average(&sharpes),
        worst_drawdown_pct,
    }
}

fn evaluate_combination(
    config: &EquityResearchConfig,
    data: &DailyPriceData,
    windows: &[WindowRange],
    parameters: ParameterSet,
) -> CombinationResult {
    let mut train_alphas = Vec::with_capacity(windows.len());
    let mut test_returns = Vec::with_capacity(windows.len());
    let mut test_alphas = Vec::with_capacity(windows.len());
    let mut test_sharpes = Vec::with_capacity(windows.len());
    let mut test_turnovers = Vec::with_capacity(windows.len());
    let mut profitable_window_count = 0;
    let mut beat_hold_window_count = 0;
    let mut active_window_count = 0;
    let mut worst_test_drawdown_pct = 0.0_f64;
    let mut total_test_trade_count = 0;
    let mut total_test_fees = 0.0;
    let mut total_test_friction = 0.0;

    for window in windows {
        let train_bars = &data.bars[window.train_start..window.train_end];
        let train_strategy = parameters.evaluate(config, train_bars, data.kind, 0);
        let train_hold = evaluate_buy_and_hold(config, train_bars, data.kind, 0);
        train_alphas.push(train_strategy.return_pct - train_hold.return_pct);

        let context = &data.bars[window.train_start..window.test_end];
        let test_start = window.train_end - window.train_start;
        let test_strategy = parameters.evaluate(config, context, data.kind, test_start);
        let test_hold = evaluate_buy_and_hold(config, context, data.kind, test_start);
        let alpha = test_strategy.return_pct - test_hold.return_pct;

        if test_strategy.return_pct > 0.0 {
            profitable_window_count += 1;
        }
        if alpha > 0.0 {
            beat_hold_window_count += 1;
        }
        if test_strategy.trade_count >= config.walk_forward.minimum_test_trades_per_window {
            active_window_count += 1;
        }
        worst_test_drawdown_pct = worst_test_drawdown_pct.max(test_strategy.max_drawdown_pct);
        total_test_trade_count += test_strategy.trade_count;
        total_test_fees += decimal_to_f64(test_strategy.total_fees);
        total_test_friction += decimal_to_f64(test_strategy.execution_friction);
        test_returns.push(test_strategy.return_pct);
        test_alphas.push(alpha);
        test_sharpes.push(test_strategy.sharpe_ratio);
        test_turnovers.push(test_strategy.turnover_pct);
    }

    let window_count = windows.len();
    CombinationResult {
        parameters,
        window_count,
        profitable_window_count,
        beat_hold_window_count,
        active_window_count,
        average_train_alpha_pct: average(&train_alphas),
        average_test_return_pct: average(&test_returns),
        average_test_alpha_pct: average(&test_alphas),
        worst_test_return_pct: minimum(&test_returns),
        average_test_sharpe: average(&test_sharpes),
        worst_test_drawdown_pct,
        average_test_turnover_pct: average(&test_turnovers),
        total_test_trade_count,
        average_test_fees: total_test_fees / window_count as f64,
        average_test_friction: total_test_friction / window_count as f64,
    }
}

fn average(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

fn minimum(values: &[f64]) -> f64 {
    values.iter().copied().fold(f64::INFINITY, f64::min)
}

fn decimal_to_f64(value: Decimal) -> f64 {
    value.micro_units() as f64 / 1_000_000.0
}

fn required_consistent_windows(window_count: usize) -> usize {
    (window_count * 7).div_ceil(10)
}

fn is_candidate(result: &CombinationResult) -> bool {
    let required = required_consistent_windows(result.window_count);
    result.profitable_window_count >= required
        && result.beat_hold_window_count >= required
        && result.active_window_count == result.window_count
        && result.average_test_return_pct > 0.0
        && result.average_test_alpha_pct > 0.0
        && result.worst_test_return_pct > 0.0
}

fn quality_label(result: &CombinationResult) -> &'static str {
    if is_candidate(result) {
        "candidate"
    } else if result.active_window_count < result.window_count {
        "thin"
    } else {
        "ok"
    }
}

fn compare_results(lhs: &CombinationResult, rhs: &CombinationResult) -> Ordering {
    is_candidate(rhs)
        .cmp(&is_candidate(lhs))
        .then_with(|| {
            rhs.average_test_alpha_pct
                .total_cmp(&lhs.average_test_alpha_pct)
        })
        .then_with(|| {
            rhs.average_test_return_pct
                .total_cmp(&lhs.average_test_return_pct)
        })
        .then_with(|| {
            rhs.worst_test_return_pct
                .total_cmp(&lhs.worst_test_return_pct)
        })
        .then_with(|| {
            lhs.worst_test_drawdown_pct
                .total_cmp(&rhs.worst_test_drawdown_pct)
        })
}

impl Display for EquityWalkForwardReport {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(formatter, "Equity walk-forward report")?;
        writeln!(
            formatter,
            "Instrument: {} ({}; {}; {})",
            self.instrument.symbol,
            self.instrument.asset_class,
            self.instrument.exchange,
            self.instrument.currency
        )?;
        writeln!(
            formatter,
            "Sessions: {} ({} to {})",
            self.session_count, self.first_date, self.last_date
        )?;
        writeln!(
            formatter,
            "Input: {} {}; valuation column: {}",
            self.input_format, self.data_kind, self.price_column
        )?;
        if self.skipped_missing_price_rows > 0 {
            writeln!(
                formatter,
                "Skipped rows: {} without {}",
                self.skipped_missing_price_rows, self.price_column
            )?;
        }
        writeln!(
            formatter,
            "Plan: {} train / {} test / {} step sessions; {} non-overlapping held-out windows",
            self.train_sessions,
            self.test_sessions,
            self.step_sessions,
            self.windows.len()
        )?;
        writeln!(
            formatter,
            "Combinations: {} runnable; {} skipped under-warmed",
            self.results.len(),
            self.skipped_under_warmed_combinations
        )?;
        writeln!(
            formatter,
            "Candidate requires >=70% profitable and buy-and-hold-beating windows, activity in every window, positive average return/alpha, and positive worst-window return"
        )?;
        writeln!(
            formatter,
            "Minimum test trades per active window: {}",
            self.minimum_test_trades_per_window
        )?;
        writeln!(formatter, "Held-out passive benchmarks")?;
        writeln!(
            formatter,
            "benchmark         avg_ret% worst_ret% avg_sharpe worst_dd%"
        )?;
        writeln!(
            formatter,
            "Cash                  0.00       0.00       0.00      0.00"
        )?;
        writeln!(
            formatter,
            "Buy and hold       {:>8.2} {:>10.2} {:>10.2} {:>9.2}",
            self.benchmark.average_return_pct,
            self.benchmark.worst_return_pct,
            self.benchmark.average_sharpe,
            self.benchmark.worst_drawdown_pct
        )?;
        writeln!(
            formatter,
            "Strategy combinations (ranked on held-out results)"
        )?;
        writeln!(
            formatter,
            "strategy   params    quality prof beat active train_a% test_ret% alpha% worst% sharpe    dd% trades turnover% fees friction"
        )?;
        for result in &self.results {
            writeln!(
                formatter,
                "{:<10} {:>7} {:>10} {:>2}/{:<2} {:>2}/{:<2} {:>2}/{:<2} {:>8.2} {:>9.2} {:>6.2} {:>6.2} {:>6.2} {:>6.2} {:>6} {:>9.2} {:>5.2} {:>8.2}",
                result.parameters.family,
                result.parameters.summary(),
                quality_label(result),
                result.profitable_window_count,
                result.window_count,
                result.beat_hold_window_count,
                result.window_count,
                result.active_window_count,
                result.window_count,
                result.average_train_alpha_pct,
                result.average_test_return_pct,
                result.average_test_alpha_pct,
                result.worst_test_return_pct,
                result.average_test_sharpe,
                result.worst_test_drawdown_pct,
                result.total_test_trade_count,
                result.average_test_turnover_pct,
                result.average_test_fees,
                result.average_test_friction,
            )?;
        }
        writeln!(formatter, "Held-out window dates")?;
        for (index, window) in self.windows.iter().enumerate() {
            writeln!(
                formatter,
                "{:>2}: train {}..{} | test {}..{}",
                index + 1,
                window.train_start_date,
                window.train_end_date,
                window.test_start_date,
                window.test_end_date
            )?;
        }
        writeln!(
            formatter,
            "Reviewing these held-out results makes them research data; validate any selected parameters on later unseen data before deployment."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{StrategyFamily, required_consistent_windows, run, walk_forward_windows};
    use crate::decimal::Decimal;
    use crate::equity::price_data::{
        DailyBar, DailyPriceData, DailyPriceKind, InputFormat, TradingDate,
    };
    use crate::equity::{AssetClass, EquityResearchConfig, EquityWalkForwardConfig, Instrument};

    fn bars(count: usize) -> Vec<DailyBar> {
        (0..count)
            .map(|index| {
                let price = Decimal::from_micro_units((100 + index as i64) * 1_000_000);
                DailyBar {
                    date: TradingDate {
                        year: 2020,
                        month: 1,
                        day: index as u32 + 1,
                    },
                    date_text: format!("session-{index}"),
                    open: price,
                    high: price,
                    low: price,
                    close: price,
                    volume: None,
                }
            })
            .collect()
    }

    #[test]
    fn builds_non_overlapping_rolling_test_windows() {
        let windows = walk_forward_windows(&bars(12), 6, 3, 3);

        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].train_start, 0);
        assert_eq!(windows[0].train_end, 6);
        assert_eq!(windows[0].test_end, 9);
        assert_eq!(windows[1].train_start, 3);
        assert_eq!(windows[1].train_end, 9);
        assert_eq!(windows[1].test_end, 12);
    }

    #[test]
    fn seventy_percent_rule_rounds_up() {
        assert_eq!(required_consistent_windows(4), 3);
        assert_eq!(required_consistent_windows(7), 5);
    }

    #[test]
    fn strategy_family_labels_are_compact() {
        assert_eq!(StrategyFamily::MovingAverage.to_string(), "ma");
        assert_eq!(StrategyFamily::Breakout.to_string(), "breakout");
    }

    #[test]
    fn evaluates_parameter_families_over_held_out_windows() {
        let data = DailyPriceData {
            bars: bars(20),
            kind: DailyPriceKind::CloseOnly,
            input_format: InputFormat::Csv,
            price_column: "close".to_string(),
            skipped_missing_price_rows: 0,
        };
        let config = EquityResearchConfig {
            instrument: Instrument {
                symbol: "TEST.L".to_string(),
                asset_class: AssetClass::Etf,
                exchange: "LSE".to_string(),
                currency: "GBP".to_string(),
            },
            initial_cash: Decimal::from_micro_units(10_000_000_000),
            commission_per_order: Decimal::ZERO,
            commission_bps: 0,
            spread_bps: 0,
            slippage_bps: 0,
            fx_bps: 0,
            allow_fractional_shares: false,
            prices_are_adjusted: true,
            annual_trading_days: 252,
            annual_risk_free_rate_pct: 0.0,
            monthly_dca_amount: Decimal::from_micro_units(500_000_000),
            ma_fast_window: 2,
            ma_slow_window: 3,
            breakout_entry_window: 3,
            breakout_exit_window: 2,
            walk_forward: EquityWalkForwardConfig {
                train_sessions: 8,
                test_sessions: 4,
                step_sessions: 4,
                minimum_test_trades_per_window: 1,
                ma_fast_windows: vec![2],
                ma_slow_windows: vec![3],
                breakout_entry_windows: vec![3],
                breakout_exit_windows: vec![2],
            },
        };

        let report = run(&config, &data).expect("walk-forward report should run");
        let output = report.to_string();

        assert_eq!(report.windows.len(), 3);
        assert_eq!(report.results.len(), 2);
        assert!(output.contains("Held-out passive benchmarks"));
        assert!(output.contains("ma"));
        assert!(output.contains("breakout"));
    }
}
