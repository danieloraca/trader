use crate::config::RsiRegimeConfig;
use crate::decimal::Decimal;
use crate::market::MarketEvent;
use crate::strategy::{Signal, Strategy, bearish_signal, bullish_signal};
use std::collections::VecDeque;

pub struct RsiRegimeStrategy {
    config: RsiRegimeConfig,
    closes: VecDeque<Decimal>,
    previous_zone: RsiZone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RsiZone {
    Neutral,
    Oversold,
    Overbought,
}

impl RsiRegimeStrategy {
    pub fn new(config: RsiRegimeConfig) -> Self {
        Self {
            config,
            closes: VecDeque::new(),
            previous_zone: RsiZone::Neutral,
        }
    }
}

impl Strategy for RsiRegimeStrategy {
    fn on_market_event(&mut self, event: &MarketEvent) -> Vec<Signal> {
        self.closes.push_back(event.price());
        let required_closes = (self.config.regime_window + 1).max(self.config.window + 1);
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

        let current_ma = average(
            self.closes
                .iter()
                .skip(self.closes.len() - self.config.regime_window)
                .copied(),
        );
        let previous_ma = average(
            self.closes
                .iter()
                .skip(self.closes.len() - self.config.regime_window - 1)
                .take(self.config.regime_window)
                .copied(),
        );
        let price = event.price();
        let bullish_regime = price > current_ma && current_ma > previous_ma;
        let bearish_regime = price < current_ma && current_ma < previous_ma;

        let signal = match zone {
            RsiZone::Oversold if self.previous_zone != RsiZone::Oversold && bullish_regime => {
                bullish_signal(
                    self.config.direction,
                    event.symbol(),
                    self.config.quantity_base,
                    price,
                    format!(
                        "RSI {:.2} oversold in rising {}-tick regime",
                        rsi, self.config.regime_window
                    ),
                )
            }
            RsiZone::Overbought if self.previous_zone != RsiZone::Overbought && bearish_regime => {
                bearish_signal(
                    self.config.direction,
                    event.symbol(),
                    self.config.quantity_base,
                    price,
                    format!(
                        "RSI {:.2} overbought in falling {}-tick regime",
                        rsi, self.config.regime_window
                    ),
                )
            }
            _ => None,
        };

        self.previous_zone = zone;
        signal.into_iter().collect()
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

fn rsi(values: impl IntoIterator<Item = Decimal>) -> f64 {
    let values = values.into_iter().collect::<Vec<_>>();
    let mut total_gain = 0.0;
    let mut total_loss = 0.0;
    for prices in values.windows(2) {
        let change = prices[1].micro_units() - prices[0].micro_units();
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
    use super::RsiRegimeStrategy;
    use crate::config::{RsiRegimeConfig, StrategyDirection};
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

    fn strategy(oversold_threshold: u8, overbought_threshold: u8) -> RsiRegimeStrategy {
        RsiRegimeStrategy::new(RsiRegimeConfig {
            window: 3,
            oversold_threshold,
            overbought_threshold,
            regime_window: 4,
            quantity_base: decimal("0.001"),
            direction: StrategyDirection::LongShort,
        })
    }

    #[test]
    fn emits_long_only_when_oversold_inside_rising_regime() {
        let mut strategy = strategy(70, 90);
        for price in ["50", "100", "100", "200"] {
            assert!(strategy.on_market_event(&tick(price)).is_empty());
        }
        let signals = strategy.on_market_event(&tick("150"));

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].side, Side::Buy);
        assert_eq!(signals[0].intent, SignalIntent::IncreaseLong);
        assert!(signals[0].reason.contains("rising"));
    }

    #[test]
    fn emits_short_only_when_overbought_inside_falling_regime() {
        let mut strategy = strategy(10, 30);
        for price in ["250", "200", "200", "100"] {
            assert!(strategy.on_market_event(&tick(price)).is_empty());
        }
        let signals = strategy.on_market_event(&tick("150"));

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].side, Side::Sell);
        assert_eq!(signals[0].intent, SignalIntent::IncreaseShort);
        assert!(signals[0].reason.contains("falling"));
    }

    #[test]
    fn blocks_countertrend_oversold_entry() {
        let mut strategy = strategy(40, 60);
        for price in ["104", "103", "102", "101", "100"] {
            strategy.on_market_event(&tick(price));
        }

        assert!(strategy.on_market_event(&tick("99")).is_empty());
    }
}
