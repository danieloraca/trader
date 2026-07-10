use crate::config::MovingAverageCrossoverConfig;
use crate::decimal::Decimal;
use crate::market::MarketEvent;
use crate::strategy::{Signal, Strategy, bearish_signal, bullish_signal};
use std::collections::VecDeque;

pub struct MovingAverageCrossoverStrategy {
    config: MovingAverageCrossoverConfig,
    closes: VecDeque<Decimal>,
    previous_fast_above_slow: Option<bool>,
}

impl MovingAverageCrossoverStrategy {
    pub fn new(config: MovingAverageCrossoverConfig) -> Self {
        Self {
            config,
            closes: VecDeque::new(),
            previous_fast_above_slow: None,
        }
    }
}

impl Strategy for MovingAverageCrossoverStrategy {
    fn on_market_event(&mut self, event: &MarketEvent) -> Vec<Signal> {
        self.closes.push_back(event.price());
        while self.closes.len() > self.config.slow_window {
            self.closes.pop_front();
        }

        if self.closes.len() < self.config.slow_window {
            return Vec::new();
        }

        let slow_average = average(self.closes.iter().copied());
        let fast_average = average(
            self.closes
                .iter()
                .skip(self.config.slow_window - self.config.fast_window)
                .copied(),
        );
        let fast_above_slow = fast_average > slow_average;
        let previous_fast_above_slow = self.previous_fast_above_slow.replace(fast_above_slow);

        match previous_fast_above_slow {
            Some(false) if fast_above_slow => bullish_signal(
                self.config.direction,
                event.symbol(),
                self.config.quantity_base,
                event.price(),
                format!(
                    "fast MA crossed above slow MA ({} > {})",
                    fast_average, slow_average
                ),
            )
            .into_iter()
            .collect(),
            Some(true) if !fast_above_slow => bearish_signal(
                self.config.direction,
                event.symbol(),
                self.config.quantity_base,
                event.price(),
                format!(
                    "fast MA crossed below slow MA ({} < {})",
                    fast_average, slow_average
                ),
            )
            .into_iter()
            .collect(),
            _ => Vec::new(),
        }
    }
}

fn average(values: impl IntoIterator<Item = Decimal>) -> Decimal {
    let mut sum_micro_units = 0_i128;
    let mut count = 0_i128;

    for value in values {
        sum_micro_units += value.micro_units() as i128;
        count += 1;
    }

    Decimal::from_micro_units((sum_micro_units / count) as i64)
}

#[cfg(test)]
mod tests {
    use super::MovingAverageCrossoverStrategy;
    use crate::config::{MovingAverageCrossoverConfig, StrategyDirection};
    use crate::decimal::Decimal;
    use crate::market::{MarketEvent, PriceTick};
    use crate::orders::Side;
    use crate::strategy::Strategy;

    fn decimal(value: &str) -> Decimal {
        Decimal::from_decimal_str(value).expect("decimal should parse")
    }

    fn tick(price: &str) -> MarketEvent {
        MarketEvent::PriceTick(PriceTick::new("BTC-USD", decimal(price)))
    }

    fn strategy() -> MovingAverageCrossoverStrategy {
        MovingAverageCrossoverStrategy::new(MovingAverageCrossoverConfig {
            fast_window: 2,
            slow_window: 3,
            quantity_base: decimal("0.001"),
            direction: StrategyDirection::LongOnly,
        })
    }

    #[test]
    fn waits_until_slow_window_is_full() {
        let mut strategy = strategy();

        assert!(strategy.on_market_event(&tick("100")).is_empty());
        assert!(strategy.on_market_event(&tick("99")).is_empty());
    }

    #[test]
    fn emits_buy_when_fast_average_crosses_above_slow_average() {
        let mut strategy = strategy();

        strategy.on_market_event(&tick("100"));
        strategy.on_market_event(&tick("99"));
        strategy.on_market_event(&tick("98"));
        let signals = strategy.on_market_event(&tick("103"));

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].side, Side::Buy);
        assert_eq!(signals[0].quantity_base.to_string(), "0.001");
        assert!(signals[0].reason.contains("crossed above"));
    }

    #[test]
    fn emits_sell_when_fast_average_crosses_below_slow_average() {
        let mut strategy = strategy();

        strategy.on_market_event(&tick("100"));
        strategy.on_market_event(&tick("99"));
        strategy.on_market_event(&tick("98"));
        strategy.on_market_event(&tick("103"));
        let signals = strategy.on_market_event(&tick("90"));

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].side, Side::Sell);
        assert!(signals[0].reason.contains("crossed below"));
    }
}
