mod sqlite;

pub use sqlite::SqliteStore;

use crate::error::Result;
use crate::market::MarketEvent;
use crate::orders::Order;
use crate::portfolio::Portfolio;

pub trait Store {
    fn load_portfolio(&self) -> Result<Option<Portfolio>>;
    fn save_portfolio(&mut self, portfolio: &Portfolio) -> Result<()>;
    fn load_replay_cursor(&self) -> Result<Option<usize>>;
    fn save_replay_cursor(&mut self, cursor: usize) -> Result<()>;
    fn load_next_order_id(&self) -> Result<Option<u64>>;
    fn save_next_order_id(&mut self, next_order_id: u64) -> Result<()>;
    fn save_heartbeat(&mut self, run_id: &str) -> Result<()>;
    fn load_unresolved_submitted_orders(&self) -> Result<Vec<Order>>;
    fn record_market_event(&mut self, event: &MarketEvent) -> Result<()>;
    fn record_order(&mut self, order: &Order) -> Result<()>;
}
