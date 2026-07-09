mod ticker;

pub use ticker::PriceTick;

#[derive(Debug, Clone)]
pub enum MarketEvent {
    PriceTick(PriceTick),
}

impl MarketEvent {
    pub fn price(&self) -> f64 {
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
