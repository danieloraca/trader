use super::csv_data::DailyBar;
use super::{EquityResearchConfig, Instrument};
use crate::decimal::Decimal;
use crate::error::{BotError, Result};
use std::fmt::{Display, Formatter};

const BPS_DENOMINATOR: i128 = 10_000;
const DECIMAL_SCALE: i64 = 1_000_000;

#[derive(Debug, Clone)]
pub struct EquityResearchReport {
    instrument: Instrument,
    first_date: String,
    last_date: String,
    session_count: usize,
    volume_session_count: usize,
    annual_trading_days: usize,
    initial_cash: Decimal,
    commission_per_order: Decimal,
    commission_bps: i64,
    spread_bps: i64,
    slippage_bps: i64,
    fx_bps: i64,
    fractional_shares: bool,
    prices_are_adjusted: bool,
    ma_slow_window: usize,
    breakout_warmup: usize,
    results: Vec<StrategyResult>,
}

#[derive(Debug, Clone)]
struct StrategyResult {
    name: String,
    final_value: Decimal,
    profit_loss: Decimal,
    return_pct: f64,
    cagr_pct: f64,
    volatility_pct: f64,
    sharpe_ratio: f64,
    max_drawdown_pct: f64,
    versus_hold_pct: f64,
    trade_count: usize,
    turnover_pct: f64,
    total_fees: Decimal,
    execution_friction: Decimal,
    exposure_pct: f64,
    final_shares: Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    BuyAll,
    BuyAmount(Decimal),
    SellAll,
}

#[derive(Debug)]
struct PortfolioState {
    cash: Decimal,
    shares: Decimal,
    trade_count: usize,
    traded_value: Decimal,
    fees: Decimal,
    execution_friction: Decimal,
    exposed_sessions: usize,
    equity_curve: Vec<Decimal>,
}

pub fn run(config: &EquityResearchConfig, bars: &[DailyBar]) -> Result<EquityResearchReport> {
    if bars.len() < 2 {
        return Err(BotError::MarketData(
            "equity research requires at least two daily bars".to_string(),
        ));
    }

    let plans = [
        ("Buy and hold".to_string(), buy_and_hold_actions(bars)),
        ("Monthly DCA".to_string(), monthly_dca_actions(config, bars)),
        (
            format!("MA {}/{}", config.ma_fast_window, config.ma_slow_window),
            moving_average_actions(config, bars),
        ),
        (
            format!(
                "Breakout {}/{}",
                config.breakout_entry_window, config.breakout_exit_window
            ),
            breakout_actions(config, bars),
        ),
    ];

    let mut results = plans
        .into_iter()
        .map(|(name, actions)| simulate(name, config, bars, &actions))
        .collect::<Vec<_>>();
    let hold_return = results[0].return_pct;
    for result in &mut results {
        result.versus_hold_pct = result.return_pct - hold_return;
    }

    Ok(EquityResearchReport {
        instrument: config.instrument.clone(),
        first_date: bars[0].date_text.clone(),
        last_date: bars[bars.len() - 1].date_text.clone(),
        session_count: bars.len(),
        volume_session_count: bars.iter().filter(|bar| bar.volume.is_some()).count(),
        annual_trading_days: config.annual_trading_days,
        initial_cash: config.initial_cash,
        commission_per_order: config.commission_per_order,
        commission_bps: config.commission_bps,
        spread_bps: config.spread_bps,
        slippage_bps: config.slippage_bps,
        fx_bps: config.fx_bps,
        fractional_shares: config.allow_fractional_shares,
        prices_are_adjusted: config.prices_are_adjusted,
        ma_slow_window: config.ma_slow_window,
        breakout_warmup: config
            .breakout_entry_window
            .max(config.breakout_exit_window),
        results,
    })
}

fn buy_and_hold_actions(bars: &[DailyBar]) -> Vec<Vec<Action>> {
    let mut actions = empty_actions(bars.len());
    actions[0].push(Action::BuyAll);
    actions
}

fn monthly_dca_actions(config: &EquityResearchConfig, bars: &[DailyBar]) -> Vec<Vec<Action>> {
    let mut actions = empty_actions(bars.len());
    let mut previous_month = None;
    for (index, bar) in bars.iter().enumerate() {
        let month = (bar.date.year, bar.date.month);
        if previous_month != Some(month) {
            actions[index].push(Action::BuyAmount(config.monthly_dca_amount));
            previous_month = Some(month);
        }
    }
    actions
}

fn moving_average_actions(config: &EquityResearchConfig, bars: &[DailyBar]) -> Vec<Vec<Action>> {
    let mut actions = empty_actions(bars.len());
    if bars.len() <= config.ma_slow_window {
        return actions;
    }

    let mut previous_invested = None;
    for index in (config.ma_slow_window - 1)..(bars.len() - 1) {
        let slow_start = index + 1 - config.ma_slow_window;
        let fast_start = index + 1 - config.ma_fast_window;
        let slow = average_close(&bars[slow_start..=index]);
        let fast = average_close(&bars[fast_start..=index]);
        let invested = fast > slow;
        if previous_invested != Some(invested) {
            actions[index + 1].push(if invested {
                Action::BuyAll
            } else {
                Action::SellAll
            });
            previous_invested = Some(invested);
        }
    }
    actions
}

fn breakout_actions(config: &EquityResearchConfig, bars: &[DailyBar]) -> Vec<Vec<Action>> {
    let mut actions = empty_actions(bars.len());
    let warmup = config
        .breakout_entry_window
        .max(config.breakout_exit_window);
    if bars.len() <= warmup + 1 {
        return actions;
    }

    let mut invested = false;
    for index in warmup..(bars.len() - 1) {
        let close = bars[index].close;
        if !invested {
            let previous_high = bars[index - config.breakout_entry_window..index]
                .iter()
                .map(|bar| bar.high)
                .max()
                .expect("entry window should not be empty");
            if close > previous_high {
                actions[index + 1].push(Action::BuyAll);
                invested = true;
            }
        } else {
            let previous_low = bars[index - config.breakout_exit_window..index]
                .iter()
                .map(|bar| bar.low)
                .min()
                .expect("exit window should not be empty");
            if close < previous_low {
                actions[index + 1].push(Action::SellAll);
                invested = false;
            }
        }
    }
    actions
}

fn empty_actions(length: usize) -> Vec<Vec<Action>> {
    (0..length).map(|_| Vec::new()).collect()
}

fn average_close(bars: &[DailyBar]) -> Decimal {
    let sum = bars
        .iter()
        .map(|bar| bar.close.micro_units() as i128)
        .sum::<i128>();
    Decimal::from_micro_units((sum / bars.len() as i128) as i64)
}

fn simulate(
    name: String,
    config: &EquityResearchConfig,
    bars: &[DailyBar],
    actions: &[Vec<Action>],
) -> StrategyResult {
    let mut state = PortfolioState {
        cash: config.initial_cash,
        shares: Decimal::ZERO,
        trade_count: 0,
        traded_value: Decimal::ZERO,
        fees: Decimal::ZERO,
        execution_friction: Decimal::ZERO,
        exposed_sessions: 0,
        equity_curve: Vec::with_capacity(bars.len()),
    };

    for (bar, day_actions) in bars.iter().zip(actions) {
        for action in day_actions {
            match action {
                Action::BuyAll => {
                    let budget = state.cash;
                    buy(config, &mut state, bar.open, budget);
                }
                Action::BuyAmount(amount) => {
                    let budget = (*amount).min(state.cash);
                    buy(config, &mut state, bar.open, budget);
                }
                Action::SellAll => sell_all(config, &mut state, bar.open),
            }
        }
        if state.shares > Decimal::ZERO {
            state.exposed_sessions += 1;
        }
        state
            .equity_curve
            .push(state.cash + state.shares * bar.close);
    }

    let final_value = *state
        .equity_curve
        .last()
        .expect("equity curve should contain a value");
    let profit_loss = final_value - config.initial_cash;
    let return_pct = percent_ratio(profit_loss, config.initial_cash);
    let statistics = calculate_statistics(
        &state.equity_curve,
        config.initial_cash,
        config.annual_trading_days,
        config.annual_risk_free_rate_pct,
    );

    StrategyResult {
        name,
        final_value,
        profit_loss,
        return_pct,
        cagr_pct: statistics.cagr_pct,
        volatility_pct: statistics.volatility_pct,
        sharpe_ratio: statistics.sharpe_ratio,
        max_drawdown_pct: statistics.max_drawdown_pct,
        versus_hold_pct: 0.0,
        trade_count: state.trade_count,
        turnover_pct: percent_ratio(state.traded_value, config.initial_cash),
        total_fees: state.fees,
        execution_friction: state.execution_friction,
        exposure_pct: state.exposed_sessions as f64 / bars.len() as f64 * 100.0,
        final_shares: state.shares,
    }
}

fn buy(
    config: &EquityResearchConfig,
    state: &mut PortfolioState,
    market_price: Decimal,
    budget: Decimal,
) {
    if budget <= config.commission_per_order || state.cash <= config.commission_per_order {
        return;
    }

    let execution_price = adjusted_execution_price(config, market_price, true);
    let variable_bps = config.commission_bps + config.fx_bps;
    let unit_variable_fee = bps_amount(execution_price, variable_bps);
    let available = budget - config.commission_per_order;
    let mut quantity = available / (execution_price + unit_variable_fee);
    if !config.allow_fractional_shares {
        quantity = whole_shares(quantity);
    }
    if quantity <= Decimal::ZERO {
        return;
    }

    let gross = execution_price * quantity;
    let variable_fee = bps_amount(gross, variable_bps);
    let fees = config.commission_per_order + variable_fee;
    let total = gross + fees;
    if total > state.cash || total > budget {
        return;
    }

    state.cash -= total;
    state.shares += quantity;
    state.trade_count += 1;
    state.traded_value += gross;
    state.fees += fees;
    state.execution_friction += (execution_price - market_price) * quantity;
}

fn sell_all(config: &EquityResearchConfig, state: &mut PortfolioState, market_price: Decimal) {
    if state.shares <= Decimal::ZERO {
        return;
    }

    let execution_price = adjusted_execution_price(config, market_price, false);
    let gross = execution_price * state.shares;
    let variable_fee = bps_amount(gross, config.commission_bps + config.fx_bps);
    let fees = config.commission_per_order + variable_fee;
    if gross <= fees {
        return;
    }

    state.cash += gross - fees;
    state.traded_value += gross;
    state.fees += fees;
    state.execution_friction += (market_price - execution_price) * state.shares;
    state.shares = Decimal::ZERO;
    state.trade_count += 1;
}

fn adjusted_execution_price(
    config: &EquityResearchConfig,
    market_price: Decimal,
    buy: bool,
) -> Decimal {
    let half_spread_plus_slippage = config.spread_bps as i128 + (config.slippage_bps as i128 * 2);
    let impact =
        (market_price.micro_units() as i128 * half_spread_plus_slippage) / (BPS_DENOMINATOR * 2);
    let impact = Decimal::from_micro_units(impact as i64);
    if buy {
        market_price + impact
    } else {
        market_price - impact
    }
}

fn bps_amount(value: Decimal, bps: i64) -> Decimal {
    Decimal::from_micro_units(
        ((value.micro_units() as i128 * bps as i128) / BPS_DENOMINATOR) as i64,
    )
}

fn whole_shares(value: Decimal) -> Decimal {
    Decimal::from_micro_units((value.micro_units() / DECIMAL_SCALE) * DECIMAL_SCALE)
}

struct Statistics {
    cagr_pct: f64,
    volatility_pct: f64,
    sharpe_ratio: f64,
    max_drawdown_pct: f64,
}

fn calculate_statistics(
    equity_curve: &[Decimal],
    initial_cash: Decimal,
    annual_trading_days: usize,
    annual_risk_free_rate_pct: f64,
) -> Statistics {
    let daily_returns = equity_curve
        .windows(2)
        .filter_map(|window| {
            let previous = to_f64(window[0]);
            (previous > 0.0).then(|| to_f64(window[1]) / previous - 1.0)
        })
        .collect::<Vec<_>>();
    let mean = if daily_returns.is_empty() {
        0.0
    } else {
        daily_returns.iter().sum::<f64>() / daily_returns.len() as f64
    };
    let variance = if daily_returns.len() > 1 {
        daily_returns
            .iter()
            .map(|value| (value - mean).powi(2))
            .sum::<f64>()
            / (daily_returns.len() - 1) as f64
    } else {
        0.0
    };
    let daily_std_dev = variance.sqrt();
    let annual_factor = (annual_trading_days as f64).sqrt();
    let volatility_pct = daily_std_dev * annual_factor * 100.0;
    let annual_risk_free = annual_risk_free_rate_pct / 100.0;
    let daily_risk_free = (1.0 + annual_risk_free).powf(1.0 / annual_trading_days as f64) - 1.0;
    let sharpe_ratio = if daily_std_dev > 0.0 {
        (mean - daily_risk_free) / daily_std_dev * annual_factor
    } else {
        0.0
    };

    let final_value = equity_curve.last().copied().unwrap_or(initial_cash);
    let years = (equity_curve.len().saturating_sub(1) as f64 / annual_trading_days as f64)
        .max(1.0 / annual_trading_days as f64);
    let growth_ratio = final_value.ratio_to(initial_cash);
    let cagr_pct = if growth_ratio > 0.0 {
        (growth_ratio.powf(1.0 / years) - 1.0) * 100.0
    } else {
        -100.0
    };

    let mut peak = to_f64(initial_cash);
    let mut max_drawdown = 0.0_f64;
    for value in equity_curve.iter().copied().map(to_f64) {
        peak = peak.max(value);
        if peak > 0.0 {
            max_drawdown = max_drawdown.max((peak - value) / peak * 100.0);
        }
    }

    Statistics {
        cagr_pct,
        volatility_pct,
        sharpe_ratio,
        max_drawdown_pct: max_drawdown,
    }
}

fn to_f64(value: Decimal) -> f64 {
    value.micro_units() as f64 / DECIMAL_SCALE as f64
}

fn percent_ratio(numerator: Decimal, denominator: Decimal) -> f64 {
    numerator.ratio_to(denominator) * 100.0
}

impl Display for EquityResearchReport {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(formatter, "Equity CSV research report")?;
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
            "Sessions: {} ({} to {}; {} assumed/year)",
            self.session_count, self.first_date, self.last_date, self.annual_trading_days
        )?;
        writeln!(
            formatter,
            "Volume coverage: {}/{} sessions",
            self.volume_session_count, self.session_count
        )?;
        writeln!(formatter, "Initial cash: {}", self.initial_cash)?;
        writeln!(
            formatter,
            "Costs: {} fixed/order, {} commission bps, {} spread bps, {} slippage bps, {} FX bps",
            self.commission_per_order,
            self.commission_bps,
            self.spread_bps,
            self.slippage_bps,
            self.fx_bps
        )?;
        writeln!(
            formatter,
            "Shares: {} | Prices: {}",
            if self.fractional_shares {
                "fractional"
            } else {
                "whole only"
            },
            if self.prices_are_adjusted {
                "declared split/dividend adjusted"
            } else {
                "source adjustment unknown (price return only)"
            }
        )?;
        if self.session_count < self.annual_trading_days {
            writeln!(
                formatter,
                "Warning: fewer than one assumed trading year; CAGR, volatility, and Sharpe are unstable."
            )?;
        }
        if self.session_count <= self.ma_slow_window {
            writeln!(
                formatter,
                "Warning: MA strategy is under-warmed (needs more than {} sessions).",
                self.ma_slow_window
            )?;
        }
        if self.session_count <= self.breakout_warmup + 1 {
            writeln!(
                formatter,
                "Warning: breakout strategy is under-warmed (needs more than {} sessions).",
                self.breakout_warmup + 1
            )?;
        }
        writeln!(
            formatter,
            "strategy              final        pnl    ret%    cagr%    vol%  sharpe     dd%  vs_hold  trades turnover%    fees friction exposure% shares"
        )?;
        for result in &self.results {
            writeln!(
                formatter,
                "{:<18} {:>11.2} {:>10.2} {:>7.2} {:>8.2} {:>7.2} {:>7.2} {:>7.2} {:>8.2} {:>7} {:>9.2} {:>7.2} {:>8.2} {:>8.2} {:.4}",
                result.name,
                to_f64(result.final_value),
                to_f64(result.profit_loss),
                result.return_pct,
                result.cagr_pct,
                result.volatility_pct,
                result.sharpe_ratio,
                result.max_drawdown_pct,
                result.versus_hold_pct,
                result.trade_count,
                result.turnover_pct,
                to_f64(result.total_fees),
                to_f64(result.execution_friction),
                result.exposure_pct,
                to_f64(result.final_shares),
            )?;
        }
        writeln!(
            formatter,
            "Signals based on a session close execute at the next session open; final positions are marked to the last close."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, moving_average_actions, run};
    use crate::decimal::Decimal;
    use crate::equity::csv_data::{DailyBar, TradingDate};
    use crate::equity::{AssetClass, EquityResearchConfig, Instrument};

    fn decimal(value: &str) -> Decimal {
        Decimal::from_decimal_str(value).expect("decimal should parse")
    }

    fn config() -> EquityResearchConfig {
        EquityResearchConfig {
            instrument: Instrument {
                symbol: "TEST.L".to_string(),
                asset_class: AssetClass::Etf,
                exchange: "LSE".to_string(),
                currency: "GBP".to_string(),
            },
            initial_cash: decimal("10000"),
            commission_per_order: decimal("1"),
            commission_bps: 0,
            spread_bps: 10,
            slippage_bps: 5,
            fx_bps: 0,
            allow_fractional_shares: false,
            prices_are_adjusted: true,
            annual_trading_days: 252,
            annual_risk_free_rate_pct: 0.0,
            monthly_dca_amount: decimal("500"),
            ma_fast_window: 2,
            ma_slow_window: 3,
            breakout_entry_window: 3,
            breakout_exit_window: 2,
        }
    }

    fn bars() -> Vec<DailyBar> {
        ["100", "99", "98", "103", "104", "97"]
            .into_iter()
            .enumerate()
            .map(|(index, close)| {
                let price = decimal(close);
                DailyBar {
                    date: TradingDate {
                        year: 2026,
                        month: if index < 4 { 1 } else { 2 },
                        day: index as u32 + 1,
                    },
                    date_text: format!("2026-01-{:02}", index + 1),
                    open: price,
                    high: price + decimal("1"),
                    low: price - decimal("1"),
                    close: price,
                    volume: Some(decimal("1000")),
                }
            })
            .collect()
    }

    #[test]
    fn close_based_ma_signal_executes_on_next_open() {
        let actions = moving_average_actions(&config(), &bars());

        assert_eq!(actions[4], vec![Action::BuyAll]);
    }

    #[test]
    fn compares_four_strategies_with_costs() {
        let report = run(&config(), &bars()).expect("research should run");
        let output = report.to_string();

        assert!(output.contains("Buy and hold"));
        assert!(output.contains("Monthly DCA"));
        assert!(output.contains("MA 2/3"));
        assert!(output.contains("Breakout 3/2"));
        assert!(output.contains("next session open"));
    }
}
