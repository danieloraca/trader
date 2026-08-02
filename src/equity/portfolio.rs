use super::price_data::{DailyPriceData, DailyPriceKind, TradingDate};
use super::simulation::{calculate_statistics, percent_ratio, to_f64};
use crate::decimal::Decimal;
use crate::error::{BotError, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};

const BPS_DENOMINATOR: i128 = 10_000;
const DECIMAL_SCALE: i64 = 1_000_000;

#[derive(Debug, Deserialize)]
struct PortfolioFile {
    equity_portfolio: PortfolioConfig,
}

#[derive(Debug, Deserialize)]
struct PortfolioConfig {
    name: String,
    currency: String,
    initial_cash: Decimal,
    #[serde(default = "default_commission_per_order")]
    commission_per_order: Decimal,
    #[serde(default)]
    commission_bps: i64,
    #[serde(default = "default_spread_bps")]
    spread_bps: i64,
    #[serde(default = "default_slippage_bps")]
    slippage_bps: i64,
    #[serde(default)]
    allow_fractional_shares: bool,
    #[serde(default)]
    prices_are_adjusted: bool,
    #[serde(default = "default_annual_trading_days")]
    annual_trading_days: usize,
    #[serde(default)]
    annual_risk_free_rate_pct: f64,
    #[serde(default = "default_rebalance_threshold_bps")]
    rebalance_threshold_bps: i64,
    #[serde(default = "default_rebalance_frequencies")]
    rebalance_frequencies: Vec<RebalanceFrequency>,
    assets: Vec<PortfolioAssetConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct PortfolioAssetConfig {
    symbol: String,
    price_file: String,
    target_weight_bps: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RebalanceFrequency {
    Monthly,
    Quarterly,
    Yearly,
}

impl Display for RebalanceFrequency {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Monthly => formatter.write_str("Monthly"),
            Self::Quarterly => formatter.write_str("Quarterly"),
            Self::Yearly => formatter.write_str("Yearly"),
        }
    }
}

struct AssetInput {
    config: PortfolioAssetConfig,
    path: PathBuf,
    data: DailyPriceData,
}

#[derive(Debug)]
struct AlignedSession {
    date: TradingDate,
    date_text: String,
    execution_prices: Vec<Decimal>,
    close_prices: Vec<Decimal>,
}

#[derive(Debug)]
struct PortfolioState {
    cash: Decimal,
    shares: Vec<Decimal>,
    trade_count: usize,
    traded_value: Decimal,
    fees: Decimal,
    friction: Decimal,
    equity_curve: Vec<Decimal>,
}

#[derive(Debug)]
struct PortfolioResult {
    name: String,
    final_value: Decimal,
    profit_loss: Decimal,
    return_pct: f64,
    cagr_pct: f64,
    volatility_pct: f64,
    sharpe_ratio: f64,
    max_drawdown_pct: f64,
    versus_static_pct: f64,
    trade_count: usize,
    turnover_pct: f64,
    fees: Decimal,
    friction: Decimal,
    final_cash: Decimal,
}

pub struct EquityPortfolioReport {
    name: String,
    currency: String,
    initial_cash: Decimal,
    first_date: String,
    last_date: String,
    common_session_count: usize,
    annual_trading_days: usize,
    commission_per_order: Decimal,
    commission_bps: i64,
    spread_bps: i64,
    slippage_bps: i64,
    allow_fractional_shares: bool,
    prices_are_adjusted: bool,
    rebalance_threshold_bps: i64,
    assets: Vec<AssetSummary>,
    results: Vec<PortfolioResult>,
}

struct AssetSummary {
    symbol: String,
    target_weight_bps: i64,
    source_session_count: usize,
    skipped_missing_price_rows: usize,
    price_column: String,
    path: PathBuf,
}

pub fn run(config_path: impl AsRef<Path>) -> Result<EquityPortfolioReport> {
    let config_path = config_path.as_ref();
    let config = load_config(config_path)?;
    let inputs = load_assets(config_path, &config)?;
    let sessions = align_sessions(&inputs)?;

    let mut results = Vec::new();
    results.push(cash_result(&config, sessions.len()));
    for asset_index in 0..inputs.len() {
        results.push(simulate_single_asset(
            &config,
            &sessions,
            asset_index,
            &inputs[asset_index].config.symbol,
        ));
    }
    results.push(simulate_target_portfolio(&config, &sessions, None));
    for frequency in &config.rebalance_frequencies {
        results.push(simulate_target_portfolio(
            &config,
            &sessions,
            Some(*frequency),
        ));
    }

    let static_return = results
        .iter()
        .find(|result| result.name == "Static target")
        .expect("static portfolio result should exist")
        .return_pct;
    for result in &mut results {
        result.versus_static_pct = result.return_pct - static_return;
    }
    results.sort_by(|left, right| right.return_pct.total_cmp(&left.return_pct));

    let assets = inputs
        .into_iter()
        .map(|input| AssetSummary {
            symbol: input.config.symbol,
            target_weight_bps: input.config.target_weight_bps,
            source_session_count: input.data.bars.len(),
            skipped_missing_price_rows: input.data.skipped_missing_price_rows,
            price_column: input.data.price_column,
            path: input.path,
        })
        .collect();

    Ok(EquityPortfolioReport {
        name: config.name,
        currency: config.currency,
        initial_cash: config.initial_cash,
        first_date: sessions[0].date_text.clone(),
        last_date: sessions[sessions.len() - 1].date_text.clone(),
        common_session_count: sessions.len(),
        annual_trading_days: config.annual_trading_days,
        commission_per_order: config.commission_per_order,
        commission_bps: config.commission_bps,
        spread_bps: config.spread_bps,
        slippage_bps: config.slippage_bps,
        allow_fractional_shares: config.allow_fractional_shares,
        prices_are_adjusted: config.prices_are_adjusted,
        rebalance_threshold_bps: config.rebalance_threshold_bps,
        assets,
        results,
    })
}

fn load_config(path: &Path) -> Result<PortfolioConfig> {
    let contents = fs::read_to_string(path).map_err(|error| {
        BotError::Config(format!(
            "failed to read equity portfolio config {}: {error}",
            path.to_string_lossy()
        ))
    })?;
    let file: PortfolioFile = toml::from_str(&contents).map_err(|error| {
        BotError::Config(format!("failed to parse equity portfolio config: {error}"))
    })?;
    file.equity_portfolio.validate()?;
    Ok(file.equity_portfolio)
}

impl PortfolioConfig {
    fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() || self.currency.trim().is_empty() {
            return Err(BotError::Config(
                "equity portfolio name and currency must not be empty".to_string(),
            ));
        }
        if self.initial_cash <= Decimal::ZERO || self.commission_per_order < Decimal::ZERO {
            return Err(BotError::Config(
                "equity portfolio initial cash must be positive and fixed commission non-negative"
                    .to_string(),
            ));
        }
        if [
            self.commission_bps,
            self.spread_bps,
            self.slippage_bps,
            self.rebalance_threshold_bps,
        ]
        .into_iter()
        .any(|value| value < 0)
            || self.rebalance_threshold_bps > 10_000
        {
            return Err(BotError::Config(
                "equity portfolio costs and rebalance threshold must be valid non-negative basis points"
                    .to_string(),
            ));
        }
        if self.annual_trading_days == 0
            || !self.annual_risk_free_rate_pct.is_finite()
            || self.annual_risk_free_rate_pct <= -100.0
        {
            return Err(BotError::Config(
                "equity portfolio annual assumptions are invalid".to_string(),
            ));
        }
        if self.assets.len() < 2 {
            return Err(BotError::Config(
                "equity portfolio research requires at least two assets".to_string(),
            ));
        }
        if self.rebalance_frequencies.is_empty()
            || self
                .rebalance_frequencies
                .iter()
                .collect::<HashSet<_>>()
                .len()
                != self.rebalance_frequencies.len()
        {
            return Err(BotError::Config(
                "equity portfolio rebalance frequencies must be non-empty and unique".to_string(),
            ));
        }

        let mut symbols = HashSet::new();
        let mut paths = HashSet::new();
        let mut weight_total = 0_i64;
        for asset in &self.assets {
            if asset.symbol.trim().is_empty()
                || asset.price_file.trim().is_empty()
                || asset.target_weight_bps <= 0
                || !symbols.insert(asset.symbol.as_str())
                || !paths.insert(asset.price_file.as_str())
            {
                return Err(BotError::Config(
                    "portfolio assets need unique non-empty symbols/files and positive weights"
                        .to_string(),
                ));
            }
            weight_total += asset.target_weight_bps;
        }
        if weight_total != 10_000 {
            return Err(BotError::Config(format!(
                "portfolio target weights must total 10000 bps, got {weight_total}"
            )));
        }
        Ok(())
    }
}

fn load_assets(config_path: &Path, config: &PortfolioConfig) -> Result<Vec<AssetInput>> {
    let base = config_path.parent().unwrap_or_else(|| Path::new("."));
    let inputs = config
        .assets
        .iter()
        .map(|asset| {
            let configured_path = Path::new(&asset.price_file);
            let path = if configured_path.is_absolute() {
                configured_path.to_path_buf()
            } else {
                base.join(configured_path)
            };
            let data = super::price_data::load(&path).map_err(|error| {
                BotError::MarketData(format!(
                    "failed to load portfolio asset {} from {}: {error}",
                    asset.symbol,
                    path.to_string_lossy()
                ))
            })?;
            Ok(AssetInput {
                config: asset.clone(),
                path,
                data,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let data_kind = inputs[0].data.kind;
    if inputs.iter().any(|input| input.data.kind != data_kind) {
        return Err(BotError::MarketData(
            "portfolio assets must all use the same price shape so executions occur consistently at opens or closes"
                .to_string(),
        ));
    }
    Ok(inputs)
}

fn align_sessions(inputs: &[AssetInput]) -> Result<Vec<AlignedSession>> {
    let indexes = inputs
        .iter()
        .map(|input| {
            input
                .data
                .bars
                .iter()
                .enumerate()
                .map(|(index, bar)| (bar.date, index))
                .collect::<BTreeMap<_, _>>()
        })
        .collect::<Vec<_>>();

    let mut sessions = Vec::new();
    for first_bar in &inputs[0].data.bars {
        let Some(asset_indexes) = indexes
            .iter()
            .map(|index| index.get(&first_bar.date).copied())
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let mut execution_prices = Vec::with_capacity(inputs.len());
        let mut close_prices = Vec::with_capacity(inputs.len());
        for (asset_index, bar_index) in asset_indexes.into_iter().enumerate() {
            let bar = &inputs[asset_index].data.bars[bar_index];
            execution_prices.push(match inputs[asset_index].data.kind {
                DailyPriceKind::Ohlcv => bar.open,
                DailyPriceKind::CloseOnly => bar.close,
            });
            close_prices.push(bar.close);
        }
        sessions.push(AlignedSession {
            date: first_bar.date,
            date_text: first_bar.date_text.clone(),
            execution_prices,
            close_prices,
        });
    }

    if sessions.len() < 2 {
        return Err(BotError::MarketData(
            "portfolio price histories need at least two common trading dates".to_string(),
        ));
    }
    Ok(sessions)
}

fn cash_result(config: &PortfolioConfig, session_count: usize) -> PortfolioResult {
    let state = PortfolioState {
        cash: config.initial_cash,
        shares: vec![Decimal::ZERO; config.assets.len()],
        trade_count: 0,
        traded_value: Decimal::ZERO,
        fees: Decimal::ZERO,
        friction: Decimal::ZERO,
        equity_curve: vec![config.initial_cash; session_count],
    };
    finish_result("Cash".to_string(), config, state)
}

fn simulate_single_asset(
    config: &PortfolioConfig,
    sessions: &[AlignedSession],
    asset_index: usize,
    symbol: &str,
) -> PortfolioResult {
    let mut state = new_state(config, sessions.len());
    let budget = state.cash;
    buy_with_budget(
        config,
        &mut state,
        asset_index,
        sessions[0].execution_prices[asset_index],
        budget,
        None,
    );
    for session in sessions {
        record_equity(&mut state, &session.close_prices);
    }
    finish_result(format!("Hold {symbol}"), config, state)
}

fn simulate_target_portfolio(
    config: &PortfolioConfig,
    sessions: &[AlignedSession],
    frequency: Option<RebalanceFrequency>,
) -> PortfolioResult {
    let mut state = new_state(config, sessions.len());
    for (asset_index, asset) in config.assets.iter().enumerate() {
        let budget = bps_value(config.initial_cash, asset.target_weight_bps);
        buy_with_budget(
            config,
            &mut state,
            asset_index,
            sessions[0].execution_prices[asset_index],
            budget,
            None,
        );
    }
    record_equity(&mut state, &sessions[0].close_prices);

    for index in 1..sessions.len() {
        if frequency.is_some_and(|frequency| {
            period_key(sessions[index - 1].date, frequency)
                != period_key(sessions[index].date, frequency)
        }) {
            rebalance(
                config,
                &mut state,
                &sessions[index - 1].close_prices,
                &sessions[index].execution_prices,
            );
        }
        record_equity(&mut state, &sessions[index].close_prices);
    }

    let name = frequency
        .map(|frequency| format!("{frequency} rebalance"))
        .unwrap_or_else(|| "Static target".to_string());
    finish_result(name, config, state)
}

fn new_state(config: &PortfolioConfig, session_count: usize) -> PortfolioState {
    PortfolioState {
        cash: config.initial_cash,
        shares: vec![Decimal::ZERO; config.assets.len()],
        trade_count: 0,
        traded_value: Decimal::ZERO,
        fees: Decimal::ZERO,
        friction: Decimal::ZERO,
        equity_curve: Vec::with_capacity(session_count),
    }
}

fn rebalance(
    config: &PortfolioConfig,
    state: &mut PortfolioState,
    decision_prices: &[Decimal],
    execution_prices: &[Decimal],
) {
    let portfolio_value = portfolio_value(state, decision_prices);
    let threshold_breached = config.assets.iter().enumerate().any(|(index, asset)| {
        let current_value = state.shares[index] * decision_prices[index];
        let current_weight_bps = if portfolio_value > Decimal::ZERO {
            ((current_value.micro_units() as i128 * BPS_DENOMINATOR)
                / portfolio_value.micro_units() as i128) as i64
        } else {
            0
        };
        (current_weight_bps - asset.target_weight_bps).abs() >= config.rebalance_threshold_bps
    });
    if !threshold_breached {
        return;
    }

    let desired_shares = config
        .assets
        .iter()
        .enumerate()
        .map(|(index, asset)| {
            normalize_quantity(
                config,
                bps_value(portfolio_value, asset.target_weight_bps) / decision_prices[index],
            )
        })
        .collect::<Vec<_>>();

    for (index, desired) in desired_shares.iter().enumerate() {
        if state.shares[index] > *desired {
            let quantity = state.shares[index] - *desired;
            sell_quantity(config, state, index, execution_prices[index], quantity);
        }
    }
    for (index, desired) in desired_shares.iter().enumerate() {
        if state.shares[index] < *desired {
            let quantity = *desired - state.shares[index];
            let budget = state.cash;
            buy_with_budget(
                config,
                state,
                index,
                execution_prices[index],
                budget,
                Some(quantity),
            );
        }
    }
}

fn buy_with_budget(
    config: &PortfolioConfig,
    state: &mut PortfolioState,
    asset_index: usize,
    market_price: Decimal,
    budget: Decimal,
    quantity_limit: Option<Decimal>,
) {
    let budget = budget.min(state.cash);
    if budget <= config.commission_per_order {
        return;
    }
    let execution_price = adjusted_execution_price(config, market_price, true);
    let unit_variable_fee = bps_amount(execution_price, config.commission_bps);
    let available = budget - config.commission_per_order;
    let mut quantity = available / (execution_price + unit_variable_fee);
    if let Some(limit) = quantity_limit {
        quantity = quantity.min(limit);
    }
    quantity = normalize_quantity(config, quantity);
    if quantity <= Decimal::ZERO {
        return;
    }

    let gross = execution_price * quantity;
    let fees = config.commission_per_order + bps_amount(gross, config.commission_bps);
    let total = gross + fees;
    if total > state.cash || total > budget {
        return;
    }
    state.cash -= total;
    state.shares[asset_index] += quantity;
    state.trade_count += 1;
    state.traded_value += gross;
    state.fees += fees;
    state.friction += (execution_price - market_price) * quantity;
}

fn sell_quantity(
    config: &PortfolioConfig,
    state: &mut PortfolioState,
    asset_index: usize,
    market_price: Decimal,
    quantity: Decimal,
) {
    let quantity = normalize_quantity(config, quantity.min(state.shares[asset_index]));
    if quantity <= Decimal::ZERO {
        return;
    }
    let execution_price = adjusted_execution_price(config, market_price, false);
    let gross = execution_price * quantity;
    let fees = config.commission_per_order + bps_amount(gross, config.commission_bps);
    if gross <= fees {
        return;
    }
    state.cash += gross - fees;
    state.shares[asset_index] -= quantity;
    state.trade_count += 1;
    state.traded_value += gross;
    state.fees += fees;
    state.friction += (market_price - execution_price) * quantity;
}

fn normalize_quantity(config: &PortfolioConfig, quantity: Decimal) -> Decimal {
    if config.allow_fractional_shares {
        quantity
    } else {
        Decimal::from_micro_units((quantity.micro_units() / DECIMAL_SCALE) * DECIMAL_SCALE)
    }
}

fn adjusted_execution_price(config: &PortfolioConfig, market_price: Decimal, buy: bool) -> Decimal {
    let half_spread_plus_slippage = config.spread_bps as i128 + config.slippage_bps as i128 * 2;
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

fn bps_value(value: Decimal, bps: i64) -> Decimal {
    bps_amount(value, bps)
}

fn period_key(date: TradingDate, frequency: RebalanceFrequency) -> (i32, u32) {
    match frequency {
        RebalanceFrequency::Monthly => (date.year, date.month),
        RebalanceFrequency::Quarterly => (date.year, (date.month - 1) / 3 + 1),
        RebalanceFrequency::Yearly => (date.year, 1),
    }
}

fn record_equity(state: &mut PortfolioState, close_prices: &[Decimal]) {
    state
        .equity_curve
        .push(portfolio_value(state, close_prices));
}

fn portfolio_value(state: &PortfolioState, prices: &[Decimal]) -> Decimal {
    state.cash
        + state
            .shares
            .iter()
            .zip(prices)
            .map(|(quantity, price)| *quantity * *price)
            .fold(Decimal::ZERO, |total, value| total + value)
}

fn finish_result(name: String, config: &PortfolioConfig, state: PortfolioState) -> PortfolioResult {
    let final_value = state
        .equity_curve
        .last()
        .copied()
        .unwrap_or(config.initial_cash);
    let profit_loss = final_value - config.initial_cash;
    let statistics = calculate_statistics(
        &state.equity_curve,
        config.initial_cash,
        config.annual_trading_days,
        config.annual_risk_free_rate_pct,
    );
    PortfolioResult {
        name,
        final_value,
        profit_loss,
        return_pct: percent_ratio(profit_loss, config.initial_cash),
        cagr_pct: statistics.cagr_pct,
        volatility_pct: statistics.volatility_pct,
        sharpe_ratio: statistics.sharpe_ratio,
        max_drawdown_pct: statistics.max_drawdown_pct,
        versus_static_pct: 0.0,
        trade_count: state.trade_count,
        turnover_pct: percent_ratio(state.traded_value, config.initial_cash),
        fees: state.fees,
        friction: state.friction,
        final_cash: state.cash,
    }
}

impl Display for EquityPortfolioReport {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(formatter, "Multi-ETF portfolio research report")?;
        writeln!(formatter, "Portfolio: {} ({})", self.name, self.currency)?;
        writeln!(
            formatter,
            "Common sessions: {} ({} to {})",
            self.common_session_count, self.first_date, self.last_date
        )?;
        writeln!(formatter, "Initial cash: {}", self.initial_cash)?;
        writeln!(
            formatter,
            "Costs: {} fixed/order, {} commission bps, {} spread bps, {} slippage bps",
            self.commission_per_order, self.commission_bps, self.spread_bps, self.slippage_bps
        )?;
        writeln!(
            formatter,
            "Shares: {} | Prices: {} | Rebalance drift threshold: {:.2}%",
            if self.allow_fractional_shares {
                "fractional"
            } else {
                "whole only"
            },
            if self.prices_are_adjusted {
                "declared split/dividend adjusted"
            } else {
                "source adjustment unknown (price return only)"
            },
            self.rebalance_threshold_bps as f64 / 100.0
        )?;
        if self.common_session_count < self.annual_trading_days {
            writeln!(
                formatter,
                "Warning: fewer than one assumed trading year; CAGR, volatility, and Sharpe are unstable."
            )?;
        }
        writeln!(formatter, "Assets")?;
        writeln!(
            formatter,
            "symbol       weight source common skipped valuation column / file"
        )?;
        for asset in &self.assets {
            writeln!(
                formatter,
                "{:<10} {:>6.2}% {:>6} {:>6} {:>7} {} / {}",
                asset.symbol,
                asset.target_weight_bps as f64 / 100.0,
                asset.source_session_count,
                self.common_session_count,
                asset.skipped_missing_price_rows,
                asset.price_column,
                asset.path.to_string_lossy()
            )?;
        }
        writeln!(
            formatter,
            "strategy                 final        pnl    ret%   cagr%    vol% sharpe    dd% vs_static trades turnover%    fees friction final_cash"
        )?;
        for result in &self.results {
            writeln!(
                formatter,
                "{:<22} {:>11.2} {:>10.2} {:>7.2} {:>7.2} {:>7.2} {:>6.2} {:>6.2} {:>9.2} {:>6} {:>9.2} {:>7.2} {:>8.2} {:>10.2}",
                result.name,
                to_f64(result.final_value),
                to_f64(result.profit_loss),
                result.return_pct,
                result.cagr_pct,
                result.volatility_pct,
                result.sharpe_ratio,
                result.max_drawdown_pct,
                result.versus_static_pct,
                result.trade_count,
                result.turnover_pct,
                to_f64(result.fees),
                to_f64(result.friction),
                to_f64(result.final_cash),
            )?;
        }
        writeln!(
            formatter,
            "Rebalances are decided from the previous common-session close and execute on the next common session; histories are restricted to dates present for every asset."
        )
    }
}

fn default_commission_per_order() -> Decimal {
    Decimal::from_micro_units(1_000_000)
}

fn default_spread_bps() -> i64 {
    10
}

fn default_slippage_bps() -> i64 {
    5
}

fn default_annual_trading_days() -> usize {
    252
}

fn default_rebalance_threshold_bps() -> i64 {
    100
}

fn default_rebalance_frequencies() -> Vec<RebalanceFrequency> {
    vec![RebalanceFrequency::Quarterly, RebalanceFrequency::Yearly]
}

#[cfg(test)]
mod tests {
    use super::{
        AssetInput, PortfolioAssetConfig, PortfolioConfig, PortfolioState, RebalanceFrequency,
        align_sessions, rebalance, run,
    };
    use crate::decimal::Decimal;
    use crate::equity::price_data::{
        DailyBar, DailyPriceData, DailyPriceKind, InputFormat, TradingDate,
    };
    use std::path::PathBuf;

    fn decimal(value: &str) -> Decimal {
        Decimal::from_decimal_str(value).expect("decimal should parse")
    }

    fn config() -> PortfolioConfig {
        PortfolioConfig {
            name: "test portfolio".to_string(),
            currency: "GBP".to_string(),
            initial_cash: decimal("10000"),
            commission_per_order: decimal("1"),
            commission_bps: 0,
            spread_bps: 0,
            slippage_bps: 0,
            allow_fractional_shares: false,
            prices_are_adjusted: true,
            annual_trading_days: 252,
            annual_risk_free_rate_pct: 0.0,
            rebalance_threshold_bps: 100,
            rebalance_frequencies: vec![RebalanceFrequency::Quarterly],
            assets: vec![
                PortfolioAssetConfig {
                    symbol: "A".to_string(),
                    price_file: "a.csv".to_string(),
                    target_weight_bps: 7000,
                },
                PortfolioAssetConfig {
                    symbol: "B".to_string(),
                    price_file: "b.csv".to_string(),
                    target_weight_bps: 3000,
                },
            ],
        }
    }

    fn input(symbol: &str, days: &[u32]) -> AssetInput {
        let bars = days
            .iter()
            .map(|day| {
                let price = decimal(&format!("10{day}"));
                DailyBar {
                    date: TradingDate {
                        year: 2026,
                        month: 1,
                        day: *day,
                    },
                    date_text: format!("2026-01-{day:02}"),
                    open: price,
                    high: price,
                    low: price,
                    close: price,
                    volume: None,
                }
            })
            .collect();
        AssetInput {
            config: PortfolioAssetConfig {
                symbol: symbol.to_string(),
                price_file: format!("{symbol}.csv"),
                target_weight_bps: 5000,
            },
            path: PathBuf::from(format!("{symbol}.csv")),
            data: DailyPriceData {
                bars,
                kind: DailyPriceKind::CloseOnly,
                input_format: InputFormat::Csv,
                price_column: "close".to_string(),
                skipped_missing_price_rows: 0,
            },
        }
    }

    #[test]
    fn rejects_target_weights_that_do_not_total_one_hundred_percent() {
        let mut config = config();
        config.assets[1].target_weight_bps = 2000;

        let error = config.validate().expect_err("weights should fail");

        assert!(error.to_string().contains("must total 10000 bps"));
    }

    #[test]
    fn aligns_only_dates_present_for_every_asset() {
        let inputs = vec![input("A", &[1, 2, 3]), input("B", &[2, 3, 4])];

        let sessions = align_sessions(&inputs).expect("histories should align");

        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].date_text, "2026-01-02");
        assert_eq!(sessions[1].date_text, "2026-01-03");
    }

    #[test]
    fn threshold_breach_rebalances_both_sides_to_target() {
        let mut config = config();
        config.commission_per_order = Decimal::ZERO;
        let mut state = PortfolioState {
            cash: Decimal::ZERO,
            shares: vec![decimal("80"), decimal("20")],
            trade_count: 0,
            traded_value: Decimal::ZERO,
            fees: Decimal::ZERO,
            friction: Decimal::ZERO,
            equity_curve: Vec::new(),
        };
        let prices = vec![decimal("100"), decimal("100")];

        rebalance(&config, &mut state, &prices, &prices);

        assert_eq!(state.shares, vec![decimal("70"), decimal("30")]);
        assert_eq!(state.cash, Decimal::ZERO);
        assert_eq!(state.trade_count, 2);
    }

    #[test]
    fn runs_repository_portfolio_fixture_end_to_end() {
        let report = run("config/equity-portfolio.example.toml")
            .expect("portfolio fixture should produce a report");
        let output = report.to_string();

        assert_eq!(report.common_session_count, 15);
        assert_eq!(report.assets.len(), 2);
        assert!(output.contains("Static target"));
        assert!(output.contains("Monthly rebalance"));
        assert!(output.contains("fewer than one assumed trading year"));
    }
}
