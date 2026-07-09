use crate::error::{BotError, Result};
use crate::market::MarketEvent;
use crate::orders::Order;

pub trait Store {
    fn record_market_event(&mut self, event: &MarketEvent) -> Result<()>;
    fn record_order(&mut self, order: &Order) -> Result<()>;
}

pub struct InMemoryStore {
    market_events: Vec<MarketEvent>,
    orders: Vec<Order>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self {
            market_events: Vec::new(),
            orders: Vec::new(),
        }
    }
}

impl Store for InMemoryStore {
    fn record_market_event(&mut self, event: &MarketEvent) -> Result<()> {
        if !event.price().is_finite() {
            return Err(BotError::Storage(
                "cannot record non-finite market price".to_string(),
            ));
        }

        self.market_events.push(event.clone());
        Ok(())
    }

    fn record_order(&mut self, order: &Order) -> Result<()> {
        self.orders.push(order.clone());
        Ok(())
    }
}
