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
        let required_closes = self
            .config
            .regime_window
            .map_or(self.config.window + 1, |window| {
                (self.config.window + 1).max(window + 1)
            });
        while self.closes.len() > required_closes {
            self.closes.pop_front();
        }

        if self.closes.len() < required_closes {
            return Vec::new();
        }

        let rsi_start = self.closes.len() - (self.config.window + 1);
        let rsi = rsi(self.closes.iter().skip(rsi_start).copied());
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
        .and_then(|signal| self.apply_regime_filter(signal, event.price()))
        .and_then(|signal| self.apply_position_cap(signal, portfolio));

        self.previous_zone = zone;
        signal.into_iter().collect()
    }

    fn apply_regime_filter(&self, mut signal: Signal, price: Decimal) -> Option<Signal> {
        let Some(regime_window) = self.config.regime_window else {
            return Some(signal);
        };

        let current_average = average(
            self.closes
                .iter()
                .skip(self.closes.len() - regime_window)
                .copied(),
        );
        let previous_average = average(
            self.closes
                .iter()
                .skip(self.closes.len() - regime_window - 1)
                .take(regime_window)
                .copied(),
        );
        let regime_label = match signal.intent {
            SignalIntent::IncreaseLong
                if price > current_average && current_average > previous_average =>
            {
                "rising"
            }
            SignalIntent::IncreaseShort
                if price < current_average && current_average < previous_average =>
            {
                "falling"
            }
            SignalIntent::IncreaseLong | SignalIntent::IncreaseShort => return None,
            SignalIntent::DecreaseLong | SignalIntent::DecreaseShort => return Some(signal),
        };
        signal
            .reason
            .push_str(&format!(" in {regime_label} {regime_window}-tick regime"));
        Some(signal)
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

fn average(values: impl IntoIterator<Item = Decimal>) -> Decimal {
    let mut sum_micro_units = 0_i128;
    let mut count = 0_i128;
    for value in values {
        sum_micro_units += value.micro_units() as i128;
        count += 1;
    }
    Decimal::from_micro_units((sum_micro_units / count) as i64)
}

fn rsi(closes: impl IntoIterator<Item = Decimal>) -> f64 {
    let closes = closes.into_iter().collect::<Vec<_>>();
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
            regime_window: None,
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
    fn regime_filter_allows_long_pullback_in_rising_market() {
        let mut strategy = strategy();
        strategy.config.oversold_threshold = 70;
        strategy.config.regime_window = Some(4);
        for price in ["50", "100", "100", "200"] {
            assert!(strategy.on_market_event(&tick(price)).is_empty());
        }

        let signals = strategy.on_market_event(&tick("150"));

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].intent, SignalIntent::IncreaseLong);
        assert!(signals[0].reason.contains("rising 4-tick regime"));
    }

    #[test]
    fn regime_filter_blocks_long_entry_in_falling_market() {
        let mut strategy = strategy();
        strategy.config.oversold_threshold = 40;
        strategy.config.regime_window = Some(4);
        for price in ["104", "103", "102", "101"] {
            assert!(strategy.on_market_event(&tick(price)).is_empty());
        }

        assert!(strategy.on_market_event(&tick("100")).is_empty());
    }

    #[test]
    fn regime_filter_allows_short_rally_in_falling_market() {
        let mut strategy = strategy();
        strategy.config.direction = StrategyDirection::ShortOnly;
        strategy.config.oversold_threshold = 10;
        strategy.config.overbought_threshold = 30;
        strategy.config.regime_window = Some(4);
        for price in ["250", "200", "200", "100"] {
            assert!(strategy.on_market_event(&tick(price)).is_empty());
        }

        let signals = strategy.on_market_event(&tick("150"));

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].intent, SignalIntent::IncreaseShort);
        assert!(signals[0].reason.contains("falling 4-tick regime"));
    }

    #[test]
    fn regime_filter_never_blocks_exposure_reducing_signal() {
        let mut strategy = strategy();
        strategy.config.direction = StrategyDirection::ShortOnly;
        strategy.config.oversold_threshold = 40;
        strategy.config.regime_window = Some(4);
        for price in ["104", "103", "102", "101"] {
            assert!(strategy.on_market_event(&tick(price)).is_empty());
        }

        let signals = strategy.on_market_event(&tick("100"));

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].intent, SignalIntent::DecreaseShort);
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
