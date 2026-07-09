mod kraken;
mod paper;

pub use kraken::KrakenExchange;
pub use paper::PaperExchange;

use crate::error::Result;
use crate::orders::{ExchangeOrder, OrderRequest};
use crate::portfolio::Portfolio;

pub trait Exchange {
    fn portfolio(&self) -> &Portfolio;
    fn sync_portfolio(&mut self) -> Result<Portfolio>;
    fn place_order(&mut self, request: OrderRequest) -> Result<ExchangeOrder>;
    fn order_status(&self, exchange_order_id: &str) -> Result<ExchangeOrder>;
    fn order_status_by_client_id(&self, client_order_id: &str) -> Result<Option<ExchangeOrder>>;
    #[allow(dead_code)]
    fn cancel_order(&mut self, exchange_order_id: &str) -> Result<ExchangeOrder>;
}
