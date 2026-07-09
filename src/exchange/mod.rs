mod paper;

pub use paper::PaperExchange;

use crate::error::Result;
use crate::orders::{ExchangeOrder, OrderRequest};
use crate::portfolio::Portfolio;

pub trait Exchange {
    fn portfolio(&self) -> &Portfolio;
    fn place_order(&mut self, request: OrderRequest) -> Result<ExchangeOrder>;
}
