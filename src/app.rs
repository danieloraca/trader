use crate::config::{Config, ExchangeKind, MarketDataKind};
use crate::error::{BotError, Result};
use crate::exchange::{Exchange, KrakenExchange, PaperExchange};
use crate::market::{KrakenTickerMarketDataSource, MarketDataSource, ReplayMarketDataSource};
use crate::orders::{OrderManager, OrderRequest, OrderStatus};
use crate::portfolio::Portfolio;
use crate::risk::RiskManager;
use crate::shutdown::{Shutdown, sleep_or_shutdown};
use crate::storage::{SqliteStore, Store};
use crate::strategy::{self, Strategy};
use crate::telemetry;
use std::time::Duration;
use tracing::{debug, info, warn};

pub struct App {
    config: Config,
    exchange: Box<dyn Exchange>,
    market_data: Box<dyn MarketDataSource>,
    order_manager: OrderManager,
    risk: RiskManager,
    run_id: String,
    strategy: Box<dyn Strategy>,
    store: SqliteStore,
}

impl App {
    pub fn new(config: Config) -> Result<Self> {
        let mut store = SqliteStore::open(&config.storage.sqlite_path)?;
        let portfolio = store.load_portfolio()?.unwrap_or_else(|| {
            Portfolio::new(
                &config.bot.base_currency,
                &config.bot.quote_currency,
                config.bot.paper_starting_quote_balance,
            )
        });
        let replay_cursor = store.load_replay_cursor()?.unwrap_or(0);
        let next_order_id = store.load_next_order_id()?.unwrap_or(1);
        let market_data: Box<dyn MarketDataSource> = match config.market_data.kind {
            MarketDataKind::Replay => Box::new(ReplayMarketDataSource::from_prices_at_cursor(
                &config.bot.symbol,
                config.market_data.replay_prices.clone(),
                replay_cursor,
            )),
            MarketDataKind::KrakenTicker => Box::new(KrakenTickerMarketDataSource::new(&config)),
        };

        let mut exchange: Box<dyn Exchange> = match config.exchange.kind {
            ExchangeKind::Paper => Box::new(PaperExchange::new(portfolio)),
            ExchangeKind::Kraken => Box::new(KrakenExchange::new(&config, portfolio)?),
        };
        let synced_portfolio = exchange.sync_portfolio()?;
        store.save_portfolio(&synced_portfolio)?;
        let run_id = telemetry::new_run_id();
        reconcile_unresolved_orders(&run_id, &mut store, exchange.as_ref())?;

        info!(
            run_id = %run_id,
            symbol = %config.bot.symbol,
            replay_cursor = ?market_data.replay_cursor(),
            next_order_id,
            "app initialized"
        );

        Ok(Self {
            exchange,
            market_data,
            order_manager: OrderManager::new_at(next_order_id),
            risk: RiskManager::new(config.risk.clone()),
            run_id,
            strategy: strategy::from_config(&config.strategy),
            store,
            config,
        })
    }

    pub fn run(&mut self, shutdown: &Shutdown) -> Result<()> {
        info!(
            run_id = %self.run_id,
            symbol = %self.config.bot.symbol,
            "trader started"
        );
        self.store.save_heartbeat(&self.run_id)?;

        let idle_sleep = Duration::from_millis(self.config.market_data.idle_sleep_ms);
        let mut logged_idle = false;

        while !shutdown.is_requested() {
            let event = match self.market_data.next_event() {
                Ok(Some(event)) => event,
                Ok(None) => {
                    self.store.save_heartbeat(&self.run_id)?;
                    if !logged_idle {
                        info!(
                            run_id = %self.run_id,
                            replay_cursor = ?self.market_data.replay_cursor(),
                            idle_sleep_ms = self.config.market_data.idle_sleep_ms,
                            "market data source idle"
                        );
                        logged_idle = true;
                    }

                    if sleep_or_shutdown(idle_sleep, shutdown) {
                        break;
                    }
                    continue;
                }
                Err(BotError::MarketData(message)) => {
                    warn!(
                        run_id = %self.run_id,
                        retry_sleep_ms = self.config.market_data.idle_sleep_ms,
                        error = %message,
                        "market data source failed; retrying"
                    );
                    self.store.save_heartbeat(&self.run_id)?;
                    if sleep_or_shutdown(idle_sleep, shutdown) {
                        break;
                    }
                    continue;
                }
                Err(error) => return Err(error),
            };

            logged_idle = false;
            debug!(
                run_id = %self.run_id,
                symbol = %event.symbol(),
                price = %event.price(),
                replay_cursor = ?self.market_data.replay_cursor(),
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
                let order_request: OrderRequest = match self.risk.approve(&signal, portfolio) {
                    Ok(order_request) => order_request,
                    Err(BotError::Risk(message)) => {
                        warn!(
                            run_id = %self.run_id,
                            symbol = %signal.symbol,
                            side = ?signal.side,
                            intent = ?signal.intent,
                            quantity_base = %signal.quantity_base,
                            price = %signal.price,
                            reason = %signal.reason,
                            rejection = %message,
                            "signal rejected by risk manager"
                        );
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                let submitted_order = self.order_manager.prepare_order(order_request);
                self.store.record_order(&submitted_order)?;
                self.store
                    .save_next_order_id(self.order_manager.next_order_id())?;
                self.log_order_transition(&submitted_order, None);

                let terminal_order = self
                    .order_manager
                    .submit_prepared_order(self.exchange.as_mut(), &submitted_order)?;
                self.store.record_order(&terminal_order)?;
                let exchange_status =
                    if let Some(exchange_order_id) = terminal_order.exchange_order_id.as_deref() {
                        Some(self.exchange.order_status(exchange_order_id)?.status)
                    } else {
                        None
                    };
                self.log_order_transition(&terminal_order, exchange_status);
            }

            self.store.save_portfolio(self.exchange.portfolio())?;
            if let Some(replay_cursor) = self.market_data.replay_cursor() {
                self.store.save_replay_cursor(replay_cursor)?;
            }
            self.store.save_heartbeat(&self.run_id)?;
        }

        self.flush_state_for_shutdown()?;
        info!(
            run_id = %self.run_id,
            replay_cursor = ?self.market_data.replay_cursor(),
            "trader shutdown complete"
        );

        Ok(())
    }

    fn flush_state_for_shutdown(&mut self) -> Result<()> {
        self.store.save_portfolio(self.exchange.portfolio())?;
        if let Some(replay_cursor) = self.market_data.replay_cursor() {
            self.store.save_replay_cursor(replay_cursor)?;
        }
        self.store.save_heartbeat(&self.run_id)
    }

    fn log_order_transition(
        &self,
        order: &crate::orders::Order,
        exchange_status: Option<OrderStatus>,
    ) {
        match order.status {
            OrderStatus::Filled => {
                info!(
                    run_id = %self.run_id,
                    bot_order_id = order.id,
                    exchange_order_id = ?order.exchange_order_id,
                    exchange_status = ?exchange_status,
                    symbol = %order.request.symbol,
                    side = ?order.request.side,
                    quantity_base = %order.request.quantity_base,
                    limit_price = %order.request.limit_price,
                    quote_value = %order.request.quote_value(),
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
}

fn reconcile_unresolved_orders(
    run_id: &str,
    store: &mut impl Store,
    exchange: &(impl Exchange + ?Sized),
) -> Result<()> {
    let unresolved_orders = store.load_unresolved_submitted_orders()?;

    if unresolved_orders.is_empty() {
        return Ok(());
    }

    info!(
        run_id,
        unresolved_order_count = unresolved_orders.len(),
        "reconciling unresolved submitted orders"
    );

    for submitted_order in unresolved_orders {
        let Some(client_order_id) = submitted_order.request.client_order_id.as_deref() else {
            warn!(
                run_id,
                bot_order_id = submitted_order.id,
                "unresolved submitted order missing client order id"
            );
            continue;
        };

        match exchange.order_status_by_client_id(client_order_id)? {
            Some(exchange_order) => {
                let reconciled_order = match exchange_order.status {
                    OrderStatus::Filled => OrderStatus::Filled,
                    OrderStatus::Rejected => OrderStatus::Rejected,
                    OrderStatus::Cancelled => OrderStatus::Cancelled,
                    OrderStatus::Submitted => {
                        warn!(
                            run_id,
                            bot_order_id = submitted_order.id,
                            client_order_id,
                            "exchange still reports submitted order as open"
                        );
                        continue;
                    }
                };

                let order = match reconciled_order {
                    OrderStatus::Filled => crate::orders::Order::filled(
                        submitted_order.id,
                        exchange_order.exchange_order_id.clone(),
                        submitted_order.request.clone(),
                    ),
                    OrderStatus::Rejected => crate::orders::Order::rejected(
                        submitted_order.id,
                        submitted_order.request.clone(),
                        "reconciled exchange rejection".to_string(),
                    ),
                    OrderStatus::Cancelled => crate::orders::Order {
                        id: submitted_order.id,
                        exchange_order_id: Some(exchange_order.exchange_order_id.clone()),
                        request: submitted_order.request.clone(),
                        status: OrderStatus::Cancelled,
                        status_reason: Some("reconciled exchange cancellation".to_string()),
                    },
                    OrderStatus::Submitted => unreachable!(),
                };

                store.record_order(&order)?;
                info!(
                    run_id,
                    bot_order_id = order.id,
                    client_order_id,
                    exchange_order_id = exchange_order.exchange_order_id,
                    status = ?order.status,
                    "unresolved order reconciled"
                );
            }
            None => {
                warn!(
                    run_id,
                    bot_order_id = submitted_order.id,
                    client_order_id,
                    "unresolved submitted order not found on exchange"
                );
            }
        }
    }

    Ok(())
}
