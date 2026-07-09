use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderStatus {
    Filled,
}

#[derive(Debug, Clone)]
pub struct OrderRequest {
    pub symbol: String,
    pub side: Side,
    pub quantity_base: f64,
    pub limit_price: f64,
}

impl OrderRequest {
    pub fn quote_value(&self) -> f64 {
        self.quantity_base * self.limit_price
    }
}

#[derive(Debug, Clone)]
pub struct Order {
    pub id: u64,
    pub request: OrderRequest,
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
