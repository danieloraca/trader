use crate::decimal::Decimal;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FuturesPositionSide {
    Flat,
    Long,
    Short,
}

impl FuturesPositionSide {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Flat => "flat",
            Self::Long => "long",
            Self::Short => "short",
        }
    }

    pub fn from_str(value: &str) -> Self {
        match value {
            "long" => Self::Long,
            "short" => Self::Short,
            _ => Self::Flat,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Portfolio {
    pub base_currency: String,
    pub quote_currency: String,
    pub base_balance: Decimal,
    pub quote_balance: Decimal,
    pub futures_enabled: bool,
    pub futures_position_side: FuturesPositionSide,
    pub futures_position_base: Decimal,
    pub futures_entry_price: Decimal,
    pub futures_margin_used_quote: Decimal,
    pub futures_realized_pnl_quote: Decimal,
}

impl Portfolio {
    pub fn new(base_currency: &str, quote_currency: &str, quote_balance: Decimal) -> Self {
        Self {
            base_currency: base_currency.to_string(),
            quote_currency: quote_currency.to_string(),
            base_balance: Decimal::ZERO,
            quote_balance,
            futures_enabled: false,
            futures_position_side: FuturesPositionSide::Flat,
            futures_position_base: Decimal::ZERO,
            futures_entry_price: Decimal::ZERO,
            futures_margin_used_quote: Decimal::ZERO,
            futures_realized_pnl_quote: Decimal::ZERO,
        }
    }

    pub fn paper_futures(
        base_currency: &str,
        quote_currency: &str,
        quote_balance: Decimal,
    ) -> Self {
        let mut portfolio = Self::new(base_currency, quote_currency, quote_balance);
        portfolio.futures_enabled = true;
        portfolio
    }

    #[allow(dead_code)]
    pub fn unrealized_futures_pnl(&self, mark_price: Decimal) -> Decimal {
        match self.futures_position_side {
            FuturesPositionSide::Flat => Decimal::ZERO,
            FuturesPositionSide::Long => {
                (mark_price - self.futures_entry_price) * self.futures_position_base
            }
            FuturesPositionSide::Short => {
                (self.futures_entry_price - mark_price) * self.futures_position_base
            }
        }
    }

    #[allow(dead_code)]
    pub fn equity_with_mark(&self, mark_price: Decimal) -> Decimal {
        if self.futures_enabled {
            self.quote_balance + self.unrealized_futures_pnl(mark_price)
        } else {
            self.quote_balance + (self.base_balance * mark_price)
        }
    }
}

impl Display for Portfolio {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.futures_enabled {
            write!(
                f,
                "{} {} futures {} @ {}, margin {}, {} equity cash {}",
                self.base_currency,
                self.futures_position_side.as_str(),
                self.futures_position_base,
                self.futures_entry_price,
                self.futures_margin_used_quote,
                self.quote_currency,
                self.quote_balance
            )
        } else {
            write!(
                f,
                "{} {}, {} {}",
                self.base_currency, self.base_balance, self.quote_currency, self.quote_balance
            )
        }
    }
}
