#[derive(Debug, Clone)]
pub struct PriceTick {
    pub symbol: String,
    pub price: f64,
}

impl PriceTick {
    pub fn new(symbol: &str, price: f64) -> Self {
        Self {
            symbol: symbol.to_string(),
            price,
        }
    }
}
