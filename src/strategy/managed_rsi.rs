use crate::config::ManagedRsiConfig;
use crate::decimal::Decimal;
use crate::market::MarketEvent;
use crate::orders::Side;
use crate::portfolio::{FuturesPositionSide, Portfolio};
use crate::strategy::{Signal, SignalIntent, Strategy, bearish_signal, bullish_signal};
use std::collections::VecDeque;

pub struct ManagedRsiStrategy {
    config: ManagedRsiConfig,
    closes: VecDeque<Decimal>,
    previous_zone: RsiZone,
    tracked_position_side: FuturesPositionSide,
    holding_events: usize,
    cooldown_remaining: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RsiZone {
    Neutral,
    Oversold,
    Overbought,
}

impl ManagedRsiStrategy {
    pub fn new(config: ManagedRsiConfig) -> Self {
        Self {
            config,
            closes: VecDeque::new(),
            previous_zone: RsiZone::Neutral,
            tracked_position_side: FuturesPositionSide::Flat,
            holding_events: 0,
            cooldown_remaining: 0,
        }
    }

    fn evaluate(&mut self, event: &MarketEvent, portfolio: Option<&Portfolio>) -> Vec<Signal> {
        self.closes.push_back(event.price());
        while self.closes.len() > self.config.window + 1 {
            self.closes.pop_front();
        }

        let futures_portfolio = portfolio.filter(|portfolio| portfolio.futures_enabled);
        if let Some(portfolio) = futures_portfolio {
            self.observe_position(portfolio.futures_position_side);
            if let Some(signal) = self.protective_exit(event, portfolio) {
                return vec![signal];
            }
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

        if let Some(portfolio) = futures_portfolio {
            if let Some(signal) = self.opposite_signal_exit(event, portfolio, zone, rsi) {
                self.previous_zone = zone;
                return vec![signal];
            }
        }

        let flat_entry_allowed = match futures_portfolio {
            Some(portfolio)
                if portfolio.futures_position_side == FuturesPositionSide::Flat
                    && self.cooldown_remaining > 0 =>
            {
                self.cooldown_remaining -= 1;
                false
            }
            _ => true,
        };

        let signal = match zone {
            RsiZone::Oversold if self.previous_zone != RsiZone::Oversold => self
                .entry_quantity(
                    futures_portfolio,
                    FuturesPositionSide::Long,
                    flat_entry_allowed,
                )
                .and_then(|quantity_base| {
                    bullish_signal(
                        self.config.direction,
                        event.symbol(),
                        quantity_base,
                        event.price(),
                        format!(
                            "managed RSI {:.2} at/below oversold {}",
                            rsi, self.config.oversold_threshold
                        ),
                    )
                }),
            RsiZone::Overbought if self.previous_zone != RsiZone::Overbought => self
                .entry_quantity(
                    futures_portfolio,
                    FuturesPositionSide::Short,
                    flat_entry_allowed,
                )
                .and_then(|quantity_base| {
                    bearish_signal(
                        self.config.direction,
                        event.symbol(),
                        quantity_base,
                        event.price(),
                        format!(
                            "managed RSI {:.2} at/above overbought {}",
                            rsi, self.config.overbought_threshold
                        ),
                    )
                }),
            _ => None,
        };

        self.previous_zone = zone;
        signal.into_iter().collect()
    }

    fn observe_position(&mut self, position_side: FuturesPositionSide) {
        if position_side == FuturesPositionSide::Flat {
            if self.tracked_position_side != FuturesPositionSide::Flat {
                self.cooldown_remaining = self.config.cooldown_events;
            }
            self.tracked_position_side = FuturesPositionSide::Flat;
            self.holding_events = 0;
        } else if position_side == self.tracked_position_side {
            self.holding_events = self.holding_events.saturating_add(1);
        } else {
            self.tracked_position_side = position_side;
            self.holding_events = 1;
            self.cooldown_remaining = 0;
        }
    }

    fn entry_quantity(
        &self,
        portfolio: Option<&Portfolio>,
        target_side: FuturesPositionSide,
        flat_entry_allowed: bool,
    ) -> Option<Decimal> {
        let Some(portfolio) = portfolio else {
            return Some(self.config.quantity_base);
        };

        match portfolio.futures_position_side {
            FuturesPositionSide::Flat => flat_entry_allowed.then_some(self.config.quantity_base),
            side if side == target_side => {
                let tranche_multiplier =
                    i64::try_from(self.config.max_tranches).unwrap_or(i64::MAX);
                let max_position = Decimal::from_micro_units(
                    self.config
                        .quantity_base
                        .micro_units()
                        .saturating_mul(tranche_multiplier),
                );
                let remaining = max_position - portfolio.futures_position_base;
                (remaining > Decimal::ZERO).then_some(self.config.quantity_base.min(remaining))
            }
            _ => None,
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

    fn opposite_signal_exit(
        &self,
        event: &MarketEvent,
        portfolio: &Portfolio,
        zone: RsiZone,
        rsi: f64,
    ) -> Option<Signal> {
        if !self.config.exit_on_opposite_signal && !self.config.reduce_on_opposite_signal {
            return None;
        }

        let opposite_transition = match portfolio.futures_position_side {
            FuturesPositionSide::Long => {
                zone == RsiZone::Overbought && self.previous_zone != RsiZone::Overbought
            }
            FuturesPositionSide::Short => {
                zone == RsiZone::Oversold && self.previous_zone != RsiZone::Oversold
            }
            FuturesPositionSide::Flat => false,
        };

        if !opposite_transition {
            return None;
        }

        if self.config.exit_on_opposite_signal {
            return Some(close_signal(
                event,
                portfolio,
                format!("opposite RSI exit at {rsi:.2}"),
            ));
        }

        Some(close_signal_with_quantity(
            event,
            portfolio,
            self.config
                .quantity_base
                .min(portfolio.futures_position_base),
            format!("opposite RSI tranche reduction at {rsi:.2}"),
        ))
    }
}

impl Strategy for ManagedRsiStrategy {
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
    close_signal_with_quantity(event, portfolio, portfolio.futures_position_base, reason)
}

fn close_signal_with_quantity(
    event: &MarketEvent,
    portfolio: &Portfolio,
    quantity_base: Decimal,
    reason: String,
) -> Signal {
    let (side, intent) = match portfolio.futures_position_side {
        FuturesPositionSide::Long => (Side::Sell, SignalIntent::DecreaseLong),
        FuturesPositionSide::Short => (Side::Buy, SignalIntent::DecreaseShort),
        FuturesPositionSide::Flat => unreachable!("flat position cannot produce close signal"),
    };
    Signal {
        symbol: event.symbol().to_string(),
        side,
        intent,
        quantity_base,
        price: event.price(),
        reason,
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
    use super::ManagedRsiStrategy;
    use crate::config::{ManagedRsiConfig, StrategyDirection};
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

    fn strategy() -> ManagedRsiStrategy {
        ManagedRsiStrategy::new(ManagedRsiConfig {
            window: 3,
            oversold_threshold: 30,
            overbought_threshold: 70,
            quantity_base: decimal("0.001"),
            take_profit_bps: 200,
            stop_loss_bps: 100,
            max_holding_events: 24,
            cooldown_events: 2,
            max_tranches: 1,
            exit_on_opposite_signal: false,
            reduce_on_opposite_signal: false,
            direction: StrategyDirection::LongShort,
        })
    }

    fn flat_portfolio() -> Portfolio {
        Portfolio::paper_futures("BTC", "USD", decimal("10000"))
    }

    fn position(side: FuturesPositionSide, entry_price: &str) -> Portfolio {
        let mut portfolio = flat_portfolio();
        portfolio.futures_position_side = side;
        portfolio.futures_position_base = decimal("0.001");
        portfolio.futures_entry_price = decimal(entry_price);
        portfolio
    }

    #[test]
    fn enters_on_raw_rsi_zone_transition() {
        let mut strategy = strategy();
        let portfolio = flat_portfolio();
        for price in ["100", "99", "98"] {
            assert!(
                strategy
                    .on_market_event_with_portfolio(&tick(price), &portfolio)
                    .is_empty()
            );
        }

        let signals = strategy.on_market_event_with_portfolio(&tick("97"), &portfolio);

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].side, Side::Buy);
        assert_eq!(signals[0].intent, SignalIntent::IncreaseLong);
        assert!(signals[0].reason.contains("managed RSI"));
    }

    #[test]
    fn enters_short_on_overbought_zone_transition() {
        let mut strategy = strategy();
        let portfolio = flat_portfolio();
        for price in ["100", "101", "102"] {
            assert!(
                strategy
                    .on_market_event_with_portfolio(&tick(price), &portfolio)
                    .is_empty()
            );
        }

        let signals = strategy.on_market_event_with_portfolio(&tick("103"), &portfolio);

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].side, Side::Sell);
        assert_eq!(signals[0].intent, SignalIntent::IncreaseShort);
    }

    #[test]
    fn does_not_pyramid_while_position_is_open() {
        let mut strategy = strategy();
        let flat = flat_portfolio();
        for price in ["100", "99", "98"] {
            strategy.on_market_event_with_portfolio(&tick(price), &flat);
        }
        let long = position(FuturesPositionSide::Long, "97");

        let signals = strategy.on_market_event_with_portfolio(&tick("97"), &long);

        assert!(signals.is_empty());
    }

    #[test]
    fn adds_one_tranche_on_same_side_transition_below_cap() {
        let mut strategy = strategy();
        strategy.config.take_profit_bps = 10_000;
        strategy.config.stop_loss_bps = 10_000;
        strategy.config.max_tranches = 2;
        let long = position(FuturesPositionSide::Long, "100");
        for price in ["100", "99", "98"] {
            strategy.on_market_event_with_portfolio(&tick(price), &long);
        }

        let signals = strategy.on_market_event_with_portfolio(&tick("97"), &long);

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].intent, SignalIntent::IncreaseLong);
        assert_eq!(signals[0].quantity_base, decimal("0.001"));
    }

    #[test]
    fn does_not_add_tranche_at_position_cap() {
        let mut strategy = strategy();
        strategy.config.take_profit_bps = 10_000;
        strategy.config.stop_loss_bps = 10_000;
        strategy.config.max_tranches = 2;
        let mut long = position(FuturesPositionSide::Long, "100");
        long.futures_position_base = decimal("0.002");
        for price in ["100", "99", "98"] {
            strategy.on_market_event_with_portfolio(&tick(price), &long);
        }

        let signals = strategy.on_market_event_with_portfolio(&tick("97"), &long);

        assert!(signals.is_empty());
    }

    #[test]
    fn closes_full_position_at_take_profit() {
        let mut strategy = strategy();
        let long = position(FuturesPositionSide::Long, "100");

        let signals = strategy.on_market_event_with_portfolio(&tick("103"), &long);

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].side, Side::Sell);
        assert_eq!(signals[0].intent, SignalIntent::DecreaseLong);
        assert_eq!(signals[0].quantity_base, decimal("0.001"));
        assert!(signals[0].reason.contains("take profit"));
    }

    #[test]
    fn closes_full_short_at_stop_loss() {
        let mut strategy = strategy();
        let short = position(FuturesPositionSide::Short, "100");

        let signals = strategy.on_market_event_with_portfolio(&tick("102"), &short);

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].side, Side::Buy);
        assert_eq!(signals[0].intent, SignalIntent::DecreaseShort);
        assert_eq!(signals[0].quantity_base, decimal("0.001"));
        assert!(signals[0].reason.contains("stop loss"));
    }

    #[test]
    fn protective_exit_closes_all_scaled_tranches() {
        let mut strategy = strategy();
        strategy.config.max_tranches = 3;
        let mut long = position(FuturesPositionSide::Long, "100");
        long.futures_position_base = decimal("0.003");

        let signals = strategy.on_market_event_with_portfolio(&tick("103"), &long);

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].intent, SignalIntent::DecreaseLong);
        assert_eq!(signals[0].quantity_base, decimal("0.003"));
        assert!(signals[0].reason.contains("take profit"));
    }

    #[test]
    fn closes_position_at_maximum_holding_period() {
        let mut strategy = strategy();
        strategy.config.take_profit_bps = 10_000;
        strategy.config.stop_loss_bps = 10_000;
        strategy.config.max_holding_events = 2;
        let long = position(FuturesPositionSide::Long, "100");

        assert!(
            strategy
                .on_market_event_with_portfolio(&tick("100"), &long)
                .is_empty()
        );
        let signals = strategy.on_market_event_with_portfolio(&tick("100"), &long);

        assert_eq!(signals.len(), 1);
        assert!(signals[0].reason.contains("maximum holding period"));
    }

    #[test]
    fn waits_configured_cooldown_after_position_closes() {
        let mut strategy = strategy();
        strategy.config.take_profit_bps = 10_000;
        strategy.config.stop_loss_bps = 10_000;
        let long = position(FuturesPositionSide::Long, "100");
        let flat = flat_portfolio();
        for price in ["100", "101", "100", "101"] {
            strategy.on_market_event_with_portfolio(&tick(price), &long);
        }

        strategy.on_market_event_with_portfolio(&tick("100"), &flat);
        assert_eq!(strategy.cooldown_remaining, 1);
        strategy.on_market_event_with_portfolio(&tick("101"), &flat);
        assert_eq!(strategy.cooldown_remaining, 0);
    }

    #[test]
    fn closes_long_on_overbought_transition_when_enabled() {
        let mut strategy = strategy();
        strategy.config.take_profit_bps = 10_000;
        strategy.config.stop_loss_bps = 10_000;
        strategy.config.exit_on_opposite_signal = true;
        let long = position(FuturesPositionSide::Long, "100");
        for price in ["100", "101", "102"] {
            assert!(
                strategy
                    .on_market_event_with_portfolio(&tick(price), &long)
                    .is_empty()
            );
        }

        let signals = strategy.on_market_event_with_portfolio(&tick("103"), &long);

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].intent, SignalIntent::DecreaseLong);
        assert!(signals[0].reason.starts_with("opposite RSI exit"));
    }

    #[test]
    fn keeps_position_open_on_opposite_transition_when_disabled() {
        let mut strategy = strategy();
        strategy.config.take_profit_bps = 10_000;
        strategy.config.stop_loss_bps = 10_000;
        let long = position(FuturesPositionSide::Long, "100");
        for price in ["100", "101", "102"] {
            strategy.on_market_event_with_portfolio(&tick(price), &long);
        }

        let signals = strategy.on_market_event_with_portfolio(&tick("103"), &long);

        assert!(signals.is_empty());
    }

    #[test]
    fn reduces_long_by_one_tranche_on_opposite_transition() {
        let mut strategy = strategy();
        strategy.config.take_profit_bps = 10_000;
        strategy.config.stop_loss_bps = 10_000;
        strategy.config.max_tranches = 3;
        strategy.config.reduce_on_opposite_signal = true;
        let mut long = position(FuturesPositionSide::Long, "100");
        long.futures_position_base = decimal("0.003");
        for price in ["100", "101", "102"] {
            strategy.on_market_event_with_portfolio(&tick(price), &long);
        }

        let signals = strategy.on_market_event_with_portfolio(&tick("103"), &long);

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].intent, SignalIntent::DecreaseLong);
        assert_eq!(signals[0].quantity_base, decimal("0.001"));
        assert!(signals[0].reason.starts_with("opposite RSI tranche"));
    }

    #[test]
    fn reduces_short_by_one_tranche_on_opposite_transition() {
        let mut strategy = strategy();
        strategy.config.take_profit_bps = 10_000;
        strategy.config.stop_loss_bps = 10_000;
        strategy.config.max_tranches = 2;
        strategy.config.reduce_on_opposite_signal = true;
        let mut short = position(FuturesPositionSide::Short, "100");
        short.futures_position_base = decimal("0.002");
        for price in ["100", "99", "98"] {
            strategy.on_market_event_with_portfolio(&tick(price), &short);
        }

        let signals = strategy.on_market_event_with_portfolio(&tick("97"), &short);

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].intent, SignalIntent::DecreaseShort);
        assert_eq!(signals[0].quantity_base, decimal("0.001"));
        assert!(signals[0].reason.starts_with("opposite RSI tranche"));
    }

    #[test]
    fn closes_short_on_oversold_transition_when_enabled() {
        let mut strategy = strategy();
        strategy.config.take_profit_bps = 10_000;
        strategy.config.stop_loss_bps = 10_000;
        strategy.config.exit_on_opposite_signal = true;
        let short = position(FuturesPositionSide::Short, "100");
        for price in ["100", "99", "98"] {
            assert!(
                strategy
                    .on_market_event_with_portfolio(&tick(price), &short)
                    .is_empty()
            );
        }

        let signals = strategy.on_market_event_with_portfolio(&tick("97"), &short);

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].intent, SignalIntent::DecreaseShort);
        assert!(signals[0].reason.starts_with("opposite RSI exit"));
    }
}
