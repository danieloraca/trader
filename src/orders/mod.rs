mod manager;

pub use manager::OrderManager;

use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderStatus {
    Submitted,
    Filled,
    Rejected,
    #[allow(dead_code)]
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct OrderRequest {
    pub symbol: String,
    pub side: Side,
    pub quantity_base: f64,
    pub limit_price: f64,
    pub client_order_id: Option<String>,
}

impl OrderRequest {
    pub fn quote_value(&self) -> f64 {
        self.quantity_base * self.limit_price
    }
}

#[derive(Debug, Clone)]
pub struct Order {
    pub id: u64,
    pub exchange_order_id: Option<u64>,
    pub request: OrderRequest,
    pub status: OrderStatus,
    pub status_reason: Option<String>,
}

impl Order {
    pub fn submitted(id: u64, request: OrderRequest) -> Self {
        Self {
            id,
            exchange_order_id: None,
            request,
            status: OrderStatus::Submitted,
            status_reason: None,
        }
    }

    pub fn filled(id: u64, exchange_order_id: u64, request: OrderRequest) -> Self {
        Self {
            id,
            exchange_order_id: Some(exchange_order_id),
            request,
            status: OrderStatus::Filled,
            status_reason: None,
        }
    }

    pub fn rejected(id: u64, request: OrderRequest, reason: String) -> Self {
        Self {
            id,
            exchange_order_id: None,
            request,
            status: OrderStatus::Rejected,
            status_reason: Some(reason),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExchangeOrder {
    pub exchange_order_id: u64,
    pub client_order_id: String,
    pub status: OrderStatus,
}

impl Display for Order {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "#{} {:?} {} {} @ {} ({:?})",
            self.id,
            self.request.side,
            self.request.quantity_base,
            self.request.symbol,
            self.request.limit_price,
            self.status
        )
    }
}
