use crate::error::{BotError, Result};
use crate::exchange::Exchange;
use crate::orders::{ExchangeOrder, OrderRequest, OrderStatus, Side};
use crate::portfolio::Portfolio;
use std::collections::HashMap;

pub struct PaperExchange {
    portfolio: Portfolio,
    orders: HashMap<u64, ExchangeOrder>,
    next_order_id: u64,
}

impl PaperExchange {
    pub fn new(portfolio: Portfolio) -> Self {
        Self {
            portfolio,
            orders: HashMap::new(),
            next_order_id: 1,
        }
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_order_id;
        self.next_order_id += 1;
        id
    }
}

impl Exchange for PaperExchange {
    fn portfolio(&self) -> &Portfolio {
        &self.portfolio
    }

    fn sync_portfolio(&self) -> Result<Portfolio> {
        Ok(self.portfolio.clone())
    }

    fn place_order(&mut self, request: OrderRequest) -> Result<ExchangeOrder> {
        let quote_value = request.quote_value();
        let client_order_id = request.client_order_id.clone().ok_or_else(|| {
            BotError::Exchange("order request missing client order id".to_string())
        })?;

        match request.side {
            Side::Buy if self.portfolio.quote_balance < quote_value => {
                return Err(BotError::Exchange(format!(
                    "insufficient quote balance for order value {quote_value:.2}"
                )));
            }
            Side::Sell if self.portfolio.base_balance < request.quantity_base => {
                return Err(BotError::Exchange(format!(
                    "insufficient base balance for quantity {:.8}",
                    request.quantity_base
                )));
            }
            _ => {}
        }

        match request.side {
            Side::Buy => {
                self.portfolio.quote_balance -= quote_value;
                self.portfolio.base_balance += request.quantity_base;
            }
            Side::Sell => {
                self.portfolio.base_balance -= request.quantity_base;
                self.portfolio.quote_balance += quote_value;
            }
        }

        let order = ExchangeOrder {
            exchange_order_id: self.next_id(),
            client_order_id,
            status: OrderStatus::Filled,
        };
        self.orders.insert(order.exchange_order_id, order.clone());

        Ok(order)
    }

    fn order_status(&self, exchange_order_id: u64) -> Result<ExchangeOrder> {
        self.orders
            .get(&exchange_order_id)
            .cloned()
            .ok_or_else(|| BotError::Exchange(format!("unknown order id {exchange_order_id}")))
    }

    fn cancel_order(&mut self, exchange_order_id: u64) -> Result<ExchangeOrder> {
        let order = self.order_status(exchange_order_id)?;

        match order.status {
            OrderStatus::Submitted => {
                let cancelled_order = ExchangeOrder {
                    exchange_order_id,
                    client_order_id: order.client_order_id,
                    status: OrderStatus::Cancelled,
                };
                self.orders
                    .insert(exchange_order_id, cancelled_order.clone());
                Ok(cancelled_order)
            }
            OrderStatus::Cancelled => Ok(order),
            OrderStatus::Filled => Err(BotError::Exchange(format!(
                "cannot cancel filled order {exchange_order_id}"
            ))),
            OrderStatus::Rejected => Err(BotError::Exchange(format!(
                "cannot cancel rejected order {exchange_order_id}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PaperExchange;
    use crate::exchange::Exchange;
    use crate::orders::{OrderRequest, OrderStatus, Side};
    use crate::portfolio::Portfolio;

    fn buy_request(quantity_base: f64, limit_price: f64) -> OrderRequest {
        OrderRequest {
            symbol: "BTC-USD".to_string(),
            side: Side::Buy,
            quantity_base,
            limit_price,
            client_order_id: Some("test-client-order".to_string()),
        }
    }

    fn sell_request(quantity_base: f64, limit_price: f64) -> OrderRequest {
        OrderRequest {
            symbol: "BTC-USD".to_string(),
            side: Side::Sell,
            quantity_base,
            limit_price,
            client_order_id: Some("test-client-order".to_string()),
        }
    }

    #[test]
    fn buy_order_updates_balances_and_assigns_id() {
        let portfolio = Portfolio::new("BTC", "USD", 1_000.0);
        let mut exchange = PaperExchange::new(portfolio);

        let order = exchange
            .place_order(buy_request(0.5, 100.0))
            .expect("buy should fill");

        assert_eq!(order.exchange_order_id, 1);
        assert_eq!(order.client_order_id, "test-client-order");
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(exchange.portfolio().base_balance, 0.5);
        assert_eq!(exchange.portfolio().quote_balance, 950.0);
    }

    #[test]
    fn sync_portfolio_returns_latest_balances() {
        let portfolio = Portfolio::new("BTC", "USD", 1_000.0);
        let mut exchange = PaperExchange::new(portfolio);

        exchange
            .place_order(buy_request(0.5, 100.0))
            .expect("buy should fill");
        let synced = exchange.sync_portfolio().expect("sync should work");

        assert_eq!(synced.base_balance, 0.5);
        assert_eq!(synced.quote_balance, 950.0);
    }

    #[test]
    fn sell_order_updates_balances_and_increments_id() {
        let portfolio = Portfolio::new("BTC", "USD", 1_000.0);
        let mut exchange = PaperExchange::new(portfolio);

        exchange
            .place_order(buy_request(0.5, 100.0))
            .expect("buy should fill");
        let order = exchange
            .place_order(sell_request(0.2, 110.0))
            .expect("sell should fill");

        assert_eq!(order.exchange_order_id, 2);
        assert_eq!(exchange.portfolio().base_balance, 0.3);
        assert_eq!(exchange.portfolio().quote_balance, 972.0);
    }

    #[test]
    fn rejects_buy_with_insufficient_quote_balance() {
        let portfolio = Portfolio::new("BTC", "USD", 10.0);
        let mut exchange = PaperExchange::new(portfolio);

        let error = exchange
            .place_order(buy_request(1.0, 100.0))
            .expect_err("buy should fail");

        assert!(error.to_string().contains("insufficient quote balance"));
        assert_eq!(exchange.portfolio().base_balance, 0.0);
        assert_eq!(exchange.portfolio().quote_balance, 10.0);
    }

    #[test]
    fn rejects_order_without_client_order_id() {
        let portfolio = Portfolio::new("BTC", "USD", 1_000.0);
        let mut exchange = PaperExchange::new(portfolio);
        let mut request = buy_request(0.5, 100.0);
        request.client_order_id = None;

        let error = exchange
            .place_order(request)
            .expect_err("order should fail");

        assert!(error.to_string().contains("missing client order id"));
        assert_eq!(exchange.portfolio().base_balance, 0.0);
        assert_eq!(exchange.portfolio().quote_balance, 1_000.0);
    }

    #[test]
    fn rejects_sell_with_insufficient_base_balance() {
        let portfolio = Portfolio::new("BTC", "USD", 1_000.0);
        let mut exchange = PaperExchange::new(portfolio);

        let error = exchange
            .place_order(sell_request(0.1, 100.0))
            .expect_err("sell should fail");

        assert!(error.to_string().contains("insufficient base balance"));
        assert_eq!(exchange.portfolio().base_balance, 0.0);
        assert_eq!(exchange.portfolio().quote_balance, 1_000.0);
    }

    #[test]
    fn polls_order_status_for_known_order() {
        let portfolio = Portfolio::new("BTC", "USD", 1_000.0);
        let mut exchange = PaperExchange::new(portfolio);
        let order = exchange
            .place_order(buy_request(0.5, 100.0))
            .expect("buy should fill");

        let status = exchange
            .order_status(order.exchange_order_id)
            .expect("status should exist");

        assert_eq!(status.exchange_order_id, order.exchange_order_id);
        assert_eq!(status.status, OrderStatus::Filled);
    }

    #[test]
    fn rejects_status_poll_for_unknown_order() {
        let portfolio = Portfolio::new("BTC", "USD", 1_000.0);
        let exchange = PaperExchange::new(portfolio);

        let error = exchange
            .order_status(99)
            .expect_err("unknown order should fail");

        assert!(error.to_string().contains("unknown order id 99"));
    }

    #[test]
    fn rejects_cancel_for_filled_order() {
        let portfolio = Portfolio::new("BTC", "USD", 1_000.0);
        let mut exchange = PaperExchange::new(portfolio);
        let order = exchange
            .place_order(buy_request(0.5, 100.0))
            .expect("buy should fill");

        let error = exchange
            .cancel_order(order.exchange_order_id)
            .expect_err("filled order cannot cancel");

        assert!(error.to_string().contains("cannot cancel filled order"));
    }
}
