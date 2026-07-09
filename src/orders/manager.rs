use crate::error::{BotError, Result};
use crate::exchange::Exchange;
use crate::orders::{Order, OrderRequest, OrderStatus};

pub struct OrderManager {
    next_order_id: u64,
}

impl OrderManager {
    pub fn new_at(next_order_id: u64) -> Self {
        Self { next_order_id }
    }

    pub fn next_order_id(&self) -> u64 {
        self.next_order_id
    }

    pub fn prepare_order(&mut self, mut request: OrderRequest) -> Order {
        let order_id = self.next_id();
        request.client_order_id = Some(client_order_id(order_id));
        Order::submitted(order_id, request)
    }

    pub fn submit_prepared_order(
        &self,
        exchange: &mut (impl Exchange + ?Sized),
        submitted_order: &Order,
    ) -> Result<Order> {
        if submitted_order.status != OrderStatus::Submitted {
            return Err(BotError::Exchange(format!(
                "cannot submit local order {} with status {:?}",
                submitted_order.id, submitted_order.status
            )));
        }

        let request = submitted_order.request.clone();

        match exchange.place_order(request.clone()) {
            Ok(exchange_order) => match exchange_order.status {
                OrderStatus::Filled => Ok(Order::filled(
                    submitted_order.id,
                    exchange_order.exchange_order_id,
                    request,
                )),
                OrderStatus::Rejected => Ok(Order::rejected(
                    submitted_order.id,
                    request,
                    "exchange rejected order".to_string(),
                )),
                OrderStatus::Submitted => Ok(Order {
                    id: submitted_order.id,
                    exchange_order_id: Some(exchange_order.exchange_order_id),
                    request,
                    status: OrderStatus::Submitted,
                    status_reason: None,
                }),
                status => Err(BotError::Exchange(format!(
                    "exchange returned unsupported order status: {status:?}"
                ))),
            },
            Err(BotError::Exchange(message)) => {
                Ok(Order::rejected(submitted_order.id, request, message))
            }
            Err(error) => return Err(error),
        }
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_order_id;
        self.next_order_id += 1;
        id
    }
}

fn client_order_id(order_id: u64) -> String {
    format!("trd-{order_id}")
}

#[cfg(test)]
mod tests {
    use super::OrderManager;
    use crate::decimal::Decimal;
    use crate::exchange::{Exchange, PaperExchange};
    use crate::orders::{OrderRequest, OrderStatus, Side};
    use crate::portfolio::Portfolio;

    fn buy_request(quantity_base: f64, limit_price: f64) -> OrderRequest {
        OrderRequest {
            symbol: "BTC-USD".to_string(),
            side: Side::Buy,
            quantity_base: Decimal::from_f64(quantity_base).expect("decimal should parse"),
            limit_price: Decimal::from_f64(limit_price).expect("decimal should parse"),
            client_order_id: None,
        }
    }

    #[test]
    fn records_submitted_and_filled_transitions() {
        let portfolio = Portfolio::new(
            "BTC",
            "USD",
            Decimal::from_f64(1_000.0).expect("decimal should parse"),
        );
        let mut exchange = PaperExchange::new(portfolio);
        let mut manager = OrderManager::new_at(1);

        let submitted = manager.prepare_order(buy_request(0.5, 100.0));
        assert_eq!(submitted.id, 1);
        assert_eq!(submitted.request.client_order_id.as_deref(), Some("trd-1"));
        assert_eq!(submitted.status, OrderStatus::Submitted);
        assert_eq!(submitted.exchange_order_id, None);

        let filled = manager
            .submit_prepared_order(&mut exchange, &submitted)
            .expect("order should submit");

        assert_eq!(filled.id, 1);
        assert_eq!(filled.request.client_order_id.as_deref(), Some("trd-1"));
        assert_eq!(filled.status, OrderStatus::Filled);
        assert_eq!(filled.exchange_order_id.as_deref(), Some("1"));
        assert_eq!(exchange.portfolio().base_balance.to_string(), "0.5");
        assert_eq!(exchange.portfolio().quote_balance.to_string(), "950");
    }

    #[test]
    fn records_rejected_transition_for_exchange_rejection() {
        let portfolio = Portfolio::new(
            "BTC",
            "USD",
            Decimal::from_f64(10.0).expect("decimal should parse"),
        );
        let mut exchange = PaperExchange::new(portfolio);
        let mut manager = OrderManager::new_at(1);

        let submitted = manager.prepare_order(buy_request(1.0, 100.0));
        let rejected = manager
            .submit_prepared_order(&mut exchange, &submitted)
            .expect("exchange rejection should be captured");

        assert_eq!(submitted.status, OrderStatus::Submitted);
        assert_eq!(rejected.status, OrderStatus::Rejected);
        assert!(
            rejected
                .status_reason
                .as_ref()
                .expect("rejection should have a reason")
                .contains("insufficient quote balance")
        );
        assert_eq!(exchange.portfolio().base_balance, Decimal::ZERO);
        assert_eq!(exchange.portfolio().quote_balance.to_string(), "10");
    }
}
