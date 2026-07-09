mod sqlite;

pub use sqlite::SqliteStore;

use crate::error::Result;
use crate::market::MarketEvent;
use crate::orders::Order;

pub trait Store {
    fn record_market_event(&mut self, event: &MarketEvent) -> Result<()>;
    fn record_order(&mut self, order: &Order) -> Result<()>;
}
