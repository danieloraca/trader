mod breakout;
mod moving_average;
mod rsi;
mod simple;

pub use breakout::BreakoutStrategy;
pub use moving_average::MovingAverageCrossoverStrategy;
pub use rsi::RsiMeanReversionStrategy;
pub use simple::SimpleMomentumStrategy;

use crate::config::{StrategyConfig, StrategyDirection, StrategyKind};
use crate::decimal::Decimal;
use crate::market::MarketEvent;
use crate::orders::Side;

#[derive(Debug, Clone)]
pub struct Signal {
    pub symbol: String,
    pub side: Side,
    pub intent: SignalIntent,
    pub quantity_base: Decimal,
    pub price: Decimal,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalIntent {
    IncreaseLong,
    DecreaseLong,
    IncreaseShort,
    DecreaseShort,
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
        StrategyKind::RsiMeanReversion => Box::new(RsiMeanReversionStrategy::new(
            config.rsi_mean_reversion.clone(),
        )),
        StrategyKind::Breakout => Box::new(BreakoutStrategy::new(config.breakout.clone())),
    }
}

pub fn bullish_signal(
    direction: StrategyDirection,
    symbol: &str,
    quantity_base: Decimal,
    price: Decimal,
    reason: String,
) -> Option<Signal> {
    match direction {
        StrategyDirection::LongOnly | StrategyDirection::LongShort => Some(Signal {
            symbol: symbol.to_string(),
            side: Side::Buy,
            intent: SignalIntent::IncreaseLong,
            quantity_base,
            price,
            reason,
        }),
        StrategyDirection::ShortOnly => Some(Signal {
            symbol: symbol.to_string(),
            side: Side::Buy,
            intent: SignalIntent::DecreaseShort,
            quantity_base,
            price,
            reason,
        }),
    }
}

pub fn bearish_signal(
    direction: StrategyDirection,
    symbol: &str,
    quantity_base: Decimal,
    price: Decimal,
    reason: String,
) -> Option<Signal> {
    match direction {
        StrategyDirection::LongOnly => Some(Signal {
            symbol: symbol.to_string(),
            side: Side::Sell,
            intent: SignalIntent::DecreaseLong,
            quantity_base,
            price,
            reason,
        }),
        StrategyDirection::ShortOnly | StrategyDirection::LongShort => Some(Signal {
            symbol: symbol.to_string(),
            side: Side::Sell,
            intent: SignalIntent::IncreaseShort,
            quantity_base,
            price,
            reason,
        }),
    }
}
