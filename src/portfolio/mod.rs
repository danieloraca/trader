use std::fmt::{Display, Formatter};

#[derive(Debug, Clone)]
pub struct Portfolio {
    pub base_currency: String,
    pub quote_currency: String,
    pub base_balance: f64,
    pub quote_balance: f64,
}

impl Portfolio {
    pub fn new(base_currency: &str, quote_currency: &str, quote_balance: f64) -> Self {
        Self {
            base_currency: base_currency.to_string(),
            quote_currency: quote_currency.to_string(),
            base_balance: 0.0,
            quote_balance,
        }
    }
}

impl Display for Portfolio {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {:.8}, {} {:.2}",
            self.base_currency, self.base_balance, self.quote_currency, self.quote_balance
        )
    }
}
