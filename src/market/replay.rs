use crate::error::Result;
use crate::market::{MarketDataSource, MarketEvent, PriceTick};

pub struct ReplayMarketDataSource {
    events: Vec<MarketEvent>,
    cursor: usize,
}

impl ReplayMarketDataSource {
    pub fn new(events: Vec<MarketEvent>) -> Self {
        Self { events, cursor: 0 }
    }

    pub fn from_prices(symbol: &str, prices: impl IntoIterator<Item = f64>) -> Self {
        let events = prices
            .into_iter()
            .map(|price| MarketEvent::PriceTick(PriceTick::new(symbol, price)))
            .collect();

        Self::new(events)
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
}

#[cfg(test)]
mod tests {
    use super::ReplayMarketDataSource;
    use crate::market::MarketDataSource;

    #[test]
    fn emits_prices_in_order_then_ends() {
        let mut source = ReplayMarketDataSource::from_prices("BTC-USD", [100.0, 101.0]);

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
        assert_eq!(first.price(), 100.0);
        assert_eq!(second.symbol(), "BTC-USD");
        assert_eq!(second.price(), 101.0);
        assert!(third.is_none());
    }
}
