use crate::decimal::Decimal;
use crate::market::MarketEvent;
use crate::orders::Side;
use crate::strategy::{Signal, Strategy};

pub struct SimpleMomentumStrategy {
    last_price: Option<Decimal>,
}

impl SimpleMomentumStrategy {
    pub fn new() -> Self {
        Self { last_price: None }
    }
}

impl Strategy for SimpleMomentumStrategy {
    fn on_market_event(&mut self, event: &MarketEvent) -> Vec<Signal> {
        let previous_price = self.last_price.replace(event.price());

        let Some(previous_price) = previous_price else {
            return Vec::new();
        };

        let change = (event.price() - previous_price).ratio_to(previous_price);

        if change > 0.005 {
            vec![Signal {
                symbol: event.symbol().to_string(),
                side: Side::Buy,
                quantity_base: Decimal::from_micro_units(10_000),
                price: event.price(),
                reason: format!("price rose {:.2}%", change * 100.0),
            }]
        } else if change < -0.01 {
            vec![Signal {
                symbol: event.symbol().to_string(),
                side: Side::Sell,
                quantity_base: Decimal::from_micro_units(5_000),
                price: event.price(),
                reason: format!("price fell {:.2}%", change * 100.0),
            }]
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SimpleMomentumStrategy;
    use crate::decimal::Decimal;
    use crate::market::{MarketEvent, PriceTick};
    use crate::orders::Side;
    use crate::strategy::Strategy;

    fn tick(price: f64) -> MarketEvent {
        MarketEvent::PriceTick(PriceTick::new(
            "BTC-USD",
            Decimal::from_f64(price).expect("price should parse"),
        ))
    }

    #[test]
    fn first_tick_does_not_emit_signal() {
        let mut strategy = SimpleMomentumStrategy::new();

        let signals = strategy.on_market_event(&tick(100.0));

        assert!(signals.is_empty());
    }

    #[test]
    fn emits_buy_signal_when_price_rises_above_threshold() {
        let mut strategy = SimpleMomentumStrategy::new();

        strategy.on_market_event(&tick(100.0));
        let signals = strategy.on_market_event(&tick(101.0));

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].side, Side::Buy);
        assert_eq!(signals[0].quantity_base.to_string(), "0.01");
        assert_eq!(signals[0].price.to_string(), "101");
        assert!(signals[0].reason.contains("price rose"));
    }

    #[test]
    fn emits_sell_signal_when_price_falls_below_threshold() {
        let mut strategy = SimpleMomentumStrategy::new();

        strategy.on_market_event(&tick(100.0));
        let signals = strategy.on_market_event(&tick(98.0));

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].side, Side::Sell);
        assert_eq!(signals[0].quantity_base.to_string(), "0.005");
        assert_eq!(signals[0].price.to_string(), "98");
        assert!(signals[0].reason.contains("price fell"));
    }

    #[test]
    fn ignores_price_change_inside_thresholds() {
        let mut strategy = SimpleMomentumStrategy::new();

        strategy.on_market_event(&tick(100.0));
        let signals = strategy.on_market_event(&tick(100.2));

        assert!(signals.is_empty());
    }
}
