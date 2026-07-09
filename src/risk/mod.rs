use crate::config::RiskConfig;
use crate::error::{BotError, Result};
use crate::orders::{OrderRequest, Side};
use crate::portfolio::Portfolio;
use crate::strategy::Signal;

pub struct RiskManager {
    config: RiskConfig,
}

impl RiskManager {
    pub fn new(config: RiskConfig) -> Self {
        Self { config }
    }

    pub fn approve(&self, signal: &Signal, portfolio: &Portfolio) -> Result<OrderRequest> {
        let request = OrderRequest {
            symbol: signal.symbol.clone(),
            side: signal.side,
            quantity_base: signal.quantity_base,
            limit_price: signal.price,
        };

        if request.quote_value() > self.config.max_order_quote_value {
            return Err(BotError::Risk(format!(
                "signal rejected: order value {:.2} exceeds max {:.2}",
                request.quote_value(),
                self.config.max_order_quote_value
            )));
        }

        if request.side == Side::Buy
            && portfolio.base_balance + request.quantity_base > self.config.max_position_base
        {
            return Err(BotError::Risk(format!(
                "signal rejected: resulting position {:.8} exceeds max {:.8}",
                portfolio.base_balance + request.quantity_base,
                self.config.max_position_base
            )));
        }

        if request.side == Side::Sell && portfolio.base_balance < request.quantity_base {
            return Err(BotError::Risk(format!(
                "signal rejected: sell quantity {:.8} exceeds position {:.8}",
                request.quantity_base, portfolio.base_balance
            )));
        }

        println!("approved signal: {}", signal.reason);
        Ok(request)
    }
}
