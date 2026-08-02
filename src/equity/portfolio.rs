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
    #[serde(default)]
    monthly_contribution: Decimal,
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
    #[serde(default = "default_minimum_common_sessions")]
    minimum_common_sessions: usize,
    #[serde(default)]
    annual_risk_free_rate_pct: f64,
    #[serde(default = "default_rebalance_threshold_bps")]
    rebalance_threshold_bps: i64,
    #[serde(default = "default_rebalance_frequencies")]
    rebalance_frequencies: Vec<RebalanceFrequency>,
    #[serde(default)]
    walk_forward: PortfolioWalkForwardConfig,
    assets: Vec<PortfolioAssetConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct PortfolioWalkForwardConfig {
    #[serde(default = "default_walk_forward_train_sessions")]
    train_sessions: usize,
    #[serde(default = "default_walk_forward_test_sessions")]
    test_sessions: usize,
    #[serde(default = "default_walk_forward_step_sessions")]
    step_sessions: usize,
    #[serde(default = "default_walk_forward_minimum_windows")]
    minimum_windows: usize,
    #[serde(default)]
    allocations_bps: Vec<Vec<i64>>,
    #[serde(default = "default_rebalance_frequencies")]
    rebalance_frequencies: Vec<RebalanceFrequency>,
}

impl Default for PortfolioWalkForwardConfig {
    fn default() -> Self {
        Self {
            train_sessions: default_walk_forward_train_sessions(),
            test_sessions: default_walk_forward_test_sessions(),
            step_sessions: default_walk_forward_step_sessions(),
            minimum_windows: default_walk_forward_minimum_windows(),
            allocations_bps: Vec::new(),
            rebalance_frequencies: default_rebalance_frequencies(),
        }
    }
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
    external_flows: Vec<Decimal>,
    total_contributions: Decimal,
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
    versus_target_pct: f64,
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
    monthly_contribution: Decimal,
    contribution_count: usize,
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

pub struct EquityPortfolioWalkForwardReport {
    name: String,
    currency: String,
    asset_symbols: Vec<String>,
    common_session_count: usize,
    train_sessions: usize,
    test_sessions: usize,
    step_sessions: usize,
    monthly_contribution: Decimal,
    windows: Vec<PortfolioWindow>,
    results: Vec<PortfolioWalkForwardResult>,
    window_details: Vec<PortfolioWindowDetail>,
}

struct PortfolioWindow {
    train_start: String,
    train_end: String,
    test_start: String,
    test_end: String,
}

struct PortfolioWalkForwardResult {
    allocation: String,
    policy: String,
    average_train_return_pct: f64,
    average_return_pct: f64,
    worst_return_pct: f64,
    average_train_sharpe: f64,
    average_sharpe: f64,
    worst_drawdown_pct: f64,
    average_vs_first_asset_pct: f64,
    average_trades: f64,
    average_turnover_pct: f64,
}

struct PortfolioWindowDetail {
    window_number: usize,
    allocation: String,
    return_pct: f64,
    versus_first_asset_pct: f64,
    sharpe_ratio: f64,
    max_drawdown_pct: f64,
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
    validate_common_session_count(&config, sessions.len())?;

    let mut results = Vec::new();
    results.push(cash_result(&config, &sessions));
    for asset_index in 0..inputs.len() {
        results.push(simulate_single_asset(
            &config,
            &sessions,
            asset_index,
            &inputs[asset_index].config.symbol,
        ));
    }
    let target_result = simulate_target_portfolio(&config, &sessions, None);
    let target_return = target_result.return_pct;
    results.push(target_result);
    for frequency in &config.rebalance_frequencies {
        results.push(simulate_target_portfolio(
            &config,
            &sessions,
            Some(*frequency),
        ));
    }

    for result in &mut results {
        result.versus_target_pct = result.return_pct - target_return;
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
        monthly_contribution: config.monthly_contribution,
        contribution_count: contribution_count(&sessions),
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

pub fn run_walk_forward(config_path: impl AsRef<Path>) -> Result<EquityPortfolioWalkForwardReport> {
    let config_path = config_path.as_ref();
    let config = load_config(config_path)?;
    let inputs = load_assets(config_path, &config)?;
    let sessions = align_sessions(&inputs)?;
    validate_common_session_count(&config, sessions.len())?;
    let walk_forward = config.walk_forward.clone();
    let allocations = walk_forward_allocations(&config)?;

    let mut windows = Vec::new();
    let mut evaluation_ranges = Vec::new();
    let mut train_start = 0;
    while train_start + walk_forward.train_sessions + walk_forward.test_sessions <= sessions.len() {
        let train_end = train_start + walk_forward.train_sessions;
        let test_end = train_end + walk_forward.test_sessions;
        windows.push(PortfolioWindow {
            train_start: sessions[train_start].date_text.clone(),
            train_end: sessions[train_end - 1].date_text.clone(),
            test_start: sessions[train_end].date_text.clone(),
            test_end: sessions[test_end - 1].date_text.clone(),
        });
        evaluation_ranges.push((train_start..train_end, train_end..test_end));
        train_start += walk_forward.step_sessions;
    }
    if windows.len() < walk_forward.minimum_windows {
        return Err(BotError::MarketData(format!(
            "portfolio walk-forward requires at least {} held-out windows, but {} common sessions produce {}",
            walk_forward.minimum_windows,
            sessions.len(),
            windows.len()
        )));
    }

    let mut policies = vec![None];
    policies.extend(walk_forward.rebalance_frequencies.iter().copied().map(Some));
    let mut results = Vec::new();
    for allocation in &allocations {
        for frequency in &policies {
            let mut returns = Vec::new();
            let mut train_returns = Vec::new();
            let mut sharpes = Vec::new();
            let mut train_sharpes = Vec::new();
            let mut drawdowns = Vec::new();
            let mut versus_first_asset = Vec::new();
            let mut trades = 0usize;
            let mut turnover = 0.0;
            for (train_range, test_range) in &evaluation_ranges {
                let train_result = simulate_target_portfolio_with_weights(
                    &config,
                    &sessions[train_range.clone()],
                    *frequency,
                    allocation,
                );
                let window_sessions = &sessions[test_range.clone()];
                let result = simulate_target_portfolio_with_weights(
                    &config,
                    window_sessions,
                    *frequency,
                    allocation,
                );
                let benchmark =
                    simulate_single_asset(&config, window_sessions, 0, &config.assets[0].symbol);
                train_returns.push(train_result.return_pct);
                train_sharpes.push(train_result.sharpe_ratio);
                returns.push(result.return_pct);
                sharpes.push(result.sharpe_ratio);
                drawdowns.push(result.max_drawdown_pct);
                versus_first_asset.push(result.return_pct - benchmark.return_pct);
                trades += result.trade_count;
                turnover += result.turnover_pct;
            }
            let window_count = evaluation_ranges.len() as f64;
            results.push(PortfolioWalkForwardResult {
                allocation: allocation_label(allocation),
                policy: frequency.map(|value| value.to_string()).unwrap_or_else(|| {
                    if config.monthly_contribution > Decimal::ZERO {
                        "Buy-only".to_string()
                    } else {
                        "Static".to_string()
                    }
                }),
                average_train_return_pct: average(&train_returns),
                average_return_pct: average(&returns),
                worst_return_pct: returns.iter().copied().fold(f64::INFINITY, f64::min),
                average_train_sharpe: average(&train_sharpes),
                average_sharpe: average(&sharpes),
                worst_drawdown_pct: drawdowns.iter().copied().fold(0.0, f64::max),
                average_vs_first_asset_pct: average(&versus_first_asset),
                average_trades: trades as f64 / window_count,
                average_turnover_pct: turnover / window_count,
            });
        }
    }
    results.sort_by(|left, right| {
        right
            .average_sharpe
            .total_cmp(&left.average_sharpe)
            .then_with(|| right.average_return_pct.total_cmp(&left.average_return_pct))
    });
    let detail_allocations = walk_forward_detail_allocations(&config, &allocations);
    let mut window_details = Vec::new();
    for allocation in detail_allocations {
        for (window_index, (_, test_range)) in evaluation_ranges.iter().enumerate() {
            let window_sessions = &sessions[test_range.clone()];
            let result =
                simulate_target_portfolio_with_weights(&config, window_sessions, None, &allocation);
            let benchmark =
                simulate_single_asset(&config, window_sessions, 0, &config.assets[0].symbol);
            window_details.push(PortfolioWindowDetail {
                window_number: window_index + 1,
                allocation: allocation_label(&allocation),
                return_pct: result.return_pct,
                versus_first_asset_pct: result.return_pct - benchmark.return_pct,
                sharpe_ratio: result.sharpe_ratio,
                max_drawdown_pct: result.max_drawdown_pct,
            });
        }
    }

    Ok(EquityPortfolioWalkForwardReport {
        name: config.name,
        currency: config.currency,
        asset_symbols: config
            .assets
            .into_iter()
            .map(|asset| asset.symbol)
            .collect(),
        common_session_count: sessions.len(),
        train_sessions: walk_forward.train_sessions,
        test_sessions: walk_forward.test_sessions,
        step_sessions: walk_forward.step_sessions,
        monthly_contribution: config.monthly_contribution,
        windows,
        results,
        window_details,
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
        if self.monthly_contribution < Decimal::ZERO {
            return Err(BotError::Config(
                "equity portfolio monthly contribution must not be negative".to_string(),
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
            || self.minimum_common_sessions < 2
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
        self.walk_forward.validate(self.assets.len())?;

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

impl PortfolioWalkForwardConfig {
    fn validate(&self, asset_count: usize) -> Result<()> {
        if self.train_sessions == 0
            || self.test_sessions < 2
            || self.step_sessions < self.test_sessions
            || self.minimum_windows == 0
        {
            return Err(BotError::Config(
                "portfolio walk-forward requires positive training, at least two test sessions, and a step no shorter than the test window"
                    .to_string(),
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
                "portfolio walk-forward rebalance frequencies must be non-empty and unique"
                    .to_string(),
            ));
        }
        let mut unique = HashSet::new();
        for allocation in &self.allocations_bps {
            if allocation.len() != asset_count
                || allocation.iter().any(|weight| *weight < 0)
                || allocation.iter().sum::<i64>() != 10_000
                || !unique.insert(allocation.clone())
            {
                return Err(BotError::Config(
                    "each portfolio walk-forward allocation must be unique, non-negative, match the asset count, and total 10000 bps"
                        .to_string(),
                ));
            }
        }
        Ok(())
    }
}

fn validate_common_session_count(config: &PortfolioConfig, session_count: usize) -> Result<()> {
    if session_count < config.minimum_common_sessions {
        return Err(BotError::MarketData(format!(
            "portfolio requires at least {} common sessions, found {session_count}",
            config.minimum_common_sessions
        )));
    }
    Ok(())
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

fn cash_result(config: &PortfolioConfig, sessions: &[AlignedSession]) -> PortfolioResult {
    let mut state = new_state(config, sessions.len());
    record_equity(&mut state, &sessions[0].close_prices, Decimal::ZERO);
    for index in 1..sessions.len() {
        let contribution = monthly_contribution(config, sessions, index);
        add_contribution(&mut state, contribution);
        record_equity(&mut state, &sessions[index].close_prices, contribution);
    }
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
    record_equity(&mut state, &sessions[0].close_prices, Decimal::ZERO);
    for index in 1..sessions.len() {
        let contribution = monthly_contribution(config, sessions, index);
        add_contribution(&mut state, contribution);
        if contribution > Decimal::ZERO {
            let budget = state.cash;
            buy_with_budget(
                config,
                &mut state,
                asset_index,
                sessions[index].execution_prices[asset_index],
                budget,
                None,
            );
        }
        record_equity(&mut state, &sessions[index].close_prices, contribution);
    }
    finish_result(format!("Hold {symbol}"), config, state)
}

fn simulate_target_portfolio(
    config: &PortfolioConfig,
    sessions: &[AlignedSession],
    frequency: Option<RebalanceFrequency>,
) -> PortfolioResult {
    let weights = config
        .assets
        .iter()
        .map(|asset| asset.target_weight_bps)
        .collect::<Vec<_>>();
    simulate_target_portfolio_with_weights(config, sessions, frequency, &weights)
}

fn simulate_target_portfolio_with_weights(
    config: &PortfolioConfig,
    sessions: &[AlignedSession],
    frequency: Option<RebalanceFrequency>,
    weights_bps: &[i64],
) -> PortfolioResult {
    let mut state = new_state(config, sessions.len());
    for (asset_index, weight_bps) in weights_bps.iter().enumerate() {
        let budget = bps_value(config.initial_cash, *weight_bps);
        buy_with_budget(
            config,
            &mut state,
            asset_index,
            sessions[0].execution_prices[asset_index],
            budget,
            None,
        );
    }
    record_equity(&mut state, &sessions[0].close_prices, Decimal::ZERO);

    for index in 1..sessions.len() {
        let contribution = monthly_contribution(config, sessions, index);
        add_contribution(&mut state, contribution);
        if contribution > Decimal::ZERO {
            invest_cash_to_underweights(
                config,
                &mut state,
                &sessions[index - 1].close_prices,
                &sessions[index].execution_prices,
                weights_bps,
            );
        }
        if frequency.is_some_and(|frequency| {
            period_key(sessions[index - 1].date, frequency)
                != period_key(sessions[index].date, frequency)
        }) {
            rebalance(
                config,
                &mut state,
                &sessions[index - 1].close_prices,
                &sessions[index].execution_prices,
                weights_bps,
            );
        }
        record_equity(&mut state, &sessions[index].close_prices, contribution);
    }

    let name = frequency
        .map(|frequency| format!("{frequency} rebalance"))
        .unwrap_or_else(|| {
            if config.monthly_contribution > Decimal::ZERO {
                "Buy-only target".to_string()
            } else {
                "Static target".to_string()
            }
        });
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
        external_flows: Vec::with_capacity(session_count),
        total_contributions: Decimal::ZERO,
    }
}

fn rebalance(
    config: &PortfolioConfig,
    state: &mut PortfolioState,
    decision_prices: &[Decimal],
    execution_prices: &[Decimal],
    weights_bps: &[i64],
) {
    let portfolio_value = portfolio_value(state, decision_prices);
    let threshold_breached = weights_bps
        .iter()
        .enumerate()
        .any(|(index, target_weight_bps)| {
            let current_value = state.shares[index] * decision_prices[index];
            let current_weight_bps = if portfolio_value > Decimal::ZERO {
                ((current_value.micro_units() as i128 * BPS_DENOMINATOR)
                    / portfolio_value.micro_units() as i128) as i64
            } else {
                0
            };
            (current_weight_bps - *target_weight_bps).abs() >= config.rebalance_threshold_bps
        });
    if !threshold_breached {
        return;
    }

    let desired_shares = weights_bps
        .iter()
        .enumerate()
        .map(|(index, target_weight_bps)| {
            normalize_quantity(
                config,
                bps_value(portfolio_value, *target_weight_bps) / decision_prices[index],
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

fn invest_cash_to_underweights(
    config: &PortfolioConfig,
    state: &mut PortfolioState,
    decision_prices: &[Decimal],
    execution_prices: &[Decimal],
    weights_bps: &[i64],
) {
    if state.cash <= config.commission_per_order {
        return;
    }
    let total_value = portfolio_value(state, decision_prices);
    let mut deficits = weights_bps
        .iter()
        .enumerate()
        .filter_map(|(index, target_weight_bps)| {
            let target_value = bps_value(total_value, *target_weight_bps);
            let current_value = state.shares[index] * decision_prices[index];
            (target_value > current_value).then_some((index, target_value - current_value))
        })
        .collect::<Vec<_>>();
    deficits.sort_by(|left, right| right.1.cmp(&left.1));

    for (index, deficit) in deficits {
        if state.cash <= config.commission_per_order {
            break;
        }
        let quantity_limit = deficit / decision_prices[index];
        let budget = state.cash;
        buy_with_budget(
            config,
            state,
            index,
            execution_prices[index],
            budget,
            Some(quantity_limit),
        );
    }
}

fn monthly_contribution(
    config: &PortfolioConfig,
    sessions: &[AlignedSession],
    index: usize,
) -> Decimal {
    if config.monthly_contribution > Decimal::ZERO
        && (
            sessions[index - 1].date.year,
            sessions[index - 1].date.month,
        ) != (sessions[index].date.year, sessions[index].date.month)
    {
        config.monthly_contribution
    } else {
        Decimal::ZERO
    }
}

fn add_contribution(state: &mut PortfolioState, contribution: Decimal) {
    state.cash += contribution;
    state.total_contributions += contribution;
}

fn contribution_count(sessions: &[AlignedSession]) -> usize {
    sessions
        .windows(2)
        .filter(|pair| {
            (pair[0].date.year, pair[0].date.month) != (pair[1].date.year, pair[1].date.month)
        })
        .count()
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

fn record_equity(state: &mut PortfolioState, close_prices: &[Decimal], external_flow: Decimal) {
    state
        .equity_curve
        .push(portfolio_value(state, close_prices));
    state.external_flows.push(external_flow);
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
    let profit_loss = final_value - config.initial_cash - state.total_contributions;
    let adjusted_equity_curve =
        cash_flow_adjusted_curve(&state.equity_curve, &state.external_flows);
    let statistics = calculate_statistics(
        &adjusted_equity_curve,
        config.initial_cash,
        config.annual_trading_days,
        config.annual_risk_free_rate_pct,
    );
    let adjusted_final_value = adjusted_equity_curve
        .last()
        .copied()
        .unwrap_or(config.initial_cash);
    PortfolioResult {
        name,
        final_value,
        profit_loss,
        return_pct: percent_ratio(
            adjusted_final_value - config.initial_cash,
            config.initial_cash,
        ),
        cagr_pct: statistics.cagr_pct,
        volatility_pct: statistics.volatility_pct,
        sharpe_ratio: statistics.sharpe_ratio,
        max_drawdown_pct: statistics.max_drawdown_pct,
        versus_target_pct: 0.0,
        trade_count: state.trade_count,
        turnover_pct: percent_ratio(
            state.traded_value,
            config.initial_cash + state.total_contributions,
        ),
        fees: state.fees,
        friction: state.friction,
        final_cash: state.cash,
    }
}

fn cash_flow_adjusted_curve(equity_curve: &[Decimal], external_flows: &[Decimal]) -> Vec<Decimal> {
    if equity_curve.is_empty() {
        return Vec::new();
    }
    let mut adjusted = Vec::with_capacity(equity_curve.len());
    adjusted.push(equity_curve[0]);
    for index in 1..equity_curve.len() {
        let capital_before_return = equity_curve[index - 1] + external_flows[index];
        let previous_adjusted = adjusted[index - 1];
        adjusted.push(if capital_before_return > Decimal::ZERO {
            previous_adjusted * equity_curve[index] / capital_before_return
        } else {
            previous_adjusted
        });
    }
    adjusted
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
            "Monthly contribution: {} ({} contributions after the initial investment)",
            self.monthly_contribution, self.contribution_count
        )?;
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
            "strategy                 final    net_pnl    twr%   cagr%    vol% sharpe    dd% vs_target trades turnover%    fees friction final_cash"
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
                result.versus_target_pct,
                result.trade_count,
                result.turnover_pct,
                to_f64(result.fees),
                to_f64(result.friction),
                to_f64(result.final_cash),
            )?;
        }
        writeln!(
            formatter,
            "P/L excludes deposits; TWR, CAGR, volatility, Sharpe, and drawdown are cash-flow adjusted. Contributions buy underweight assets before any scheduled drift rebalance."
        )?;
        writeln!(
            formatter,
            "Rebalances are decided from the previous common-session close and execute on the next common session; histories are restricted to dates present for every asset."
        )
    }
}

impl Display for EquityPortfolioWalkForwardReport {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(formatter, "Portfolio allocation walk-forward report")?;
        writeln!(formatter, "Portfolio: {} ({})", self.name, self.currency)?;
        writeln!(
            formatter,
            "Monthly contribution per held-out simulation: {}",
            self.monthly_contribution
        )?;
        writeln!(
            formatter,
            "Assets: {} | Common sessions: {}",
            self.asset_symbols.join(" / "),
            self.common_session_count
        )?;
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
            "Ranked by average held-out Sharpe; vs_first compares return with holding {} alone.",
            self.asset_symbols[0]
        )?;
        writeln!(
            formatter,
            "allocation                    policy      train_ret% test_ret% worst_ret% train_sh test_sh worst_dd% vs_first% trades turnover%"
        )?;
        for result in &self.results {
            writeln!(
                formatter,
                "{:<29} {:<11} {:>9.2} {:>9.2} {:>10.2} {:>8.2} {:>7.2} {:>9.2} {:>9.2} {:>6.1} {:>9.2}",
                result.allocation,
                result.policy,
                result.average_train_return_pct,
                result.average_return_pct,
                result.worst_return_pct,
                result.average_train_sharpe,
                result.average_sharpe,
                result.worst_drawdown_pct,
                result.average_vs_first_asset_pct,
                result.average_trades,
                result.average_turnover_pct,
            )?;
        }
        writeln!(
            formatter,
            "Selected {} allocations by held-out window{}",
            if self.monthly_contribution > Decimal::ZERO {
                "buy-only"
            } else {
                "static"
            },
            if self.monthly_contribution > Decimal::ZERO {
                " (contribution-first)"
            } else {
                ""
            }
        )?;
        writeln!(
            formatter,
            "window allocation                      twr% vs_first% sharpe    dd%"
        )?;
        for detail in &self.window_details {
            writeln!(
                formatter,
                "{:>6} {:<29} {:>7.2} {:>9.2} {:>6.2} {:>6.2}",
                detail.window_number,
                detail.allocation,
                detail.return_pct,
                detail.versus_first_asset_pct,
                detail.sharpe_ratio,
                detail.max_drawdown_pct,
            )?;
        }
        writeln!(formatter, "Held-out window dates")?;
        for (index, window) in self.windows.iter().enumerate() {
            writeln!(
                formatter,
                " {:>2}: train {}..{} | test {}..{}",
                index + 1,
                window.train_start,
                window.train_end,
                window.test_start,
                window.test_end
            )?;
        }
        writeln!(
            formatter,
            "Each test window starts from the configured cash balance. Reviewing these results consumes the windows for research; future unseen data is still required."
        )
    }
}

fn walk_forward_allocations(config: &PortfolioConfig) -> Result<Vec<Vec<i64>>> {
    if !config.walk_forward.allocations_bps.is_empty() {
        return Ok(config.walk_forward.allocations_bps.clone());
    }
    if config.assets.len() == 2 {
        return Ok((0..=10)
            .rev()
            .map(|first_tenths| vec![first_tenths * 1_000, (10 - first_tenths) * 1_000])
            .collect());
    }
    Ok(vec![
        config
            .assets
            .iter()
            .map(|asset| asset.target_weight_bps)
            .collect(),
    ])
}

fn walk_forward_detail_allocations(
    config: &PortfolioConfig,
    allocations: &[Vec<i64>],
) -> Vec<Vec<i64>> {
    let configured = config
        .assets
        .iter()
        .map(|asset| asset.target_weight_bps)
        .collect::<Vec<_>>();
    let mut details = Vec::new();
    if config.assets.len() == 2 {
        let growth_tilt = vec![8_000, 2_000];
        if allocations.contains(&growth_tilt) {
            details.push(growth_tilt);
        }
    }
    if allocations.contains(&configured) && !details.contains(&configured) {
        details.push(configured);
    }
    if details.is_empty() {
        details.extend(allocations.iter().take(2).cloned());
    }
    details
}

fn allocation_label(weights_bps: &[i64]) -> String {
    weights_bps
        .iter()
        .map(|weight| format!("{:.0}%", *weight as f64 / 100.0))
        .collect::<Vec<_>>()
        .join(" / ")
}

fn average(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
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

fn default_minimum_common_sessions() -> usize {
    2
}

fn default_rebalance_threshold_bps() -> i64 {
    100
}

fn default_rebalance_frequencies() -> Vec<RebalanceFrequency> {
    vec![RebalanceFrequency::Quarterly, RebalanceFrequency::Yearly]
}

fn default_walk_forward_train_sessions() -> usize {
    756
}

fn default_walk_forward_test_sessions() -> usize {
    252
}

fn default_walk_forward_step_sessions() -> usize {
    252
}

fn default_walk_forward_minimum_windows() -> usize {
    1
}

#[cfg(test)]
mod tests {
    use super::{
        AssetInput, PortfolioAssetConfig, PortfolioConfig, PortfolioState,
        PortfolioWalkForwardConfig, RebalanceFrequency, align_sessions, cash_flow_adjusted_curve,
        invest_cash_to_underweights, load_config, rebalance, run, run_walk_forward,
        validate_common_session_count, walk_forward_allocations,
    };
    use crate::decimal::Decimal;
    use crate::equity::price_data::{
        DailyBar, DailyPriceData, DailyPriceKind, InputFormat, TradingDate,
    };
    use std::path::PathBuf;
    use std::{fs, process};

    fn decimal(value: &str) -> Decimal {
        Decimal::from_decimal_str(value).expect("decimal should parse")
    }

    fn config() -> PortfolioConfig {
        PortfolioConfig {
            name: "test portfolio".to_string(),
            currency: "GBP".to_string(),
            initial_cash: decimal("10000"),
            monthly_contribution: Decimal::ZERO,
            commission_per_order: decimal("1"),
            commission_bps: 0,
            spread_bps: 0,
            slippage_bps: 0,
            allow_fractional_shares: false,
            prices_are_adjusted: true,
            annual_trading_days: 252,
            minimum_common_sessions: 2,
            annual_risk_free_rate_pct: 0.0,
            rebalance_threshold_bps: 100,
            rebalance_frequencies: vec![RebalanceFrequency::Quarterly],
            walk_forward: PortfolioWalkForwardConfig::default(),
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
    fn defaults_two_asset_walk_forward_to_ten_percent_weight_steps() {
        let allocations = walk_forward_allocations(&config()).expect("grid should build");

        assert_eq!(allocations.len(), 11);
        assert_eq!(allocations[0], vec![10_000, 0]);
        assert_eq!(allocations[3], vec![7_000, 3_000]);
        assert_eq!(allocations[10], vec![0, 10_000]);
    }

    #[test]
    fn rejects_invalid_walk_forward_allocation() {
        let mut config = config();
        config.walk_forward.allocations_bps = vec![vec![8_000, 1_000]];

        let error = config.validate().expect_err("allocation should fail");

        assert!(error.to_string().contains("allocation"));
    }

    #[test]
    fn rejects_history_shorter_than_configured_minimum() {
        let mut config = config();
        config.minimum_common_sessions = 2_000;

        let error =
            validate_common_session_count(&config, 1_999).expect_err("short history should fail");

        assert!(error.to_string().contains("at least 2000 common sessions"));
    }

    #[test]
    fn parses_proxy_validation_config() {
        let config = load_config(std::path::Path::new(
            "config/equity-portfolio-proxy.example.toml",
        ))
        .expect("proxy config should parse");

        assert_eq!(config.minimum_common_sessions, 2_000);
        assert_eq!(config.walk_forward.minimum_windows, 5);
        assert_eq!(config.walk_forward.allocations_bps.len(), 5);
        assert!(config.allow_fractional_shares);
        assert!(config.prices_are_adjusted);
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
            external_flows: Vec::new(),
            total_contributions: Decimal::ZERO,
        };
        let prices = vec![decimal("100"), decimal("100")];

        rebalance(&config, &mut state, &prices, &prices, &[7000, 3000]);

        assert_eq!(state.shares, vec![decimal("70"), decimal("30")]);
        assert_eq!(state.cash, Decimal::ZERO);
        assert_eq!(state.trade_count, 2);
    }

    #[test]
    fn cash_flow_adjustment_does_not_treat_deposits_as_returns() {
        let curve = vec![decimal("10000"), decimal("10500"), decimal("11000")];
        let flows = vec![Decimal::ZERO, decimal("500"), decimal("500")];

        let adjusted = cash_flow_adjusted_curve(&curve, &flows);

        assert_eq!(adjusted, vec![decimal("10000"); 3]);
    }

    #[test]
    fn contribution_cash_buys_the_underweight_asset_without_selling() {
        let mut config = config();
        config.allow_fractional_shares = true;
        config.commission_per_order = Decimal::ZERO;
        let mut state = PortfolioState {
            cash: decimal("1000"),
            shares: vec![decimal("70"), decimal("20")],
            trade_count: 0,
            traded_value: Decimal::ZERO,
            fees: Decimal::ZERO,
            friction: Decimal::ZERO,
            equity_curve: Vec::new(),
            external_flows: Vec::new(),
            total_contributions: decimal("1000"),
        };
        let prices = vec![decimal("100"), decimal("100")];

        invest_cash_to_underweights(&config, &mut state, &prices, &prices, &[7000, 3000]);

        assert_eq!(state.shares, vec![decimal("70"), decimal("30")]);
        assert_eq!(state.cash, Decimal::ZERO);
        assert_eq!(state.trade_count, 1);
    }

    #[test]
    fn runs_repository_portfolio_fixture_end_to_end() {
        let report = run("config/equity-portfolio.example.toml")
            .expect("portfolio fixture should produce a report");
        let output = report.to_string();

        assert_eq!(report.common_session_count, 15);
        assert_eq!(report.assets.len(), 2);
        assert!(output.contains("Buy-only target"));
        assert!(output.contains("Monthly rebalance"));
        assert!(output.contains("fewer than one assumed trading year"));
    }

    #[test]
    fn runs_portfolio_walk_forward_end_to_end() {
        let directory = std::env::temp_dir().join(format!(
            "trader-portfolio-walk-forward-{}-{}",
            process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::create_dir_all(&directory).expect("temp directory should be created");
        let first = directory.join("first.csv");
        let second = directory.join("second.csv");
        let config_path = directory.join("portfolio.toml");
        let dates = (1..=10)
            .map(|day| format!("2026-01-{day:02}"))
            .collect::<Vec<_>>();
        let first_csv = std::iter::once("date,close".to_string())
            .chain(
                dates
                    .iter()
                    .enumerate()
                    .map(|(index, date)| format!("{date},{}", 100 + index)),
            )
            .collect::<Vec<_>>()
            .join("\n");
        let second_csv = std::iter::once("date,close".to_string())
            .chain(dates.iter().map(|date| format!("{date},100")))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&first, first_csv).expect("first history should write");
        fs::write(&second, second_csv).expect("second history should write");
        fs::write(
            &config_path,
            format!(
                r#"[equity_portfolio]
name = "test"
currency = "GBP"
initial_cash = 10000
commission_per_order = 0
spread_bps = 0
slippage_bps = 0
allow_fractional_shares = true
rebalance_frequencies = ["quarterly"]

[equity_portfolio.walk_forward]
train_sessions = 4
test_sessions = 2
step_sessions = 2
allocations_bps = [[10000, 0], [5000, 5000]]
rebalance_frequencies = ["quarterly"]

[[equity_portfolio.assets]]
symbol = "A"
price_file = "{}"
target_weight_bps = 7000

[[equity_portfolio.assets]]
symbol = "B"
price_file = "{}"
target_weight_bps = 3000
"#,
                first.to_string_lossy(),
                second.to_string_lossy()
            ),
        )
        .expect("config should write");

        let report = run_walk_forward(&config_path).expect("walk-forward should run");
        let output = report.to_string();

        assert_eq!(report.windows.len(), 3);
        assert_eq!(report.results.len(), 4);
        assert!(output.contains("100% / 0%"));
        assert!(output.contains("50% / 50%"));
        assert!(output.contains("3 non-overlapping held-out windows"));
        let _ = fs::remove_dir_all(directory);
    }
}
