use crate::config::RsiMeanReversionConfig;
use crate::decimal::Decimal;
use crate::market::MarketEvent;
use crate::portfolio::{FuturesPositionSide, Portfolio};
use crate::strategy::{Signal, SignalIntent, Strategy, bearish_signal, bullish_signal};
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

    fn evaluate(&mut self, event: &MarketEvent, portfolio: Option<&Portfolio>) -> Vec<Signal> {
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
            RsiZone::Oversold if self.previous_zone != RsiZone::Oversold => bullish_signal(
                self.config.direction,
                event.symbol(),
                self.config.quantity_base,
                event.price(),
                format!(
                    "RSI {:.2} at/below oversold {}",
                    rsi, self.config.oversold_threshold
                ),
            ),
            RsiZone::Overbought if self.previous_zone != RsiZone::Overbought => bearish_signal(
                self.config.direction,
                event.symbol(),
                self.config.quantity_base,
                event.price(),
                format!(
                    "RSI {:.2} at/above overbought {}",
                    rsi, self.config.overbought_threshold
                ),
            ),
            _ => None,
        }
        .and_then(|signal| self.apply_position_cap(signal, portfolio));

        self.previous_zone = zone;
        signal.into_iter().collect()
    }

    fn apply_position_cap(
        &self,
        mut signal: Signal,
        portfolio: Option<&Portfolio>,
    ) -> Option<Signal> {
        let (Some(max_tranches), Some(portfolio)) = (self.config.max_tranches, portfolio) else {
            return Some(signal);
        };

        let increases_existing_side = matches!(
            (signal.intent, portfolio.futures_position_side),
            (SignalIntent::IncreaseLong, FuturesPositionSide::Long)
                | (SignalIntent::IncreaseShort, FuturesPositionSide::Short)
        );
        if !increases_existing_side {
            return Some(signal);
        }

        let tranche_multiplier = i64::try_from(max_tranches).unwrap_or(i64::MAX);
        let max_position = Decimal::from_micro_units(
            self.config
                .quantity_base
                .micro_units()
                .saturating_mul(tranche_multiplier),
        );
        let remaining = max_position - portfolio.futures_position_base;
        if remaining <= Decimal::ZERO {
            return None;
        }

        signal.quantity_base = signal.quantity_base.min(remaining);
        Some(signal)
    }
}

impl Strategy for RsiMeanReversionStrategy {
    fn on_market_event(&mut self, event: &MarketEvent) -> Vec<Signal> {
        self.evaluate(event, None)
    }

    fn on_market_event_with_portfolio(
        &mut self,
        event: &MarketEvent,
        portfolio: &Portfolio,
    ) -> Vec<Signal> {
        self.evaluate(event, Some(portfolio))
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
    use crate::config::{RsiMeanReversionConfig, StrategyDirection};
    use crate::decimal::Decimal;
    use crate::market::{MarketEvent, PriceTick};
    use crate::orders::Side;
    use crate::portfolio::{FuturesPositionSide, Portfolio};
    use crate::strategy::{SignalIntent, Strategy};

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
            max_tranches: None,
            direction: StrategyDirection::LongOnly,
        })
    }

    fn long_position(quantity: &str) -> Portfolio {
        let mut portfolio = Portfolio::paper_futures("BTC", "USD", decimal("10000"));
        portfolio.futures_position_side = FuturesPositionSide::Long;
        portfolio.futures_position_base = decimal(quantity);
        portfolio.futures_entry_price = decimal("100");
        portfolio
    }

    fn short_position(quantity: &str) -> Portfolio {
        let mut portfolio = Portfolio::paper_futures("BTC", "USD", decimal("10000"));
        portfolio.futures_position_side = FuturesPositionSide::Short;
        portfolio.futures_position_base = decimal(quantity);
        portfolio.futures_entry_price = decimal("100");
        portfolio
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

    #[test]
    fn capped_rsi_blocks_same_side_entry_at_cap() {
        let mut strategy = strategy();
        strategy.config.direction = StrategyDirection::LongShort;
        strategy.config.max_tranches = Some(4);
        let portfolio = long_position("0.004");
        for price in ["100", "99", "98"] {
            strategy.on_market_event_with_portfolio(&tick(price), &portfolio);
        }

        let signals = strategy.on_market_event_with_portfolio(&tick("97"), &portfolio);

        assert!(signals.is_empty());
    }

    #[test]
    fn capped_rsi_blocks_short_entry_at_cap() {
        let mut strategy = strategy();
        strategy.config.direction = StrategyDirection::LongShort;
        strategy.config.max_tranches = Some(4);
        let portfolio = short_position("0.004");
        for price in ["100", "101", "102"] {
            strategy.on_market_event_with_portfolio(&tick(price), &portfolio);
        }

        let signals = strategy.on_market_event_with_portfolio(&tick("103"), &portfolio);

        assert!(signals.is_empty());
    }

    #[test]
    fn capped_rsi_fills_only_remaining_capacity() {
        let mut strategy = strategy();
        strategy.config.direction = StrategyDirection::LongShort;
        strategy.config.max_tranches = Some(4);
        let portfolio = long_position("0.0035");
        for price in ["100", "99", "98"] {
            strategy.on_market_event_with_portfolio(&tick(price), &portfolio);
        }

        let signals = strategy.on_market_event_with_portfolio(&tick("97"), &portfolio);

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].intent, SignalIntent::IncreaseLong);
        assert_eq!(signals[0].quantity_base, decimal("0.0005"));
    }

    #[test]
    fn capped_rsi_preserves_opposite_signal_quantity() {
        let mut strategy = strategy();
        strategy.config.direction = StrategyDirection::LongShort;
        strategy.config.max_tranches = Some(4);
        let portfolio = long_position("0.004");
        for price in ["100", "101", "102"] {
            strategy.on_market_event_with_portfolio(&tick(price), &portfolio);
        }

        let signals = strategy.on_market_event_with_portfolio(&tick("103"), &portfolio);

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].intent, SignalIntent::IncreaseShort);
        assert_eq!(signals[0].quantity_base, decimal("0.001"));
    }
}
