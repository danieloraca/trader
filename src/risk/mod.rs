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

#[cfg(test)]
mod tests {
    use super::RiskManager;
    use crate::config::RiskConfig;
    use crate::orders::Side;
    use crate::portfolio::Portfolio;
    use crate::strategy::Signal;

    fn risk_manager() -> RiskManager {
        RiskManager::new(RiskConfig {
            max_order_quote_value: 500.0,
            max_position_base: 0.25,
        })
    }

    fn portfolio(base_balance: f64) -> Portfolio {
        let mut portfolio = Portfolio::new("BTC", "USD", 10_000.0);
        portfolio.base_balance = base_balance;
        portfolio
    }

    fn signal(side: Side, quantity_base: f64, price: f64) -> Signal {
        Signal {
            symbol: "BTC-USD".to_string(),
            side,
            quantity_base,
            price,
            reason: "test signal".to_string(),
        }
    }

    #[test]
    fn approves_order_within_limits() {
        let request = risk_manager()
            .approve(&signal(Side::Buy, 0.01, 100.0), &portfolio(0.0))
            .expect("signal should be approved");

        assert_eq!(request.symbol, "BTC-USD");
        assert_eq!(request.side, Side::Buy);
        assert_eq!(request.quantity_base, 0.01);
        assert_eq!(request.limit_price, 100.0);
    }

    #[test]
    fn rejects_order_above_quote_limit() {
        let error = risk_manager()
            .approve(&signal(Side::Buy, 1.0, 501.0), &portfolio(0.0))
            .expect_err("signal should be rejected");

        assert!(error.to_string().contains("order value 501.00 exceeds max"));
    }

    #[test]
    fn rejects_buy_that_exceeds_position_limit() {
        let error = risk_manager()
            .approve(&signal(Side::Buy, 0.02, 100.0), &portfolio(0.24))
            .expect_err("signal should be rejected");

        assert!(error.to_string().contains("resulting position"));
    }

    #[test]
    fn rejects_sell_above_current_position() {
        let error = risk_manager()
            .approve(&signal(Side::Sell, 0.01, 100.0), &portfolio(0.005))
            .expect_err("signal should be rejected");

        assert!(error.to_string().contains("sell quantity"));
    }
}
