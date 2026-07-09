use crate::config::Config;
use crate::decimal::Decimal;
use crate::error::{BotError, Result};
use crate::market::{MarketDataSource, MarketEvent, PriceTick};
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::Value;
use std::thread;
use std::time::{Duration, Instant};

pub struct KrakenTickerMarketDataSource {
    client: Client,
    base_url: String,
    pair: String,
    symbol: String,
    poll_interval: Duration,
    last_poll: Option<Instant>,
}

#[derive(Debug, Deserialize)]
struct KrakenTickerEnvelope {
    error: Vec<String>,
    result: Value,
}

impl KrakenTickerMarketDataSource {
    pub fn new(config: &Config) -> Self {
        let kraken = &config.market_data.kraken;
        Self {
            client: Client::new(),
            base_url: kraken.base_url.trim_end_matches('/').to_string(),
            pair: kraken.pair.clone(),
            symbol: config.bot.symbol.clone(),
            poll_interval: Duration::from_millis(kraken.poll_interval_ms),
            last_poll: None,
        }
    }

    fn wait_until_next_poll(&mut self) {
        if let Some(last_poll) = self.last_poll {
            let elapsed = last_poll.elapsed();
            if elapsed < self.poll_interval {
                thread::sleep(self.poll_interval - elapsed);
            }
        }

        self.last_poll = Some(Instant::now());
    }

    fn fetch_price(&self) -> Result<Decimal> {
        let url = format!("{}/0/public/Ticker", self.base_url);
        let response = self
            .client
            .get(url)
            .query(&[("pair", self.pair.as_str())])
            .send()
            .map_err(|error| {
                BotError::MarketData(format!("kraken ticker request failed: {error}"))
            })?
            .error_for_status()
            .map_err(|error| {
                BotError::MarketData(format!("kraken ticker returned http error: {error}"))
            })?
            .json::<KrakenTickerEnvelope>()
            .map_err(|error| {
                BotError::MarketData(format!("failed to decode kraken ticker response: {error}"))
            })?;

        if !response.error.is_empty() {
            return Err(BotError::MarketData(format!(
                "kraken ticker rejected request: {}",
                response.error.join(", ")
            )));
        }

        ticker_price_from_result(&response.result)
    }
}

impl MarketDataSource for KrakenTickerMarketDataSource {
    fn next_event(&mut self) -> Result<Option<MarketEvent>> {
        self.wait_until_next_poll();
        let price = self.fetch_price()?;

        Ok(Some(MarketEvent::PriceTick(PriceTick::new(
            &self.symbol,
            price,
        ))))
    }
}

fn ticker_price_from_result(result: &Value) -> Result<Decimal> {
    let pair = result
        .as_object()
        .and_then(|pairs| pairs.values().next())
        .ok_or_else(|| BotError::MarketData("kraken ticker result was empty".to_string()))?;
    let price = pair
        .get("c")
        .and_then(Value::as_array)
        .and_then(|last_trade| last_trade.first())
        .and_then(Value::as_str)
        .ok_or_else(|| {
            BotError::MarketData("kraken ticker response missing last trade price".to_string())
        })?;

    Decimal::from_decimal_str(price)
        .map_err(|error| BotError::MarketData(format!("invalid kraken ticker price: {error}")))
}

#[cfg(test)]
mod tests {
    use super::ticker_price_from_result;
    use serde_json::json;

    #[test]
    fn parses_last_trade_price_from_ticker_result() {
        let result = json!({
            "XXBTZUSD": {
                "a": ["30300.10000", "1", "1.000"],
                "b": ["30300.00000", "1", "1.000"],
                "c": ["30303.20000", "0.00067643"]
            }
        });

        let price = ticker_price_from_result(&result).expect("price should parse");

        assert_eq!(price.to_string(), "30303.2");
    }
}
