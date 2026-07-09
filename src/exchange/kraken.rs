use crate::config::Config;
use crate::decimal::Decimal;
use crate::error::{BotError, Result};
use crate::exchange::Exchange;
use crate::orders::{ExchangeOrder, OrderRequest, OrderStatus, Side};
use crate::portfolio::Portfolio;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use hmac::{Hmac, Mac};
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256, Sha512};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha512 = Hmac<Sha512>;

pub struct KrakenExchange {
    client: Client,
    base_url: String,
    api_key: String,
    api_secret: String,
    pair: String,
    portfolio: Portfolio,
    enable_order_placement: bool,
}

#[derive(Debug, Deserialize)]
struct KrakenEnvelope {
    error: Vec<String>,
    result: Value,
}

impl KrakenExchange {
    pub fn new(config: &Config, portfolio: Portfolio) -> Result<Self> {
        let kraken = &config.exchange.kraken;
        let api_key = std::env::var(&kraken.api_key_env).map_err(|_| {
            BotError::Config(format!(
                "environment variable {} must be set for Kraken",
                kraken.api_key_env
            ))
        })?;
        let api_secret = std::env::var(&kraken.api_secret_env).map_err(|_| {
            BotError::Config(format!(
                "environment variable {} must be set for Kraken",
                kraken.api_secret_env
            ))
        })?;

        Ok(Self {
            client: Client::new(),
            base_url: kraken.base_url.trim_end_matches('/').to_string(),
            api_key,
            api_secret,
            pair: kraken.pair.clone(),
            portfolio,
            enable_order_placement: kraken.enable_order_placement,
        })
    }

    fn private_post(&self, path: &str, fields: &[(&str, String)]) -> Result<Value> {
        let nonce = nonce()?;
        let mut encoded_fields = vec![("nonce", nonce.clone())];
        encoded_fields.extend_from_slice(fields);
        let body = form_encode(&encoded_fields);
        let signature = kraken_signature(path, &body, &nonce, &self.api_secret)?;
        let url = format!("{}{}", self.base_url, path);

        let response = self
            .client
            .post(url)
            .header("API-Key", &self.api_key)
            .header("API-Sign", signature)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .map_err(|error| BotError::Exchange(format!("kraken request failed: {error}")))?
            .error_for_status()
            .map_err(|error| BotError::Exchange(format!("kraken returned http error: {error}")))?
            .json::<KrakenEnvelope>()
            .map_err(|error| {
                BotError::Exchange(format!("failed to decode kraken response: {error}"))
            })?;

        if !response.error.is_empty() {
            return Err(BotError::Exchange(format!(
                "kraken rejected request: {}",
                response.error.join(", ")
            )));
        }

        Ok(response.result)
    }

    fn balance_for_currency(balances: &HashMap<String, Decimal>, currency: &str) -> Decimal {
        kraken_asset_codes(currency)
            .into_iter()
            .find_map(|asset| balances.get(&asset).copied())
            .unwrap_or(Decimal::ZERO)
    }

    fn exchange_order_from_value(exchange_order_id: &str, value: &Value) -> Result<ExchangeOrder> {
        let status = value
            .get("status")
            .and_then(Value::as_str)
            .map(kraken_status)
            .unwrap_or(OrderStatus::Submitted);
        let client_order_id = value
            .get("cl_ord_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        Ok(ExchangeOrder {
            exchange_order_id: exchange_order_id.to_string(),
            client_order_id,
            status,
        })
    }

    fn find_order_by_client_id_in(
        &self,
        path: &str,
        root: &str,
        client_order_id: &str,
    ) -> Result<Option<ExchangeOrder>> {
        let result = self.private_post(path, &[])?;
        let Some(orders) = result.get(root).and_then(Value::as_object) else {
            return Ok(None);
        };

        for (exchange_order_id, value) in orders {
            if value.get("cl_ord_id").and_then(Value::as_str) == Some(client_order_id) {
                return Self::exchange_order_from_value(exchange_order_id, value).map(Some);
            }
        }

        Ok(None)
    }
}

impl Exchange for KrakenExchange {
    fn portfolio(&self) -> &Portfolio {
        &self.portfolio
    }

    fn sync_portfolio(&mut self) -> Result<Portfolio> {
        let result = self.private_post("/0/private/Balance", &[])?;
        let balances = result
            .as_object()
            .ok_or_else(|| {
                BotError::Exchange("kraken balance result was not an object".to_string())
            })?
            .iter()
            .map(|(asset, value)| {
                let balance = value
                    .as_str()
                    .ok_or_else(|| {
                        BotError::Exchange(format!("kraken balance for {asset} was not a string"))
                    })
                    .and_then(|value| {
                        Decimal::from_decimal_str(value).map_err(|error| {
                            BotError::Exchange(format!(
                                "invalid kraken balance for {asset}: {error}"
                            ))
                        })
                    })?;
                Ok((asset.clone(), balance))
            })
            .collect::<Result<HashMap<_, _>>>()?;

        self.portfolio.base_balance =
            Self::balance_for_currency(&balances, &self.portfolio.base_currency);
        self.portfolio.quote_balance =
            Self::balance_for_currency(&balances, &self.portfolio.quote_currency);

        Ok(self.portfolio.clone())
    }

    fn place_order(&mut self, request: OrderRequest) -> Result<ExchangeOrder> {
        if !self.enable_order_placement {
            return Err(BotError::Exchange(
                "kraken order placement is disabled in config".to_string(),
            ));
        }

        let client_order_id = request.client_order_id.clone().ok_or_else(|| {
            BotError::Exchange("order request missing client order id".to_string())
        })?;
        if client_order_id.len() > 18 {
            return Err(BotError::Exchange(format!(
                "kraken client order id must be at most 18 characters: {client_order_id}"
            )));
        }

        let side = match request.side {
            Side::Buy => "buy",
            Side::Sell => "sell",
        };
        let fields = [
            ("ordertype", "limit".to_string()),
            ("type", side.to_string()),
            ("volume", request.quantity_base.to_string()),
            ("pair", self.pair.clone()),
            ("price", request.limit_price.to_string()),
            ("cl_ord_id", client_order_id.clone()),
        ];
        let result = self.private_post("/0/private/AddOrder", &fields)?;
        let exchange_order_id = result
            .get("txid")
            .and_then(Value::as_array)
            .and_then(|txids| txids.first())
            .and_then(Value::as_str)
            .ok_or_else(|| {
                BotError::Exchange("kraken AddOrder response missing txid".to_string())
            })?;

        Ok(ExchangeOrder {
            exchange_order_id: exchange_order_id.to_string(),
            client_order_id,
            status: OrderStatus::Submitted,
        })
    }

    fn order_status(&self, exchange_order_id: &str) -> Result<ExchangeOrder> {
        let fields = [("txid", exchange_order_id.to_string())];
        let result = self.private_post("/0/private/QueryOrders", &fields)?;
        let order = result.get(exchange_order_id).ok_or_else(|| {
            BotError::Exchange(format!("kraken order {exchange_order_id} not found"))
        })?;

        Self::exchange_order_from_value(exchange_order_id, order)
    }

    fn order_status_by_client_id(&self, client_order_id: &str) -> Result<Option<ExchangeOrder>> {
        if let Some(order) =
            self.find_order_by_client_id_in("/0/private/OpenOrders", "open", client_order_id)?
        {
            return Ok(Some(order));
        }

        self.find_order_by_client_id_in("/0/private/ClosedOrders", "closed", client_order_id)
    }

    fn cancel_order(&mut self, exchange_order_id: &str) -> Result<ExchangeOrder> {
        let fields = [("txid", exchange_order_id.to_string())];
        self.private_post("/0/private/CancelOrder", &fields)?;

        Ok(ExchangeOrder {
            exchange_order_id: exchange_order_id.to_string(),
            client_order_id: String::new(),
            status: OrderStatus::Cancelled,
        })
    }
}

fn kraken_status(status: &str) -> OrderStatus {
    match status {
        "closed" => OrderStatus::Filled,
        "canceled" | "cancelled" | "expired" => OrderStatus::Cancelled,
        "open" | "pending" => OrderStatus::Submitted,
        _ => OrderStatus::Rejected,
    }
}

fn kraken_asset_codes(currency: &str) -> Vec<String> {
    match currency.to_ascii_uppercase().as_str() {
        "BTC" | "XBT" => vec!["XXBT".to_string(), "XBT".to_string()],
        "ETH" => vec!["XETH".to_string(), "ETH".to_string()],
        "USD" => vec!["ZUSD".to_string(), "USD".to_string()],
        "EUR" => vec!["ZEUR".to_string(), "EUR".to_string()],
        "GBP" => vec!["ZGBP".to_string(), "GBP".to_string()],
        value => vec![value.to_string()],
    }
}

fn nonce() -> Result<String> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            BotError::Exchange(format!("system clock is before unix epoch: {error}"))
        })?;

    Ok(duration.as_millis().to_string())
}

fn kraken_signature(path: &str, body: &str, nonce: &str, secret: &str) -> Result<String> {
    let secret = BASE64
        .decode(secret)
        .map_err(|error| BotError::Exchange(format!("kraken api secret is not base64: {error}")))?;
    let hash = Sha256::digest(format!("{nonce}{body}").as_bytes());
    let mut mac = HmacSha512::new_from_slice(&secret)
        .map_err(|error| BotError::Exchange(format!("failed to create kraken hmac: {error}")))?;
    mac.update(path.as_bytes());
    mac.update(&hash);
    Ok(BASE64.encode(mac.finalize().into_bytes()))
}

fn form_encode(fields: &[(&str, String)]) -> String {
    fields
        .iter()
        .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::{form_encode, kraken_signature, kraken_status};
    use crate::orders::OrderStatus;

    #[test]
    fn maps_kraken_statuses_to_order_lifecycle() {
        assert_eq!(kraken_status("open"), OrderStatus::Submitted);
        assert_eq!(kraken_status("closed"), OrderStatus::Filled);
        assert_eq!(kraken_status("canceled"), OrderStatus::Cancelled);
    }

    #[test]
    fn form_encoding_escapes_reserved_characters() {
        let encoded = form_encode(&[
            ("nonce", "123".to_string()),
            ("cl_ord_id", "trd-1".to_string()),
            ("note", "a b+c".to_string()),
        ]);

        assert_eq!(encoded, "nonce=123&cl_ord_id=trd-1&note=a%20b%2Bc");
    }

    #[test]
    fn signs_payload_with_kraken_reference_vector() {
        let signature = kraken_signature(
            "/0/private/AddOrder",
            "nonce=1616492376594&ordertype=limit&pair=XBTUSD&price=37500&type=buy&volume=1.25",
            "1616492376594",
            "kQH5HW/8p1uGOVjbgWA7FunAmGO8lsSUXNsu3eow76sz84Q18fWxnyRzBHCd3pd5nE9qa99HAZtuZuj6F1huXg==",
        )
        .expect("signature should generate");

        assert_eq!(
            signature,
            "4/dpxb3iT4tp/ZCVEwSnEsLxx0bqyhLpdfOpc6fn7OR8+UClSV5n9E6aSS8MPtnRfp32bAb0nmbRn6H8ndwLUQ=="
        );
    }
}
