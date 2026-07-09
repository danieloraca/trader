mod simple;

pub use simple::SimpleMomentumStrategy;

use crate::market::MarketEvent;
use crate::orders::Side;

#[derive(Debug, Clone)]
pub struct Signal {
    pub symbol: String,
    pub side: Side,
    pub quantity_base: f64,
    pub price: f64,
    pub reason: String,
}

pub trait Strategy {
    fn on_market_event(&mut self, event: &MarketEvent) -> Vec<Signal>;
}
