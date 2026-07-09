mod moving_average;
mod simple;

pub use moving_average::MovingAverageCrossoverStrategy;
pub use simple::SimpleMomentumStrategy;

use crate::config::{StrategyConfig, StrategyKind};
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

pub fn from_config(config: &StrategyConfig) -> Box<dyn Strategy> {
    match config.kind {
        StrategyKind::SimpleMomentum => {
            Box::new(SimpleMomentumStrategy::new(config.simple_momentum.clone()))
        }
        StrategyKind::MovingAverageCrossover => Box::new(MovingAverageCrossoverStrategy::new(
            config.moving_average_crossover.clone(),
        )),
    }
}
