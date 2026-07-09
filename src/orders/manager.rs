use crate::error::{BotError, Result};
use crate::exchange::Exchange;
use crate::orders::{Order, OrderRequest, OrderStatus};

pub struct OrderManager {
    next_order_id: u64,
}

impl OrderManager {
    pub fn new() -> Self {
        Self { next_order_id: 1 }
    }

    pub fn submit_order(
        &mut self,
        exchange: &mut impl Exchange,
        request: OrderRequest,
    ) -> Result<Vec<Order>> {
        let order_id = self.next_id();
        let mut transitions = vec![Order::submitted(order_id, request.clone())];

        match exchange.place_order(request.clone()) {
            Ok(exchange_order) => {
                let final_order = match exchange_order.status {
                    OrderStatus::Filled => {
                        Order::filled(order_id, exchange_order.exchange_order_id, request)
                    }
                    OrderStatus::Rejected => {
                        Order::rejected(order_id, request, "exchange rejected order".to_string())
                    }
                    status => {
                        return Err(BotError::Exchange(format!(
                            "paper exchange returned unsupported terminal status: {status:?}"
                        )));
                    }
                };
                transitions.push(final_order);
            }
            Err(BotError::Exchange(message)) => {
                transitions.push(Order::rejected(order_id, request, message));
            }
            Err(error) => return Err(error),
        }

        Ok(transitions)
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_order_id;
        self.next_order_id += 1;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::OrderManager;
    use crate::exchange::{Exchange, PaperExchange};
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

    #[test]
    fn records_submitted_and_filled_transitions() {
        let portfolio = Portfolio::new("BTC", "USD", 1_000.0);
        let mut exchange = PaperExchange::new(portfolio);
        let mut manager = OrderManager::new();

        let transitions = manager
            .submit_order(&mut exchange, buy_request(0.5, 100.0))
            .expect("order should submit");

        assert_eq!(transitions.len(), 2);
        assert_eq!(transitions[0].id, 1);
        assert_eq!(transitions[0].status, OrderStatus::Submitted);
        assert_eq!(transitions[0].exchange_order_id, None);
        assert_eq!(transitions[1].id, 1);
        assert_eq!(transitions[1].status, OrderStatus::Filled);
        assert_eq!(transitions[1].exchange_order_id, Some(1));
        assert_eq!(exchange.portfolio().base_balance, 0.5);
        assert_eq!(exchange.portfolio().quote_balance, 950.0);
    }

    #[test]
    fn records_rejected_transition_for_exchange_rejection() {
        let portfolio = Portfolio::new("BTC", "USD", 10.0);
        let mut exchange = PaperExchange::new(portfolio);
        let mut manager = OrderManager::new();

        let transitions = manager
            .submit_order(&mut exchange, buy_request(1.0, 100.0))
            .expect("exchange rejection should be captured");

        assert_eq!(transitions.len(), 2);
        assert_eq!(transitions[0].status, OrderStatus::Submitted);
        assert_eq!(transitions[1].status, OrderStatus::Rejected);
        assert!(
            transitions[1]
                .status_reason
                .as_ref()
                .expect("rejection should have a reason")
                .contains("insufficient quote balance")
        );
        assert_eq!(exchange.portfolio().base_balance, 0.0);
        assert_eq!(exchange.portfolio().quote_balance, 10.0);
    }
}
