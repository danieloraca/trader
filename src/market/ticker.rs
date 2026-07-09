use crate::decimal::Decimal;

#[derive(Debug, Clone)]
pub struct PriceTick {
    pub symbol: String,
    pub price: Decimal,
}

impl PriceTick {
    pub fn new(symbol: &str, price: Decimal) -> Self {
        Self {
            symbol: symbol.to_string(),
            price,
        }
    }
}
