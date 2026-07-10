use crate::config::SimpleMomentumConfig;
use crate::decimal::Decimal;
use crate::market::MarketEvent;
use crate::strategy::{Signal, Strategy, bearish_signal, bullish_signal};

pub struct SimpleMomentumStrategy {
    config: SimpleMomentumConfig,
    last_price: Option<Decimal>,
}

impl SimpleMomentumStrategy {
    pub fn new(config: SimpleMomentumConfig) -> Self {
        Self {
            config,
            last_price: None,
        }
    }
}

impl Strategy for SimpleMomentumStrategy {
    fn on_market_event(&mut self, event: &MarketEvent) -> Vec<Signal> {
        let previous_price = self.last_price.replace(event.price());

        let Some(previous_price) = previous_price else {
            return Vec::new();
        };

        let change = (event.price() - previous_price).ratio_to(previous_price);

        let change_bps = change * 10_000.0;

        if change_bps > self.config.buy_threshold_bps as f64 {
            bullish_signal(
                self.config.direction,
                event.symbol(),
                self.config.buy_quantity_base,
                event.price(),
                format!("price rose {:.2}%", change * 100.0),
            )
            .into_iter()
            .collect()
        } else if change_bps < self.config.sell_threshold_bps as f64 {
            bearish_signal(
                self.config.direction,
                event.symbol(),
                self.config.sell_quantity_base,
                event.price(),
                format!("price fell {:.2}%", change * 100.0),
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
    use super::SimpleMomentumStrategy;
    use crate::config::{SimpleMomentumConfig, StrategyDirection};
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

    fn strategy() -> SimpleMomentumStrategy {
        SimpleMomentumStrategy::new(SimpleMomentumConfig::default())
    }

    #[test]
    fn first_tick_does_not_emit_signal() {
        let mut strategy = strategy();

        let signals = strategy.on_market_event(&tick(100.0));

        assert!(signals.is_empty());
    }

    #[test]
    fn emits_buy_signal_when_price_rises_above_threshold() {
        let mut strategy = strategy();

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
        let mut strategy = strategy();

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
        let mut strategy = strategy();

        strategy.on_market_event(&tick(100.0));
        let signals = strategy.on_market_event(&tick(100.2));

        assert!(signals.is_empty());
    }

    #[test]
    fn uses_configured_thresholds_and_quantities() {
        let mut strategy = SimpleMomentumStrategy::new(SimpleMomentumConfig {
            buy_threshold_bps: 200,
            sell_threshold_bps: -300,
            buy_quantity_base: Decimal::from_micro_units(20_000),
            sell_quantity_base: Decimal::from_micro_units(10_000),
            direction: StrategyDirection::LongOnly,
        });

        strategy.on_market_event(&tick(100.0));
        assert!(strategy.on_market_event(&tick(101.0)).is_empty());

        let signals = strategy.on_market_event(&tick(104.0));

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].side, Side::Buy);
        assert_eq!(signals[0].quantity_base.to_string(), "0.02");
    }
}
