use crate::config::BreakoutConfig;
use crate::decimal::Decimal;
use crate::market::MarketEvent;
use crate::strategy::{Signal, Strategy, bearish_signal, bullish_signal};
use std::collections::VecDeque;

pub struct BreakoutStrategy {
    config: BreakoutConfig,
    closes: VecDeque<Decimal>,
}

impl BreakoutStrategy {
    pub fn new(config: BreakoutConfig) -> Self {
        Self {
            config,
            closes: VecDeque::new(),
        }
    }
}

impl Strategy for BreakoutStrategy {
    fn on_market_event(&mut self, event: &MarketEvent) -> Vec<Signal> {
        if self.closes.len() < self.config.window {
            self.closes.push_back(event.price());
            return Vec::new();
        }

        let previous_high = self
            .closes
            .iter()
            .copied()
            .max()
            .expect("window should contain prices");
        let previous_low = self
            .closes
            .iter()
            .copied()
            .min()
            .expect("window should contain prices");
        let price = event.price();

        self.closes.push_back(price);
        while self.closes.len() > self.config.window {
            self.closes.pop_front();
        }

        if price > previous_high {
            bullish_signal(
                self.config.direction,
                event.symbol(),
                self.config.quantity_base,
                price,
                format!(
                    "price broke above {}-tick high {}",
                    self.config.window, previous_high
                ),
            )
            .into_iter()
            .collect()
        } else if price < previous_low {
            bearish_signal(
                self.config.direction,
                event.symbol(),
                self.config.quantity_base,
                price,
                format!(
                    "price broke below {}-tick low {}",
                    self.config.window, previous_low
                ),
            )
            .into_iter()
            .collect()
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BreakoutStrategy;
    use crate::config::{BreakoutConfig, StrategyDirection};
    use crate::decimal::Decimal;
    use crate::market::{MarketEvent, PriceTick};
    use crate::orders::Side;
    use crate::strategy::{SignalIntent, Strategy};

    fn decimal(value: &str) -> Decimal {
        Decimal::from_decimal_str(value).expect("decimal should parse")
    }

    fn tick(price: &str) -> MarketEvent {
        MarketEvent::PriceTick(PriceTick::new("BTC-USD", decimal(price)))
    }

    fn strategy(direction: StrategyDirection) -> BreakoutStrategy {
        BreakoutStrategy::new(BreakoutConfig {
            window: 3,
            quantity_base: decimal("0.001"),
            direction,
        })
    }

    #[test]
    fn emits_long_signal_on_upside_breakout() {
        let mut strategy = strategy(StrategyDirection::LongOnly);

        strategy.on_market_event(&tick("100"));
        strategy.on_market_event(&tick("101"));
        strategy.on_market_event(&tick("99"));
        let signals = strategy.on_market_event(&tick("102"));

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].side, Side::Buy);
        assert_eq!(signals[0].intent, SignalIntent::IncreaseLong);
        assert!(signals[0].reason.contains("broke above"));
    }

    #[test]
    fn emits_short_signal_on_downside_breakout_when_short_only() {
        let mut strategy = strategy(StrategyDirection::ShortOnly);

        strategy.on_market_event(&tick("100"));
        strategy.on_market_event(&tick("101"));
        strategy.on_market_event(&tick("99"));
        let signals = strategy.on_market_event(&tick("98"));

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].side, Side::Sell);
        assert_eq!(signals[0].intent, SignalIntent::IncreaseShort);
        assert!(signals[0].reason.contains("broke below"));
    }
}
