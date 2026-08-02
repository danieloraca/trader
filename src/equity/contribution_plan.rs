use super::portfolio::{PortfolioPlanningContext, load_planning_context};
use crate::decimal::Decimal;
use crate::error::{BotError, Result};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::Path;

const BPS_DENOMINATOR: i128 = 10_000;
const DECIMAL_SCALE: i64 = 1_000_000;
const MAX_PLANNED_SHARES: usize = 100_000;

#[derive(Debug, Deserialize)]
struct HoldingsFile {
    portfolio_holdings: HoldingsConfig,
}

#[derive(Debug, Deserialize)]
struct HoldingsConfig {
    available_cash: Decimal,
    assets: Vec<HoldingAsset>,
}

#[derive(Debug, Deserialize)]
struct HoldingAsset {
    symbol: String,
    shares: Decimal,
}

pub struct ContributionPlanReport {
    portfolio_name: String,
    currency: String,
    price_date: String,
    starting_cash: Decimal,
    remaining_cash: Decimal,
    starting_value: Decimal,
    estimated_final_value: Decimal,
    estimated_fees: Decimal,
    estimated_friction: Decimal,
    before_drift_bps: i64,
    after_drift_bps: i64,
    assets: Vec<PlannedAsset>,
}

struct PlannedAsset {
    symbol: String,
    target_weight_bps: i64,
    market_price: Decimal,
    starting_shares: Decimal,
    buy_shares: i64,
    final_shares: Decimal,
    before_weight_bps: i64,
    after_weight_bps: i64,
    estimated_cost: Decimal,
}

pub fn run(
    portfolio_config_path: impl AsRef<Path>,
    holdings_path: impl AsRef<Path>,
) -> Result<ContributionPlanReport> {
    let context = load_planning_context(portfolio_config_path)?;
    let holdings = load_holdings(holdings_path.as_ref(), &context)?;
    plan(&context, holdings)
}

fn load_holdings(path: &Path, context: &PortfolioPlanningContext) -> Result<HoldingsConfig> {
    let contents = fs::read_to_string(path).map_err(|error| {
        BotError::Config(format!(
            "failed to read portfolio holdings {}: {error}",
            path.to_string_lossy()
        ))
    })?;
    let file: HoldingsFile = toml::from_str(&contents).map_err(|error| {
        BotError::Config(format!("failed to parse portfolio holdings: {error}"))
    })?;
    let holdings = file.portfolio_holdings;
    if holdings.available_cash < Decimal::ZERO {
        return Err(BotError::Config(
            "portfolio holdings available cash must not be negative".to_string(),
        ));
    }
    let expected = context
        .assets
        .iter()
        .map(|asset| asset.symbol.as_str())
        .collect::<HashSet<_>>();
    let mut actual = HashSet::new();
    for asset in &holdings.assets {
        if asset.symbol.trim().is_empty()
            || asset.shares < Decimal::ZERO
            || asset.shares.micro_units() % DECIMAL_SCALE != 0
            || !actual.insert(asset.symbol.as_str())
        {
            return Err(BotError::Config(
                "portfolio holdings require unique symbols and non-negative whole-share quantities"
                    .to_string(),
            ));
        }
    }
    if actual != expected {
        return Err(BotError::Config(format!(
            "portfolio holdings symbols must exactly match: {}",
            context
                .assets
                .iter()
                .map(|asset| asset.symbol.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    Ok(holdings)
}

fn plan(
    context: &PortfolioPlanningContext,
    holdings: HoldingsConfig,
) -> Result<ContributionPlanReport> {
    let by_symbol = holdings
        .assets
        .into_iter()
        .map(|asset| (asset.symbol, asset.shares))
        .collect::<HashMap<_, _>>();
    let starting_shares = context
        .assets
        .iter()
        .map(|asset| by_symbol[&asset.symbol])
        .collect::<Vec<_>>();
    let starting_cash = holdings.available_cash;
    let starting_value = portfolio_value(context, &starting_shares, starting_cash);
    if starting_value <= Decimal::ZERO {
        return Err(BotError::Config(
            "portfolio holdings must contain cash or shares with positive value".to_string(),
        ));
    }

    let before_weights = weights_bps(context, &starting_shares, starting_cash);
    let before_drift_bps = allocation_drift_bps(context, &before_weights);
    let mut shares = starting_shares.clone();
    let mut cash = starting_cash;
    let mut buy_quantities = vec![0_i64; context.assets.len()];
    let mut estimated_fees = Decimal::ZERO;
    let mut estimated_friction = Decimal::ZERO;

    for _ in 0..MAX_PLANNED_SHARES {
        let current_weights = weights_bps(context, &shares, cash);
        let current_drift = allocation_drift_bps(context, &current_weights);
        let mut best: Option<(usize, Decimal, Decimal, i64)> = None;
        for (index, asset) in context.assets.iter().enumerate() {
            let old_quantity = Decimal::from_micro_units(buy_quantities[index] * DECIMAL_SCALE);
            let new_quantity =
                Decimal::from_micro_units((buy_quantities[index] + 1) * DECIMAL_SCALE);
            let variable_fee = bps_amount(
                asset.estimated_buy_price * new_quantity,
                context.commission_bps,
            ) - bps_amount(
                asset.estimated_buy_price * old_quantity,
                context.commission_bps,
            );
            let fixed_fee = if buy_quantities[index] == 0 {
                context.commission_per_order
            } else {
                Decimal::ZERO
            };
            let cost = asset.estimated_buy_price + variable_fee + fixed_fee;
            if cost > cash {
                continue;
            }
            let mut candidate_shares = shares.clone();
            candidate_shares[index] += Decimal::from_micro_units(DECIMAL_SCALE);
            let candidate_weights = weights_bps(context, &candidate_shares, cash - cost);
            let candidate_drift = allocation_drift_bps(context, &candidate_weights);
            if candidate_drift >= current_drift {
                continue;
            }
            if best
                .as_ref()
                .is_none_or(|(_, _, _, best_drift)| candidate_drift < *best_drift)
            {
                best = Some((index, cost, variable_fee + fixed_fee, candidate_drift));
            }
        }

        let Some((index, cost, fees, _)) = best else {
            break;
        };
        cash -= cost;
        shares[index] += Decimal::from_micro_units(DECIMAL_SCALE);
        buy_quantities[index] += 1;
        estimated_fees += fees;
        estimated_friction +=
            context.assets[index].estimated_buy_price - context.assets[index].market_price;
    }
    if buy_quantities.iter().sum::<i64>() as usize == MAX_PLANNED_SHARES {
        return Err(BotError::Config(
            "contribution plan exceeded the whole-share planning limit".to_string(),
        ));
    }

    let after_weights = weights_bps(context, &shares, cash);
    let after_drift_bps = allocation_drift_bps(context, &after_weights);
    let assets = context
        .assets
        .iter()
        .enumerate()
        .map(|(index, asset)| {
            let quantity = Decimal::from_micro_units(buy_quantities[index] * DECIMAL_SCALE);
            let variable_fees =
                bps_amount(asset.estimated_buy_price * quantity, context.commission_bps);
            let fixed_fee = if buy_quantities[index] > 0 {
                context.commission_per_order
            } else {
                Decimal::ZERO
            };
            PlannedAsset {
                symbol: asset.symbol.clone(),
                target_weight_bps: asset.target_weight_bps,
                market_price: asset.market_price,
                starting_shares: starting_shares[index],
                buy_shares: buy_quantities[index],
                final_shares: shares[index],
                before_weight_bps: before_weights[index],
                after_weight_bps: after_weights[index],
                estimated_cost: asset.estimated_buy_price * quantity + variable_fees + fixed_fee,
            }
        })
        .collect();

    Ok(ContributionPlanReport {
        portfolio_name: context.name.clone(),
        currency: context.currency.clone(),
        price_date: context.price_date.clone(),
        starting_cash,
        remaining_cash: cash,
        starting_value,
        estimated_final_value: portfolio_value(context, &shares, cash),
        estimated_fees,
        estimated_friction,
        before_drift_bps,
        after_drift_bps,
        assets,
    })
}

fn portfolio_value(
    context: &PortfolioPlanningContext,
    shares: &[Decimal],
    cash: Decimal,
) -> Decimal {
    cash + context
        .assets
        .iter()
        .zip(shares)
        .map(|(asset, quantity)| asset.market_price * *quantity)
        .fold(Decimal::ZERO, |total, value| total + value)
}

fn weights_bps(context: &PortfolioPlanningContext, shares: &[Decimal], cash: Decimal) -> Vec<i64> {
    let total = portfolio_value(context, shares, cash);
    context
        .assets
        .iter()
        .zip(shares)
        .map(|(asset, quantity)| {
            if total > Decimal::ZERO {
                ((asset.market_price.micro_units() as i128 * quantity.micro_units() as i128
                    / DECIMAL_SCALE as i128)
                    * BPS_DENOMINATOR
                    / total.micro_units() as i128) as i64
            } else {
                0
            }
        })
        .collect()
}

fn allocation_drift_bps(context: &PortfolioPlanningContext, weights: &[i64]) -> i64 {
    context
        .assets
        .iter()
        .zip(weights)
        .map(|(asset, weight)| (asset.target_weight_bps - *weight).abs())
        .sum()
}

fn bps_amount(value: Decimal, bps: i64) -> Decimal {
    Decimal::from_micro_units(
        ((value.micro_units() as i128 * bps as i128) / BPS_DENOMINATOR) as i64,
    )
}

impl Display for ContributionPlanReport {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(formatter, "Monthly portfolio contribution plan")?;
        writeln!(
            formatter,
            "Portfolio: {} ({})",
            self.portfolio_name, self.currency
        )?;
        writeln!(formatter, "Indicative price date: {}", self.price_date)?;
        writeln!(
            formatter,
            "Starting value: {} | Available cash: {} | Remaining cash: {}",
            self.starting_value, self.starting_cash, self.remaining_cash
        )?;
        writeln!(
            formatter,
            "Estimated post-trade value: {} | Fees: {} | Execution friction: {}",
            self.estimated_final_value, self.estimated_fees, self.estimated_friction
        )?;
        writeln!(
            formatter,
            "Total allocation drift: {:.2}% -> {:.2}%",
            self.before_drift_bps as f64 / 100.0,
            self.after_drift_bps as f64 / 100.0
        )?;
        writeln!(
            formatter,
            "symbol       target price       held   buy  final before% after% estimated_cost"
        )?;
        for asset in &self.assets {
            writeln!(
                formatter,
                "{:<12} {:>5.1}% {:>8} {:>10} {:>5} {:>6} {:>7.2} {:>6.2} {:>14}",
                asset.symbol,
                asset.target_weight_bps as f64 / 100.0,
                asset.market_price,
                asset.starting_shares,
                asset.buy_shares,
                asset.final_shares,
                asset.before_weight_bps as f64 / 100.0,
                asset.after_weight_bps as f64 / 100.0,
                asset.estimated_cost,
            )?;
        }
        writeln!(
            formatter,
            "Planning output only: prices are historical closes, estimates may differ from executable quotes, and no orders were placed."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{HoldingAsset, HoldingsConfig, plan, run};
    use crate::decimal::Decimal;
    use crate::equity::portfolio::{PortfolioPlanningAsset, PortfolioPlanningContext};

    fn decimal(value: &str) -> Decimal {
        Decimal::from_decimal_str(value).expect("decimal should parse")
    }

    #[test]
    fn buys_only_the_underweight_asset_when_one_share_restores_target() {
        let context = PortfolioPlanningContext {
            name: "test".to_string(),
            currency: "GBP".to_string(),
            price_date: "2026-08-01".to_string(),
            commission_per_order: Decimal::ZERO,
            commission_bps: 0,
            assets: vec![
                PortfolioPlanningAsset {
                    symbol: "EQUITY".to_string(),
                    target_weight_bps: 8_000,
                    market_price: decimal("100"),
                    estimated_buy_price: decimal("100"),
                },
                PortfolioPlanningAsset {
                    symbol: "BOND".to_string(),
                    target_weight_bps: 2_000,
                    market_price: decimal("50"),
                    estimated_buy_price: decimal("50"),
                },
            ],
        };
        let holdings = HoldingsConfig {
            available_cash: decimal("100"),
            assets: vec![
                HoldingAsset {
                    symbol: "EQUITY".to_string(),
                    shares: decimal("7"),
                },
                HoldingAsset {
                    symbol: "BOND".to_string(),
                    shares: decimal("4"),
                },
            ],
        };

        let report = plan(&context, holdings).expect("plan should succeed");

        assert_eq!(report.assets[0].buy_shares, 1);
        assert_eq!(report.assets[1].buy_shares, 0);
        assert_eq!(report.remaining_cash, Decimal::ZERO);
        assert_eq!(report.after_drift_bps, 0);
    }

    #[test]
    fn leaves_cash_when_no_whole_share_improves_drift() {
        let context = PortfolioPlanningContext {
            name: "test".to_string(),
            currency: "GBP".to_string(),
            price_date: "2026-08-01".to_string(),
            commission_per_order: decimal("3"),
            commission_bps: 0,
            assets: vec![PortfolioPlanningAsset {
                symbol: "EQUITY".to_string(),
                target_weight_bps: 10_000,
                market_price: decimal("100"),
                estimated_buy_price: decimal("100"),
            }],
        };
        let holdings = HoldingsConfig {
            available_cash: decimal("50"),
            assets: vec![HoldingAsset {
                symbol: "EQUITY".to_string(),
                shares: decimal("1"),
            }],
        };

        let report = plan(&context, holdings).expect("plan should succeed");

        assert_eq!(report.assets[0].buy_shares, 0);
        assert_eq!(report.remaining_cash, decimal("50"));
    }

    #[test]
    fn runs_repository_fixture_end_to_end() {
        let path = std::env::temp_dir().join(format!(
            "trader-portfolio-holdings-{}-{}.toml",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::write(
            &path,
            r#"[portfolio_holdings]
available_cash = 500

[[portfolio_holdings.assets]]
symbol = "GLOBAL_EQUITY"
shares = 10

[[portfolio_holdings.assets]]
symbol = "GLOBAL_BOND"
shares = 20
"#,
        )
        .expect("holdings fixture should write");

        let report = run("config/equity-portfolio.example.toml", &path)
            .expect("contribution plan should run");
        let output = report.to_string();

        assert!(output.contains("Monthly portfolio contribution plan"));
        assert!(output.contains("GLOBAL_EQUITY"));
        assert!(output.contains("no orders were placed"));
        let _ = std::fs::remove_file(path);
    }
}
