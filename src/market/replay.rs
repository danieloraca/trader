use crate::decimal::Decimal;
use crate::error::Result;
use crate::market::{MarketDataSource, MarketEvent, PriceTick};

pub struct ReplayMarketDataSource {
    events: Vec<MarketEvent>,
    cursor: usize,
}

impl ReplayMarketDataSource {
    pub fn new_at_cursor(events: Vec<MarketEvent>, cursor: usize) -> Self {
        let cursor = cursor.min(events.len());
        Self { events, cursor }
    }

    pub fn from_prices_at_cursor(
        symbol: &str,
        prices: impl IntoIterator<Item = Decimal>,
        cursor: usize,
    ) -> Self {
        let events = prices
            .into_iter()
            .map(|price| MarketEvent::PriceTick(PriceTick::new(symbol, price)))
            .collect();

        Self::new_at_cursor(events, cursor)
    }
}

impl MarketDataSource for ReplayMarketDataSource {
    fn next_event(&mut self) -> Result<Option<MarketEvent>> {
        let event = self.events.get(self.cursor).cloned();

        if event.is_some() {
            self.cursor += 1;
        }

        Ok(event)
    }

    fn replay_cursor(&self) -> Option<usize> {
        Some(self.cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::ReplayMarketDataSource;
    use crate::decimal::Decimal;
    use crate::market::MarketDataSource;

    fn decimal(value: f64) -> Decimal {
        Decimal::from_f64(value).expect("decimal should parse")
    }

    #[test]
    fn emits_prices_in_order_then_ends() {
        let mut source = ReplayMarketDataSource::from_prices_at_cursor(
            "BTC-USD",
            [decimal(100.0), decimal(101.0)],
            0,
        );

        let first = source
            .next_event()
            .expect("source should not fail")
            .expect("first event should exist");
        let second = source
            .next_event()
            .expect("source should not fail")
            .expect("second event should exist");
        let third = source.next_event().expect("source should not fail");

        assert_eq!(first.symbol(), "BTC-USD");
        assert_eq!(first.price().to_string(), "100");
        assert_eq!(second.symbol(), "BTC-USD");
        assert_eq!(second.price().to_string(), "101");
        assert!(third.is_none());
    }

    #[test]
    fn starts_from_saved_cursor() {
        let mut source = ReplayMarketDataSource::from_prices_at_cursor(
            "BTC-USD",
            [decimal(100.0), decimal(101.0), decimal(102.0)],
            2,
        );

        let event = source
            .next_event()
            .expect("source should not fail")
            .expect("event should exist");

        assert_eq!(event.price().to_string(), "102");
        assert_eq!(source.replay_cursor(), Some(3));
        assert!(
            source
                .next_event()
                .expect("source should not fail")
                .is_none()
        );
    }

    #[test]
    fn clamps_saved_cursor_to_available_events() {
        let mut source =
            ReplayMarketDataSource::from_prices_at_cursor("BTC-USD", [decimal(100.0)], 99);

        assert_eq!(source.replay_cursor(), Some(1));
        assert!(
            source
                .next_event()
                .expect("source should not fail")
                .is_none()
        );
    }
}
