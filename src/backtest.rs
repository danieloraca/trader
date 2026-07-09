use crate::config::Config;
use crate::decimal::Decimal;
use crate::error::{BotError, Result};
use crate::exchange::{Exchange, PaperExchange};
use crate::market::{MarketDataSource, ReplayMarketDataSource};
use crate::orders::{OrderManager, OrderStatus, Side};
use crate::portfolio::Portfolio;
use crate::risk::RiskManager;
use crate::strategy;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone)]
pub struct BacktestReport {
    pub symbol: String,
    pub event_count: usize,
    pub signal_count: usize,
    pub filled_order_count: usize,
    pub rejected_order_count: usize,
    pub buy_count: usize,
    pub sell_count: usize,
    pub initial_value_quote: Decimal,
    pub final_value_quote: Decimal,
    pub profit_loss_quote: Decimal,
    pub max_drawdown_pct: f64,
    pub final_base_balance: Decimal,
    pub final_quote_balance: Decimal,
}

pub fn run(config: &Config) -> Result<BacktestReport> {
    if config.market_data.replay_prices.is_empty() {
        return Err(BotError::Config(
            "backtest requires market_data.replay_prices".to_string(),
        ));
    }

    let mut market_data = ReplayMarketDataSource::from_prices_at_cursor(
        &config.bot.symbol,
        config.market_data.replay_prices.clone(),
        0,
    );
    let portfolio = Portfolio::new(
        &config.bot.base_currency,
        &config.bot.quote_currency,
        config.bot.paper_starting_quote_balance,
    );
    let mut exchange = PaperExchange::new(portfolio);
    let mut strategy = strategy::from_config(&config.strategy);
    let risk = RiskManager::new(config.risk.clone());
    let mut order_manager = OrderManager::new_at(1);

    let mut report = BacktestReport {
        symbol: config.bot.symbol.clone(),
        event_count: 0,
        signal_count: 0,
        filled_order_count: 0,
        rejected_order_count: 0,
        buy_count: 0,
        sell_count: 0,
        initial_value_quote: config.bot.paper_starting_quote_balance,
        final_value_quote: config.bot.paper_starting_quote_balance,
        profit_loss_quote: Decimal::ZERO,
        max_drawdown_pct: 0.0,
        final_base_balance: Decimal::ZERO,
        final_quote_balance: config.bot.paper_starting_quote_balance,
    };
    let mut peak_value = report.initial_value_quote;
    let mut last_price = None;

    while let Some(event) = market_data.next_event()? {
        report.event_count += 1;
        last_price = Some(event.price());

        let signals = strategy.on_market_event(&event);
        report.signal_count += signals.len();

        for signal in signals {
            let order_request = match risk.approve(&signal, exchange.portfolio()) {
                Ok(order_request) => order_request,
                Err(BotError::Risk(_)) => {
                    report.rejected_order_count += 1;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let side = order_request.side;
            let submitted_order = order_manager.prepare_order(order_request);
            let order = order_manager.submit_prepared_order(&mut exchange, &submitted_order)?;

            match order.status {
                OrderStatus::Filled => {
                    report.filled_order_count += 1;
                    match side {
                        Side::Buy => report.buy_count += 1,
                        Side::Sell => report.sell_count += 1,
                    }
                }
                OrderStatus::Rejected => report.rejected_order_count += 1,
                OrderStatus::Submitted | OrderStatus::Cancelled => {}
            }
        }

        let value = portfolio_value(exchange.portfolio(), event.price());
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

    let last_price = last_price.ok_or_else(|| {
        BotError::Config("backtest requires at least one replay price".to_string())
    })?;
    report.final_base_balance = exchange.portfolio().base_balance;
    report.final_quote_balance = exchange.portfolio().quote_balance;
    report.final_value_quote = portfolio_value(exchange.portfolio(), last_price);
    report.profit_loss_quote = report.final_value_quote - report.initial_value_quote;

    Ok(report)
}

fn portfolio_value(portfolio: &Portfolio, price: Decimal) -> Decimal {
    portfolio.quote_balance + (portfolio.base_balance * price)
}

impl Display for BacktestReport {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Backtest report")?;
        writeln!(f, "Symbol: {}", self.symbol)?;
        writeln!(f, "Events: {}", self.event_count)?;
        writeln!(f, "Signals: {}", self.signal_count)?;
        writeln!(f, "Filled orders: {}", self.filled_order_count)?;
        writeln!(f, "Rejected orders: {}", self.rejected_order_count)?;
        writeln!(f, "Buys: {}", self.buy_count)?;
        writeln!(f, "Sells: {}", self.sell_count)?;
        writeln!(f, "Initial value: {}", self.initial_value_quote)?;
        writeln!(f, "Final value: {}", self.final_value_quote)?;
        writeln!(f, "P/L: {}", self.profit_loss_quote)?;
        writeln!(f, "Max drawdown: {:.2}%", self.max_drawdown_pct)?;
        writeln!(f, "Final base balance: {}", self.final_base_balance)?;
        write!(f, "Final quote balance: {}", self.final_quote_balance)
    }
}

#[cfg(test)]
mod tests {
    use super::run;
    use crate::config::{
        BotConfig, Config, ExchangeConfig, MarketDataConfig, RiskConfig, StorageConfig,
        StrategyConfig, TelemetryConfig,
    };
    use crate::decimal::Decimal;

    fn decimal(value: &str) -> Decimal {
        Decimal::from_decimal_str(value).expect("decimal should parse")
    }

    fn config() -> Config {
        Config {
            bot: BotConfig {
                symbol: "BTC-USD".to_string(),
                base_currency: "BTC".to_string(),
                quote_currency: "USD".to_string(),
                paper_starting_quote_balance: decimal("10000"),
            },
            exchange: ExchangeConfig::default(),
            market_data: MarketDataConfig {
                replay_prices: vec![
                    decimal("100"),
                    decimal("101"),
                    decimal("102"),
                    decimal("101.5"),
                    decimal("99"),
                ],
                idle_sleep_ms: 1000,
                ..MarketDataConfig::default()
            },
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
    fn reports_backtest_summary() {
        let report = run(&config()).expect("backtest should run");

        assert_eq!(report.event_count, 5);
        assert_eq!(report.signal_count, 3);
        assert_eq!(report.filled_order_count, 3);
        assert_eq!(report.buy_count, 2);
        assert_eq!(report.sell_count, 1);
        assert!(report.final_value_quote > Decimal::ZERO);
    }
}
