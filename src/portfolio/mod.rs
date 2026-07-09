use crate::decimal::Decimal;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone)]
pub struct Portfolio {
    pub base_currency: String,
    pub quote_currency: String,
    pub base_balance: Decimal,
    pub quote_balance: Decimal,
}

impl Portfolio {
    pub fn new(base_currency: &str, quote_currency: &str, quote_balance: Decimal) -> Self {
        Self {
            base_currency: base_currency.to_string(),
            quote_currency: quote_currency.to_string(),
            base_balance: Decimal::ZERO,
            quote_balance,
        }
    }
}

impl Display for Portfolio {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {}, {} {}",
            self.base_currency, self.base_balance, self.quote_currency, self.quote_balance
        )
    }
}
