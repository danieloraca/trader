use crate::decimal::Decimal;
use crate::error::{BotError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedPrice {
    pub recorded_at_ms: i64,
    pub price: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candle {
    pub start_ms: i64,
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub tick_count: usize,
}

pub fn aggregate_prices_to_candles(
    prices: &[RecordedPrice],
    interval_ms: i64,
) -> Result<Vec<Candle>> {
    if interval_ms <= 0 {
        return Err(BotError::Config(
            "candle interval must be positive".to_string(),
        ));
    }

    let mut candles = Vec::new();
    let mut current: Option<Candle> = None;

    for recorded_price in prices {
        let start_ms = (recorded_price.recorded_at_ms / interval_ms) * interval_ms;

        match &mut current {
            Some(candle) if candle.start_ms == start_ms => {
                if recorded_price.price > candle.high {
                    candle.high = recorded_price.price;
                }
                if recorded_price.price < candle.low {
                    candle.low = recorded_price.price;
                }
                candle.close = recorded_price.price;
                candle.tick_count += 1;
            }
            Some(_) => {
                candles.push(current.take().expect("current candle should exist"));
                current = Some(new_candle(start_ms, recorded_price.price));
            }
            None => current = Some(new_candle(start_ms, recorded_price.price)),
        }
    }

    if let Some(candle) = current {
        candles.push(candle);
    }

    Ok(candles)
}

fn new_candle(start_ms: i64, price: Decimal) -> Candle {
    Candle {
        start_ms,
        open: price,
        high: price,
        low: price,
        close: price,
        tick_count: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::{RecordedPrice, aggregate_prices_to_candles};
    use crate::decimal::Decimal;

    fn decimal(value: &str) -> Decimal {
        Decimal::from_decimal_str(value).expect("decimal should parse")
    }

    #[test]
    fn aggregates_recorded_prices_into_ohlc_candles() {
        let candles = aggregate_prices_to_candles(
            &[
                RecordedPrice {
                    recorded_at_ms: 1_000,
                    price: decimal("100"),
                },
                RecordedPrice {
                    recorded_at_ms: 20_000,
                    price: decimal("102"),
                },
                RecordedPrice {
                    recorded_at_ms: 40_000,
                    price: decimal("99"),
                },
                RecordedPrice {
                    recorded_at_ms: 70_000,
                    price: decimal("101"),
                },
            ],
            60_000,
        )
        .expect("candles should aggregate");

        assert_eq!(candles.len(), 2);
        assert_eq!(candles[0].start_ms, 0);
        assert_eq!(candles[0].open.to_string(), "100");
        assert_eq!(candles[0].high.to_string(), "102");
        assert_eq!(candles[0].low.to_string(), "99");
        assert_eq!(candles[0].close.to_string(), "99");
        assert_eq!(candles[0].tick_count, 3);
        assert_eq!(candles[1].start_ms, 60_000);
        assert_eq!(candles[1].close.to_string(), "101");
    }
}
