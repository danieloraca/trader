mod simple;

pub use simple::SimpleMomentumStrategy;

use crate::decimal::Decimal;
use crate::market::MarketEvent;
use crate::orders::Side;

#[derive(Debug, Clone)]
pub struct Signal {
    pub symbol: String,
    pub side: Side,
    pub quantity_base: Decimal,
    pub price: Decimal,
    pub reason: String,
}

pub trait Strategy {
    fn on_market_event(&mut self, event: &MarketEvent) -> Vec<Signal>;
}
