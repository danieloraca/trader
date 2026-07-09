mod replay;
mod ticker;

pub use replay::ReplayMarketDataSource;
pub use ticker::PriceTick;

use crate::decimal::Decimal;
use crate::error::Result;

#[derive(Debug, Clone)]
pub enum MarketEvent {
    PriceTick(PriceTick),
}

pub trait MarketDataSource {
    fn next_event(&mut self) -> Result<Option<MarketEvent>>;
}

impl MarketEvent {
    pub fn price(&self) -> Decimal {
        match self {
            Self::PriceTick(tick) => tick.price,
        }
    }

    pub fn symbol(&self) -> &str {
        match self {
            Self::PriceTick(tick) => &tick.symbol,
        }
    }
}
