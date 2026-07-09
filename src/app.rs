use crate::config::Config;
use crate::error::Result;
use crate::exchange::{Exchange, PaperExchange};
use crate::market::{MarketDataSource, ReplayMarketDataSource};
use crate::orders::{OrderManager, OrderRequest, OrderStatus};
use crate::portfolio::Portfolio;
use crate::risk::RiskManager;
use crate::storage::{SqliteStore, Store};
use crate::strategy::{SimpleMomentumStrategy, Strategy};
use crate::telemetry;
use std::thread;
use std::time::Duration;
use tracing::{debug, info, warn};

pub struct App {
    config: Config,
    exchange: PaperExchange,
    market_data: ReplayMarketDataSource,
    order_manager: OrderManager,
    risk: RiskManager,
    run_id: String,
    strategy: SimpleMomentumStrategy,
    store: SqliteStore,
}

impl App {
    pub fn new(config: Config) -> Result<Self> {
        let store = SqliteStore::open(&config.storage.sqlite_path)?;
        let portfolio = store.load_portfolio()?.unwrap_or_else(|| {
            Portfolio::new(
                &config.bot.base_currency,
                &config.bot.quote_currency,
                config.bot.paper_starting_quote_balance,
            )
        });
        let replay_cursor = store.load_replay_cursor()?.unwrap_or(0);
        let next_order_id = store.load_next_order_id()?.unwrap_or(1);
        let market_data = ReplayMarketDataSource::from_prices_at_cursor(
            &config.bot.symbol,
            config.market_data.replay_prices.clone(),
            replay_cursor,
        );

        let exchange = PaperExchange::new(portfolio);
        let _synced_portfolio = exchange.sync_portfolio()?;
        let run_id = telemetry::new_run_id();

        info!(
            run_id = %run_id,
            symbol = %config.bot.symbol,
            replay_cursor,
            next_order_id,
            "app initialized"
        );

        Ok(Self {
            exchange,
            market_data,
            order_manager: OrderManager::new_at(next_order_id),
            risk: RiskManager::new(config.risk.clone()),
            run_id,
            strategy: SimpleMomentumStrategy::new(),
            store,
            config,
        })
    }

    pub fn run(&mut self) -> Result<()> {
        info!(
            run_id = %self.run_id,
            symbol = %self.config.bot.symbol,
            "trader started"
        );

        let idle_sleep = Duration::from_millis(self.config.market_data.idle_sleep_ms);
        let mut logged_idle = false;

        loop {
            let Some(event) = self.market_data.next_event()? else {
                if !logged_idle {
                    info!(
                        run_id = %self.run_id,
                        replay_cursor = self.market_data.cursor(),
                        idle_sleep_ms = self.config.market_data.idle_sleep_ms,
                        "market data source idle"
                    );
                    logged_idle = true;
                }

                thread::sleep(idle_sleep);
                continue;
            };

            logged_idle = false;
            debug!(
                run_id = %self.run_id,
                symbol = %event.symbol(),
                price = event.price(),
                replay_cursor = self.market_data.cursor(),
                "market event received"
            );
            self.store.record_market_event(&event)?;

            let signals = self.strategy.on_market_event(&event);
            debug!(
                run_id = %self.run_id,
                signal_count = signals.len(),
                "strategy evaluated market event"
            );

            for signal in signals {
                let portfolio = self.exchange.portfolio();
                let order_request: OrderRequest = self.risk.approve(&signal, portfolio)?;
                let transitions = self
                    .order_manager
                    .submit_order(&mut self.exchange, order_request)?;
                for order in transitions {
                    self.store.record_order(&order)?;
                    let exchange_status = if let Some(exchange_order_id) = order.exchange_order_id {
                        Some(self.exchange.order_status(exchange_order_id)?.status)
                    } else {
                        None
                    };

                    match order.status {
                        OrderStatus::Filled => {
                            info!(
                                run_id = %self.run_id,
                                bot_order_id = order.id,
                                exchange_order_id = ?order.exchange_order_id,
                                exchange_status = ?exchange_status,
                                symbol = %order.request.symbol,
                                side = ?order.request.side,
                                quantity_base = order.request.quantity_base,
                                limit_price = order.request.limit_price,
                                quote_value = order.request.quote_value(),
                                status = ?order.status,
                                "order transition recorded"
                            );
                        }
                        OrderStatus::Rejected => {
                            warn!(
                                run_id = %self.run_id,
                                bot_order_id = order.id,
                                symbol = %order.request.symbol,
                                side = ?order.request.side,
                                status = ?order.status,
                                reason = ?order.status_reason,
                                "order transition recorded"
                            );
                        }
                        _ => {
                            debug!(
                                run_id = %self.run_id,
                                bot_order_id = order.id,
                                exchange_order_id = ?order.exchange_order_id,
                                status = ?order.status,
                                "order transition recorded"
                            );
                        }
                    }
                }

                self.store
                    .save_next_order_id(self.order_manager.next_order_id())?;
            }

            self.store.save_portfolio(self.exchange.portfolio())?;
            self.store.save_replay_cursor(self.market_data.cursor())?;
        }
    }
}
