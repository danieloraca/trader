use crate::candles::{LiveCandleCloses, LiveCandleUpdate};
use crate::config::{Config, ExchangeKind, MarketDataKind};
use crate::error::{BotError, Result};
use crate::exchange::{Exchange, KrakenExchange, PaperExchange, PaperFuturesExchange};
use crate::market::{
    KrakenTickerMarketDataSource, MarketDataSource, MarketEvent, PriceTick, ReplayMarketDataSource,
};
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
    candle_closes: Option<LiveCandleCloses>,
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
            ExchangeKind::Paper => Box::new(PaperExchange::new_with_costs(
                portfolio,
                config.backtest.fee_bps,
                config.backtest.slippage_bps,
            )),
            ExchangeKind::PaperFutures => Box::new(PaperFuturesExchange::new(
                portfolio,
                config.exchange.paper_futures.leverage,
            )),
            ExchangeKind::Kraken => Box::new(KrakenExchange::new(&config, portfolio)?),
        };
        let synced_portfolio = exchange.sync_portfolio()?;
        store.save_portfolio(&synced_portfolio)?;
        let run_id = telemetry::new_run_id();
        reconcile_unresolved_orders(&run_id, &mut store, exchange.as_mut())?;

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
            candle_closes: config
                .strategy
                .candle_interval_seconds
                .map(LiveCandleCloses::new),
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
        let mut logged_pending_order = false;

        while !shutdown.is_requested() {
            let orders_resolved =
                reconcile_unresolved_orders(&self.run_id, &mut self.store, self.exchange.as_mut())?;
            if orders_resolved {
                logged_pending_order = false;
            } else if !logged_pending_order {
                warn!(
                    run_id = %self.run_id,
                    "order outcome is unresolved; new trades are paused"
                );
                logged_pending_order = true;
                if self.candle_closes.is_some() {
                    self.strategy = strategy::from_config(&self.config.strategy);
                }
            }

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
            let recorded_at_ms = self.store.record_market_event(&event)?;
            let strategy_event = match self.candle_closes.as_mut() {
                None => Some(event.clone()),
                Some(closes) => match closes.observe(recorded_at_ms, event.price()) {
                    LiveCandleUpdate::Pending => None,
                    LiveCandleUpdate::Closed(close) => {
                        info!(run_id = %self.run_id, close = %close, "completed strategy candle");
                        Some(MarketEvent::PriceTick(PriceTick::new(
                            event.symbol(),
                            close,
                        )))
                    }
                    LiveCandleUpdate::Gap => {
                        warn!(run_id = %self.run_id, "market data gap reset strategy warmup");
                        self.strategy = strategy::from_config(&self.config.strategy);
                        None
                    }
                },
            };
            if !orders_resolved {
                self.save_progress()?;
                continue;
            }

            let Some(strategy_event) = strategy_event else {
                self.save_progress()?;
                continue;
            };

            let signals = self
                .strategy
                .on_market_event_with_portfolio(&strategy_event, self.exchange.portfolio());
            debug!(
                run_id = %self.run_id,
                signal_count = signals.len(),
                "strategy evaluated market event"
            );

            for mut signal in signals {
                // The completed candle provides the signal; the next live tick sets the paper fill price.
                if self.candle_closes.is_some() {
                    signal.price = event.price();
                }
                let portfolio = self.exchange.portfolio();
                let order_request: OrderRequest = match self.risk.approve(&signal, portfolio) {
                    Ok(order_request) => {
                        self.store
                            .record_signal_decision(&self.run_id, &signal, None)?;
                        order_request
                    }
                    Err(BotError::Risk(message)) => {
                        self.store
                            .record_signal_decision(&self.run_id, &signal, Some(&message))?;
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
                self.log_order_transition(&submitted_order);

                let updated_order = self
                    .order_manager
                    .submit_prepared_order(self.exchange.as_mut(), &submitted_order)?;
                self.store.record_order(&updated_order)?;
                self.log_order_transition(&updated_order);
                if updated_order.status == OrderStatus::Submitted
                    && !reconcile_unresolved_orders(
                        &self.run_id,
                        &mut self.store,
                        self.exchange.as_mut(),
                    )?
                {
                    break;
                }
            }

            self.save_progress()?;
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
        self.save_progress()
    }

    fn save_progress(&mut self) -> Result<()> {
        self.store.save_portfolio(self.exchange.portfolio())?;
        if let Some(replay_cursor) = self.market_data.replay_cursor() {
            self.store.save_replay_cursor(replay_cursor)?;
        }
        self.store.save_heartbeat(&self.run_id)
    }

    fn log_order_transition(&self, order: &crate::orders::Order) {
        match order.status {
            OrderStatus::Filled => {
                info!(
                    run_id = %self.run_id,
                    bot_order_id = order.id,
                    exchange_order_id = ?order.exchange_order_id,
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
    exchange: &mut (impl Exchange + ?Sized),
) -> Result<bool> {
    let unresolved_orders = store.load_unresolved_submitted_orders()?;

    if unresolved_orders.is_empty() {
        return Ok(true);
    }

    debug!(
        run_id,
        unresolved_order_count = unresolved_orders.len(),
        "reconciling unresolved submitted orders"
    );

    let mut still_open = false;
    let mut found_terminal_order = false;
    for submitted_order in unresolved_orders {
        let Some(client_order_id) = submitted_order.request.client_order_id.as_deref() else {
            warn!(
                run_id,
                bot_order_id = submitted_order.id,
                "unresolved submitted order missing client order id"
            );
            still_open = true;
            continue;
        };

        let exchange_order =
            if let Some(exchange_order_id) = submitted_order.exchange_order_id.as_deref() {
                Some(exchange.order_status(exchange_order_id)?)
            } else {
                exchange.order_status_by_client_id(client_order_id)?
            };
        match exchange_order {
            Some(exchange_order) => {
                let reconciled_order = match exchange_order.status {
                    OrderStatus::Filled => OrderStatus::Filled,
                    OrderStatus::Rejected => OrderStatus::Rejected,
                    OrderStatus::Cancelled => OrderStatus::Cancelled,
                    OrderStatus::Submitted => {
                        debug!(
                            run_id,
                            bot_order_id = submitted_order.id,
                            client_order_id,
                            "exchange still reports submitted order as open"
                        );
                        still_open = true;
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
                found_terminal_order = true;
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
                debug!(
                    run_id,
                    bot_order_id = submitted_order.id,
                    client_order_id,
                    "unresolved submitted order not found on exchange"
                );
                still_open = true;
            }
        }
    }

    if found_terminal_order {
        let portfolio = exchange.sync_portfolio()?;
        store.save_portfolio(&portfolio)?;
    }

    Ok(!still_open)
}

#[cfg(test)]
mod tests {
    use super::{App, reconcile_unresolved_orders};
    use crate::config::Config;
    use crate::decimal::Decimal;
    use crate::error::{BotError, Result};
    use crate::exchange::Exchange;
    use crate::market::{MarketDataSource, MarketEvent, PriceTick};
    use crate::orders::OrderManager;
    use crate::orders::{ExchangeOrder, Order, OrderRequest, OrderStatus, Side};
    use crate::portfolio::Portfolio;
    use crate::risk::RiskManager;
    use crate::shutdown::Shutdown;
    use crate::storage::{SqliteStore, Store};
    use crate::strategy::{Signal, SignalIntent, Strategy};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestExchange {
        portfolio: Portfolio,
        order: Option<ExchangeOrder>,
        sync_count: usize,
    }

    struct AlwaysBuy;

    impl Strategy for AlwaysBuy {
        fn on_market_event(&mut self, event: &MarketEvent) -> Vec<Signal> {
            vec![Signal {
                symbol: event.symbol().to_string(),
                side: Side::Buy,
                intent: SignalIntent::IncreaseLong,
                quantity_base: Decimal::from_micro_units(10_000),
                price: event.price(),
                reason: "test entry".to_string(),
            }]
        }
    }

    struct SingleEventSource {
        sent: bool,
        shutdown: Shutdown,
    }

    impl MarketDataSource for SingleEventSource {
        fn next_event(&mut self) -> Result<Option<MarketEvent>> {
            if self.sent {
                self.shutdown.request();
                Ok(None)
            } else {
                self.sent = true;
                Ok(Some(MarketEvent::PriceTick(PriceTick::new(
                    "BTC-USD",
                    Decimal::from_micro_units(100_000_000),
                ))))
            }
        }
    }

    impl Exchange for TestExchange {
        fn portfolio(&self) -> &Portfolio {
            &self.portfolio
        }

        fn sync_portfolio(&mut self) -> Result<Portfolio> {
            self.sync_count += 1;
            Ok(self.portfolio.clone())
        }

        fn place_order(&mut self, _request: OrderRequest) -> Result<ExchangeOrder> {
            unreachable!()
        }

        fn order_status(&self, exchange_order_id: &str) -> Result<ExchangeOrder> {
            self.order
                .as_ref()
                .filter(|order| order.exchange_order_id == exchange_order_id)
                .cloned()
                .ok_or_else(|| BotError::Exchange("order not found".to_string()))
        }

        fn order_status_by_client_id(
            &self,
            client_order_id: &str,
        ) -> Result<Option<ExchangeOrder>> {
            Ok(self
                .order
                .as_ref()
                .filter(|order| order.client_order_id == client_order_id)
                .cloned())
        }

        fn cancel_order(&mut self, _exchange_order_id: &str) -> Result<ExchangeOrder> {
            unreachable!()
        }
    }

    #[test]
    fn keeps_trading_paused_until_an_uncertain_order_resolves_and_syncs_balances() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "trader-order-reconciliation-{}-{nonce}.sqlite",
            std::process::id()
        ));
        let mut store = SqliteStore::open(&path).expect("store should open");
        store
            .record_order(&Order::submitted(
                1,
                OrderRequest {
                    symbol: "BTC-USD".to_string(),
                    side: Side::Buy,
                    quantity_base: Decimal::from_micro_units(100_000),
                    limit_price: Decimal::from_micro_units(100_000_000),
                    client_order_id: Some("trd-1".to_string()),
                },
            ))
            .expect("pending order should record");
        let mut portfolio = Portfolio::new("BTC", "USD", Decimal::from_micro_units(900_000_000));
        portfolio.base_balance = Decimal::from_micro_units(100_000);
        let mut exchange = TestExchange {
            portfolio: portfolio.clone(),
            order: None,
            sync_count: 0,
        };

        assert!(
            !reconcile_unresolved_orders("test", &mut store, &mut exchange)
                .expect("missing order lookup should remain unresolved")
        );
        exchange.order = Some(ExchangeOrder {
            exchange_order_id: "exchange-1".to_string(),
            client_order_id: "trd-1".to_string(),
            status: OrderStatus::Submitted,
        });
        assert!(
            !reconcile_unresolved_orders("test", &mut store, &mut exchange)
                .expect("open order should remain unresolved")
        );
        assert_eq!(exchange.sync_count, 0);
        exchange.order.as_mut().expect("order should exist").status = OrderStatus::Filled;

        assert!(
            reconcile_unresolved_orders("test", &mut store, &mut exchange)
                .expect("filled order should reconcile")
        );
        assert!(
            store
                .load_unresolved_submitted_orders()
                .expect("pending orders should load")
                .is_empty()
        );
        assert_eq!(exchange.sync_count, 1);
        assert_eq!(
            store
                .load_portfolio()
                .expect("portfolio should load")
                .expect("portfolio should exist")
                .base_balance,
            portfolio.base_balance
        );

        drop(store);
        fs::remove_file(path).expect("test database should be removed");
    }

    #[test]
    fn run_does_not_submit_new_signals_while_an_order_is_pending() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "trader-pending-order-pause-{}-{nonce}.sqlite",
            std::process::id()
        ));
        let mut store = SqliteStore::open(&path).expect("store should open");
        store
            .record_order(&Order::submitted(
                1,
                OrderRequest {
                    symbol: "BTC-USD".to_string(),
                    side: Side::Buy,
                    quantity_base: Decimal::from_micro_units(100_000),
                    limit_price: Decimal::from_micro_units(100_000_000),
                    client_order_id: Some("trd-1".to_string()),
                },
            ))
            .expect("pending order should record");
        let config = Config::load_from_path("config/trader.example.toml")
            .expect("example config should load");
        let shutdown = Shutdown::new_for_test();
        let exchange = TestExchange {
            portfolio: Portfolio::new("BTC", "USD", Decimal::from_micro_units(900_000_000)),
            order: Some(ExchangeOrder {
                exchange_order_id: "exchange-1".to_string(),
                client_order_id: "trd-1".to_string(),
                status: OrderStatus::Submitted,
            }),
            sync_count: 0,
        };
        let mut app = App {
            risk: RiskManager::new(config.risk.clone()),
            config,
            exchange: Box::new(exchange),
            market_data: Box::new(SingleEventSource {
                sent: false,
                shutdown: shutdown.clone(),
            }),
            candle_closes: None,
            order_manager: OrderManager::new_at(2),
            run_id: "test".to_string(),
            strategy: Box::new(AlwaysBuy),
            store,
        };

        app.run(&shutdown).expect("run should stop after one event");
        assert_eq!(
            app.store
                .load_unresolved_submitted_orders()
                .expect("pending orders should load")
                .len(),
            1
        );

        drop(app);
        fs::remove_file(path).expect("test database should be removed");
    }
}
