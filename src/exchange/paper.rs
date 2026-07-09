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
