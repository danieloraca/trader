use crate::error::{BotError, Result};
use crate::exchange::Exchange;
use crate::orders::{Order, OrderRequest, OrderStatus, Side};
use crate::portfolio::Portfolio;

pub struct PaperExchange {
    portfolio: Portfolio,
    next_order_id: u64,
}

impl PaperExchange {
    pub fn new(portfolio: Portfolio) -> Self {
        Self {
            portfolio,
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

    fn place_order(&mut self, request: OrderRequest) -> Result<Order> {
        let quote_value = request.quote_value();

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

        Ok(Order {
            id: self.next_id(),
            request,
            status: OrderStatus::Filled,
        })
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
        }
    }

    fn sell_request(quantity_base: f64, limit_price: f64) -> OrderRequest {
        OrderRequest {
            symbol: "BTC-USD".to_string(),
            side: Side::Sell,
            quantity_base,
            limit_price,
        }
    }

    #[test]
    fn buy_order_updates_balances_and_assigns_id() {
        let portfolio = Portfolio::new("BTC", "USD", 1_000.0);
        let mut exchange = PaperExchange::new(portfolio);

        let order = exchange
            .place_order(buy_request(0.5, 100.0))
            .expect("buy should fill");

        assert_eq!(order.id, 1);
        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(exchange.portfolio().base_balance, 0.5);
        assert_eq!(exchange.portfolio().quote_balance, 950.0);
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

        assert_eq!(order.id, 2);
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
}
