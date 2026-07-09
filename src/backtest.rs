use crate::candles::RecordedPrice;
use crate::config::Config;
use crate::decimal::Decimal;
use crate::error::{BotError, Result};
use crate::market::{MarketDataSource, ReplayMarketDataSource};
use crate::orders::{OrderRequest, Side};
use crate::portfolio::Portfolio;
use crate::risk::RiskManager;
use crate::strategy;
use rusqlite::Connection;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::Path;

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
    pub return_pct: f64,
    pub buy_and_hold_value_quote: Decimal,
    pub buy_and_hold_profit_loss_quote: Decimal,
    pub buy_and_hold_return_pct: f64,
    pub max_drawdown_pct: f64,
    pub total_fees_quote: Decimal,
    pub total_slippage_quote: Decimal,
    pub average_trade_return_pct: f64,
    pub win_count: usize,
    pub loss_count: usize,
    pub exposure_pct: f64,
    pub final_base_balance: Decimal,
    pub final_quote_balance: Decimal,
    pub trade_log_csv_path: Option<String>,
    pub trades: Vec<TradeRecord>,
}

#[derive(Debug, Clone)]
pub struct TradeRecord {
    pub event_index: usize,
    pub side: Side,
    pub quantity_base: Decimal,
    pub signal_price: Decimal,
    pub fill_price: Decimal,
    pub gross_quote_value: Decimal,
    pub fee_quote: Decimal,
    pub slippage_quote: Decimal,
    pub equity_after: Decimal,
    pub realized_pnl_quote: Decimal,
    pub reason: String,
}

#[derive(Debug, Clone)]
struct SimulatedPortfolio {
    base_balance: Decimal,
    quote_balance: Decimal,
    cost_basis_quote: Decimal,
}

pub fn run(config: &Config) -> Result<BacktestReport> {
    if config.market_data.replay_prices.is_empty() {
        return Err(BotError::Config(
            "backtest requires market_data.replay_prices".to_string(),
        ));
    }

    let market_data = ReplayMarketDataSource::from_prices_at_cursor(
        &config.bot.symbol,
        config.market_data.replay_prices.clone(),
        0,
    );

    run_with_source(config, market_data)
}

pub fn run_from_sqlite(config: &Config, sqlite_path: &str) -> Result<BacktestReport> {
    let prices = load_prices_from_sqlite(sqlite_path, &config.bot.symbol)?;
    run_from_prices(config, prices)
}

pub fn run_from_prices(
    config: &Config,
    prices: impl IntoIterator<Item = Decimal>,
) -> Result<BacktestReport> {
    let prices = prices.into_iter().collect::<Vec<_>>();
    if prices.is_empty() {
        return Err(BotError::Config(
            "backtest price source is empty".to_string(),
        ));
    }

    let market_data = ReplayMarketDataSource::from_prices_at_cursor(&config.bot.symbol, prices, 0);

    run_with_source(config, market_data)
}

fn run_with_source(
    config: &Config,
    mut market_data: impl MarketDataSource,
) -> Result<BacktestReport> {
    let mut portfolio = SimulatedPortfolio {
        base_balance: Decimal::ZERO,
        quote_balance: config.bot.paper_starting_quote_balance,
        cost_basis_quote: Decimal::ZERO,
    };
    let mut risk_portfolio = Portfolio::new(
        &config.bot.base_currency,
        &config.bot.quote_currency,
        config.bot.paper_starting_quote_balance,
    );
    let mut strategy = strategy::from_config(&config.strategy);
    let risk = RiskManager::new(config.risk.clone());

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
        trade_log_csv_path: config.backtest.trade_log_csv_path.clone(),
        trades: Vec::new(),
    };

    let mut peak_value = report.initial_value_quote;
    let mut first_price = None;
    let mut last_price = None;
    let mut exposed_events = 0_usize;
    let mut trade_return_sum_pct = 0.0;

    while let Some(event) = market_data.next_event()? {
        report.event_count += 1;
        first_price.get_or_insert(event.price());
        last_price = Some(event.price());

        if portfolio.base_balance > Decimal::ZERO {
            exposed_events += 1;
        }

        let signals = strategy.on_market_event(&event);
        report.signal_count += signals.len();

        for signal in signals {
            let order_request = match risk.approve(&signal, &risk_portfolio) {
                Ok(order_request) => order_request,
                Err(BotError::Risk(_)) => {
                    report.rejected_order_count += 1;
                    continue;
                }
                Err(error) => return Err(error),
            };

            match fill_order(
                &mut portfolio,
                &order_request,
                config.backtest.fee_bps,
                config.backtest.slippage_bps,
            ) {
                Ok(mut trade) => {
                    report.filled_order_count += 1;
                    report.total_fees_quote += trade.fee_quote;
                    report.total_slippage_quote += trade.slippage_quote;
                    trade.event_index = report.event_count;
                    trade.reason = signal.reason;
                    trade.equity_after = portfolio_value(&portfolio, event.price());

                    match order_request.side {
                        Side::Buy => report.buy_count += 1,
                        Side::Sell => {
                            report.sell_count += 1;
                            let trade_return_pct =
                                trade.realized_pnl_quote.ratio_to(trade.gross_quote_value) * 100.0;
                            trade_return_sum_pct += trade_return_pct;
                            if trade.realized_pnl_quote > Decimal::ZERO {
                                report.win_count += 1;
                            } else if trade.realized_pnl_quote < Decimal::ZERO {
                                report.loss_count += 1;
                            }
                        }
                    }

                    report.trades.push(trade);
                }
                Err(BotError::Risk(_)) => report.rejected_order_count += 1,
                Err(error) => return Err(error),
            }
        }

        risk_portfolio.base_balance = portfolio.base_balance;
        risk_portfolio.quote_balance = portfolio.quote_balance;

        let value = portfolio_value(&portfolio, event.price());
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

    let first_price = first_price.ok_or_else(|| {
        BotError::Config("backtest requires at least one replay price".to_string())
    })?;
    let last_price = last_price.expect("last price should exist when first price exists");
    report.final_base_balance = portfolio.base_balance;
    report.final_quote_balance = portfolio.quote_balance;
    report.final_value_quote = portfolio_value(&portfolio, last_price);
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

    if report.sell_count > 0 {
        report.average_trade_return_pct = trade_return_sum_pct / report.sell_count as f64;
    }
    if report.event_count > 0 {
        report.exposure_pct = exposed_events as f64 / report.event_count as f64 * 100.0;
    }

    if let Some(path) = &report.trade_log_csv_path {
        write_trade_log_csv(path, &report.trades)?;
    }

    Ok(report)
}

pub fn load_prices_from_sqlite(sqlite_path: &str, symbol: &str) -> Result<Vec<Decimal>> {
    Ok(load_recorded_prices_from_sqlite(sqlite_path, symbol)?
        .into_iter()
        .map(|recorded_price| recorded_price.price)
        .collect())
}

pub fn load_recorded_prices_from_sqlite(
    sqlite_path: &str,
    symbol: &str,
) -> Result<Vec<RecordedPrice>> {
    let connection = Connection::open_with_flags(
        sqlite_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_URI
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| {
        BotError::Storage(format!(
            "failed to open backtest sqlite source {sqlite_path}: {error}"
        ))
    })?;
    let mut statement = connection
        .prepare(
            "
            SELECT recorded_at_ms, price_micro_units
            FROM market_events
            WHERE symbol = ?1
            ORDER BY recorded_at_ms ASC, id ASC
            ",
        )
        .map_err(|error| {
            BotError::Storage(format!(
                "failed to prepare backtest sqlite market event query: {error}"
            ))
        })?;

    statement
        .query_map([symbol], |row| {
            let recorded_at_ms: i64 = row.get(0)?;
            let price_micro_units: i64 = row.get(1)?;
            Ok(RecordedPrice {
                recorded_at_ms,
                price: Decimal::from_micro_units(price_micro_units),
            })
        })
        .map_err(|error| {
            BotError::Storage(format!(
                "failed to query backtest sqlite market events: {error}"
            ))
        })?
        .map(|row| {
            row.map_err(|error| {
                BotError::Storage(format!(
                    "failed to read backtest sqlite market event: {error}"
                ))
            })
        })
        .collect()
}

fn fill_order(
    portfolio: &mut SimulatedPortfolio,
    request: &OrderRequest,
    fee_bps: i64,
    slippage_bps: i64,
) -> Result<TradeRecord> {
    let slippage = bps_value(request.limit_price, slippage_bps);
    let fill_price = match request.side {
        Side::Buy => request.limit_price + slippage,
        Side::Sell => request.limit_price - slippage,
    };
    let gross_quote_value = request.quantity_base * fill_price;
    let fee_quote = bps_value(gross_quote_value, fee_bps);
    let slippage_quote = request.quantity_base * slippage;

    let realized_pnl_quote = match request.side {
        Side::Buy => {
            let total_quote_cost = gross_quote_value + fee_quote;
            if portfolio.quote_balance < total_quote_cost {
                return Err(BotError::Risk(format!(
                    "backtest rejected: insufficient quote balance for cost {total_quote_cost}"
                )));
            }
            portfolio.quote_balance -= total_quote_cost;
            portfolio.base_balance += request.quantity_base;
            portfolio.cost_basis_quote += total_quote_cost;
            Decimal::ZERO
        }
        Side::Sell => {
            if portfolio.base_balance < request.quantity_base {
                return Err(BotError::Risk(format!(
                    "backtest rejected: sell quantity {} exceeds position {}",
                    request.quantity_base, portfolio.base_balance
                )));
            }
            let cost_basis_sold =
                (portfolio.cost_basis_quote * request.quantity_base) / portfolio.base_balance;
            let net_proceeds = gross_quote_value - fee_quote;
            portfolio.base_balance -= request.quantity_base;
            portfolio.quote_balance += net_proceeds;
            portfolio.cost_basis_quote -= cost_basis_sold;
            net_proceeds - cost_basis_sold
        }
    };

    Ok(TradeRecord {
        event_index: 0,
        side: request.side,
        quantity_base: request.quantity_base,
        signal_price: request.limit_price,
        fill_price,
        gross_quote_value,
        fee_quote,
        slippage_quote,
        equity_after: Decimal::ZERO,
        realized_pnl_quote,
        reason: String::new(),
    })
}

fn portfolio_value(portfolio: &SimulatedPortfolio, price: Decimal) -> Decimal {
    portfolio.quote_balance + (portfolio.base_balance * price)
}

fn bps_value(value: Decimal, bps: i64) -> Decimal {
    Decimal::from_micro_units(((value.micro_units() as i128 * bps as i128) / 10_000) as i64)
}

fn write_trade_log_csv(path: &str, trades: &[TradeRecord]) -> Result<()> {
    let path = Path::new(path);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| {
            BotError::Storage(format!(
                "failed to create backtest csv directory {}: {error}",
                parent.to_string_lossy()
            ))
        })?;
    }

    let mut csv = String::from(
        "event_index,side,quantity_base,signal_price,fill_price,gross_quote_value,fee_quote,slippage_quote,equity_after,realized_pnl_quote,reason\n",
    );
    for trade in trades {
        csv.push_str(&format!(
            "{},{:?},{},{},{},{},{},{},{},{},{}\n",
            trade.event_index,
            trade.side,
            trade.quantity_base,
            trade.signal_price,
            trade.fill_price,
            trade.gross_quote_value,
            trade.fee_quote,
            trade.slippage_quote,
            trade.equity_after,
            trade.realized_pnl_quote,
            csv_escape(&trade.reason)
        ));
    }

    fs::write(path, csv).map_err(|error| {
        BotError::Storage(format!(
            "failed to write backtest csv {}: {error}",
            path.to_string_lossy()
        ))
    })
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
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
        writeln!(
            f,
            "P/L: {} ({:.2}%)",
            self.profit_loss_quote, self.return_pct
        )?;
        writeln!(f, "Buy & hold value: {}", self.buy_and_hold_value_quote)?;
        writeln!(
            f,
            "Buy & hold P/L: {} ({:.2}%)",
            self.buy_and_hold_profit_loss_quote, self.buy_and_hold_return_pct
        )?;
        writeln!(f, "Max drawdown: {:.2}%", self.max_drawdown_pct)?;
        writeln!(f, "Total fees: {}", self.total_fees_quote)?;
        writeln!(f, "Total slippage: {}", self.total_slippage_quote)?;
        writeln!(
            f,
            "Average realized sell return: {:.2}%",
            self.average_trade_return_pct
        )?;
        writeln!(f, "Wins / losses: {} / {}", self.win_count, self.loss_count)?;
        writeln!(f, "Exposure: {:.2}%", self.exposure_pct)?;
        writeln!(f, "Final base balance: {}", self.final_base_balance)?;
        writeln!(f, "Final quote balance: {}", self.final_quote_balance)?;
        if let Some(path) = &self.trade_log_csv_path {
            writeln!(f, "Trade log CSV: {path}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{run, run_from_sqlite};
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

    fn db_path(name: &str) -> PathBuf {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_millis();
        std::env::temp_dir().join(format!("trader-backtest-{name}-{millis}.sqlite"))
    }

    #[test]
    fn reports_backtest_summary_with_costs_and_benchmark() {
        let report = run(&config()).expect("backtest should run");

        assert_eq!(report.event_count, 5);
        assert_eq!(report.signal_count, 3);
        assert_eq!(report.filled_order_count, 3);
        assert_eq!(report.buy_count, 2);
        assert_eq!(report.sell_count, 1);
        assert_eq!(report.trades.len(), 3);
        assert!(report.total_fees_quote > Decimal::ZERO);
        assert!(report.total_slippage_quote > Decimal::ZERO);
        assert!(report.buy_and_hold_value_quote > Decimal::ZERO);
    }

    #[test]
    fn reports_backtest_from_sqlite_market_events() {
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

        let report = run_from_sqlite(&config(), path.to_str().expect("path should be utf8"))
            .expect("backtest should run");

        assert_eq!(report.event_count, 5);
        assert_eq!(report.signal_count, 3);

        fs::remove_file(path).expect("test database should be removed");
    }
}
