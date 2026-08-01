use crate::config::RsiRegimeConfig;
use crate::decimal::Decimal;
use crate::market::MarketEvent;
use crate::orders::Side;
use crate::portfolio::{FuturesPositionSide, Portfolio};
use crate::strategy::{Signal, SignalIntent, Strategy, bearish_signal, bullish_signal};
use std::collections::VecDeque;

pub struct RsiRegimeStrategy {
    config: RsiRegimeConfig,
    closes: VecDeque<Decimal>,
    previous_zone: RsiZone,
    tracked_position_side: FuturesPositionSide,
    holding_events: usize,
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
            tracked_position_side: FuturesPositionSide::Flat,
            holding_events: 0,
        }
    }

    fn evaluate(&mut self, event: &MarketEvent, portfolio: Option<&Portfolio>) -> Vec<Signal> {
        self.closes.push_back(event.price());
        let required_closes = (self.config.regime_window + 1).max(self.config.window + 1);
        while self.closes.len() > required_closes {
            self.closes.pop_front();
        }

        let futures_portfolio = portfolio.filter(|portfolio| portfolio.futures_enabled);
        if let Some(portfolio) = futures_portfolio {
            self.observe_position(portfolio.futures_position_side);
            if let Some(signal) = self.protective_exit(event, portfolio) {
                return vec![signal];
            }
        }

        if self.closes.len() < required_closes {
            return Vec::new();
        }

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

        if let Some(portfolio) = futures_portfolio {
            if let Some(signal) = self.regime_exit(event, portfolio, current_ma) {
                return vec![signal];
            }
            if portfolio.futures_position_side != FuturesPositionSide::Flat {
                return Vec::new();
            }
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

    fn observe_position(&mut self, position_side: FuturesPositionSide) {
        if position_side == FuturesPositionSide::Flat {
            self.tracked_position_side = FuturesPositionSide::Flat;
            self.holding_events = 0;
        } else if position_side == self.tracked_position_side {
            self.holding_events = self.holding_events.saturating_add(1);
        } else {
            self.tracked_position_side = position_side;
            self.holding_events = 1;
        }
    }

    fn protective_exit(&self, event: &MarketEvent, portfolio: &Portfolio) -> Option<Signal> {
        let side = portfolio.futures_position_side;
        if side == FuturesPositionSide::Flat || portfolio.futures_entry_price <= Decimal::ZERO {
            return None;
        }

        let pnl_bps = match side {
            FuturesPositionSide::Long => {
                (event.price() - portfolio.futures_entry_price)
                    .ratio_to(portfolio.futures_entry_price)
                    * 10_000.0
            }
            FuturesPositionSide::Short => {
                (portfolio.futures_entry_price - event.price())
                    .ratio_to(portfolio.futures_entry_price)
                    * 10_000.0
            }
            FuturesPositionSide::Flat => 0.0,
        };

        let reason = if pnl_bps >= self.config.take_profit_bps as f64 {
            Some(format!(
                "take profit at {pnl_bps:.2} bps (target {})",
                self.config.take_profit_bps
            ))
        } else if pnl_bps <= -(self.config.stop_loss_bps as f64) {
            Some(format!(
                "stop loss at {pnl_bps:.2} bps (limit -{})",
                self.config.stop_loss_bps
            ))
        } else if self.holding_events >= self.config.max_holding_events {
            Some(format!(
                "maximum holding period reached at {} events",
                self.holding_events
            ))
        } else {
            None
        }?;

        Some(close_signal(event, portfolio, reason))
    }

    fn regime_exit(
        &self,
        event: &MarketEvent,
        portfolio: &Portfolio,
        current_ma: Decimal,
    ) -> Option<Signal> {
        if !self.config.exit_on_regime_change {
            return None;
        }

        let invalidated = match portfolio.futures_position_side {
            FuturesPositionSide::Long => event.price() < current_ma,
            FuturesPositionSide::Short => event.price() > current_ma,
            FuturesPositionSide::Flat => false,
        };
        invalidated.then(|| {
            close_signal(
                event,
                portfolio,
                format!(
                    "{}-tick regime invalidated at MA {}",
                    self.config.regime_window, current_ma
                ),
            )
        })
    }
}

impl Strategy for RsiRegimeStrategy {
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

fn close_signal(event: &MarketEvent, portfolio: &Portfolio, reason: String) -> Signal {
    let (side, intent) = match portfolio.futures_position_side {
        FuturesPositionSide::Long => (Side::Sell, SignalIntent::DecreaseLong),
        FuturesPositionSide::Short => (Side::Buy, SignalIntent::DecreaseShort),
        FuturesPositionSide::Flat => unreachable!("flat position cannot produce close signal"),
    };
    Signal {
        symbol: event.symbol().to_string(),
        side,
        intent,
        quantity_base: portfolio.futures_position_base,
        price: event.price(),
        reason,
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
    use crate::portfolio::{FuturesPositionSide, Portfolio};
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
            take_profit_bps: 200,
            stop_loss_bps: 100,
            max_holding_events: 24,
            exit_on_regime_change: true,
            direction: StrategyDirection::LongShort,
        })
    }

    fn position(side: FuturesPositionSide, entry_price: &str) -> Portfolio {
        let mut portfolio = Portfolio::paper_futures("BTC", "USD", decimal("10000"));
        portfolio.futures_position_side = side;
        portfolio.futures_position_base = decimal("0.001");
        portfolio.futures_entry_price = decimal(entry_price);
        portfolio
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

    #[test]
    fn closes_full_long_at_take_profit() {
        let mut strategy = strategy(30, 70);
        let portfolio = position(FuturesPositionSide::Long, "100");

        let signals = strategy.on_market_event_with_portfolio(&tick("103"), &portfolio);

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].side, Side::Sell);
        assert_eq!(signals[0].intent, SignalIntent::DecreaseLong);
        assert_eq!(signals[0].quantity_base, decimal("0.001"));
        assert!(signals[0].reason.contains("take profit"));
    }

    #[test]
    fn closes_full_short_at_stop_loss() {
        let mut strategy = strategy(30, 70);
        let portfolio = position(FuturesPositionSide::Short, "100");

        let signals = strategy.on_market_event_with_portfolio(&tick("102"), &portfolio);

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].side, Side::Buy);
        assert_eq!(signals[0].intent, SignalIntent::DecreaseShort);
        assert!(signals[0].reason.contains("stop loss"));
    }

    #[test]
    fn closes_position_at_maximum_holding_period() {
        let mut strategy = strategy(30, 70);
        strategy.config.take_profit_bps = 10_000;
        strategy.config.stop_loss_bps = 10_000;
        strategy.config.max_holding_events = 2;
        strategy.config.exit_on_regime_change = false;
        let portfolio = position(FuturesPositionSide::Long, "100");

        assert!(
            strategy
                .on_market_event_with_portfolio(&tick("100"), &portfolio)
                .is_empty()
        );
        let signals = strategy.on_market_event_with_portfolio(&tick("100"), &portfolio);

        assert_eq!(signals.len(), 1);
        assert!(signals[0].reason.contains("maximum holding period"));
    }

    #[test]
    fn closes_long_when_price_invalidates_regime() {
        let mut strategy = strategy(30, 70);
        strategy.config.take_profit_bps = 10_000;
        strategy.config.stop_loss_bps = 10_000;
        for price in ["100", "102", "104", "106"] {
            strategy.on_market_event(&tick(price));
        }
        let portfolio = position(FuturesPositionSide::Long, "90");

        let signals = strategy.on_market_event_with_portfolio(&tick("90"), &portfolio);

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].intent, SignalIntent::DecreaseLong);
        assert!(signals[0].reason.contains("regime invalidated"));
    }
}
