use crate::config::RsiMeanReversionConfig;
use crate::decimal::Decimal;
use crate::market::MarketEvent;
use crate::orders::Side;
use crate::strategy::{Signal, Strategy};
use std::collections::VecDeque;

pub struct RsiMeanReversionStrategy {
    config: RsiMeanReversionConfig,
    closes: VecDeque<Decimal>,
    previous_zone: RsiZone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RsiZone {
    Neutral,
    Oversold,
    Overbought,
}

impl RsiMeanReversionStrategy {
    pub fn new(config: RsiMeanReversionConfig) -> Self {
        Self {
            config,
            closes: VecDeque::new(),
            previous_zone: RsiZone::Neutral,
        }
    }
}

impl Strategy for RsiMeanReversionStrategy {
    fn on_market_event(&mut self, event: &MarketEvent) -> Vec<Signal> {
        self.closes.push_back(event.price());
        while self.closes.len() > self.config.window + 1 {
            self.closes.pop_front();
        }

        if self.closes.len() < self.config.window + 1 {
            return Vec::new();
        }

        let rsi = rsi(&self.closes);
        let zone = if rsi <= self.config.oversold_threshold as f64 {
            RsiZone::Oversold
        } else if rsi >= self.config.overbought_threshold as f64 {
            RsiZone::Overbought
        } else {
            RsiZone::Neutral
        };

        let signal = match zone {
            RsiZone::Oversold if self.previous_zone != RsiZone::Oversold => Some(Signal {
                symbol: event.symbol().to_string(),
                side: Side::Buy,
                quantity_base: self.config.quantity_base,
                price: event.price(),
                reason: format!(
                    "RSI {:.2} at/below oversold {}",
                    rsi, self.config.oversold_threshold
                ),
            }),
            RsiZone::Overbought if self.previous_zone != RsiZone::Overbought => Some(Signal {
                symbol: event.symbol().to_string(),
                side: Side::Sell,
                quantity_base: self.config.quantity_base,
                price: event.price(),
                reason: format!(
                    "RSI {:.2} at/above overbought {}",
                    rsi, self.config.overbought_threshold
                ),
            }),
            _ => None,
        };

        self.previous_zone = zone;
        signal.into_iter().collect()
    }
}

fn rsi(closes: &VecDeque<Decimal>) -> f64 {
    let mut total_gain = 0.0;
    let mut total_loss = 0.0;

    for index in 1..closes.len() {
        let change = closes[index].micro_units() - closes[index - 1].micro_units();
        if change > 0 {
            total_gain += change as f64;
        } else {
            total_loss += (-change) as f64;
        }
    }

    if total_loss == 0.0 {
        return 100.0;
    }
    if total_gain == 0.0 {
        return 0.0;
    }

    let relative_strength = total_gain / total_loss;
    100.0 - (100.0 / (1.0 + relative_strength))
}

#[cfg(test)]
mod tests {
    use super::RsiMeanReversionStrategy;
    use crate::config::RsiMeanReversionConfig;
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

    fn strategy() -> RsiMeanReversionStrategy {
        RsiMeanReversionStrategy::new(RsiMeanReversionConfig {
            window: 3,
            oversold_threshold: 30,
            overbought_threshold: 70,
            quantity_base: decimal("0.001"),
        })
    }

    #[test]
    fn waits_until_window_has_price_changes() {
        let mut strategy = strategy();

        assert!(strategy.on_market_event(&tick("100")).is_empty());
        assert!(strategy.on_market_event(&tick("99")).is_empty());
        assert!(strategy.on_market_event(&tick("98")).is_empty());
    }

    #[test]
    fn emits_buy_when_rsi_enters_oversold_zone() {
        let mut strategy = strategy();

        strategy.on_market_event(&tick("100"));
        strategy.on_market_event(&tick("99"));
        strategy.on_market_event(&tick("98"));
        let signals = strategy.on_market_event(&tick("97"));

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].side, Side::Buy);
        assert!(signals[0].reason.contains("oversold"));
    }

    #[test]
    fn emits_sell_when_rsi_enters_overbought_zone() {
        let mut strategy = strategy();

        strategy.on_market_event(&tick("100"));
        strategy.on_market_event(&tick("101"));
        strategy.on_market_event(&tick("102"));
        let signals = strategy.on_market_event(&tick("103"));

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].side, Side::Sell);
        assert!(signals[0].reason.contains("overbought"));
    }
}
