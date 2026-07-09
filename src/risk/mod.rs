use crate::config::RiskConfig;
use crate::error::{BotError, Result};
use crate::orders::{OrderRequest, Side};
use crate::portfolio::Portfolio;
use crate::strategy::Signal;
use tracing::info;

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
            client_order_id: None,
        };

        if request.quote_value() > self.config.max_order_quote_value {
            return Err(BotError::Risk(format!(
                "signal rejected: order value {} exceeds max {}",
                request.quote_value(),
                self.config.max_order_quote_value
            )));
        }

        if request.side == Side::Buy
            && portfolio.base_balance + request.quantity_base > self.config.max_position_base
        {
            return Err(BotError::Risk(format!(
                "signal rejected: resulting position {} exceeds max {}",
                portfolio.base_balance + request.quantity_base,
                self.config.max_position_base
            )));
        }

        if request.side == Side::Sell && portfolio.base_balance < request.quantity_base {
            return Err(BotError::Risk(format!(
                "signal rejected: sell quantity {} exceeds position {}",
                request.quantity_base, portfolio.base_balance
            )));
        }

        info!(
            symbol = %signal.symbol,
            side = ?signal.side,
            quantity_base = %signal.quantity_base,
            price = %signal.price,
            reason = %signal.reason,
            "approved signal"
        );
        Ok(request)
    }
}

#[cfg(test)]
mod tests {
    use super::RiskManager;
    use crate::config::RiskConfig;
    use crate::decimal::Decimal;
    use crate::orders::Side;
    use crate::portfolio::Portfolio;
    use crate::strategy::Signal;

    fn risk_manager() -> RiskManager {
        RiskManager::new(RiskConfig {
            max_order_quote_value: Decimal::from_f64(500.0).expect("decimal should parse"),
            max_position_base: Decimal::from_f64(0.25).expect("decimal should parse"),
        })
    }

    fn portfolio(base_balance: f64) -> Portfolio {
        let mut portfolio = Portfolio::new(
            "BTC",
            "USD",
            Decimal::from_f64(10_000.0).expect("decimal should parse"),
        );
        portfolio.base_balance = Decimal::from_f64(base_balance).expect("decimal should parse");
        portfolio
    }

    fn signal(side: Side, quantity_base: f64, price: f64) -> Signal {
        Signal {
            symbol: "BTC-USD".to_string(),
            side,
            quantity_base: Decimal::from_f64(quantity_base).expect("decimal should parse"),
            price: Decimal::from_f64(price).expect("decimal should parse"),
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
        assert_eq!(request.quantity_base.to_string(), "0.01");
        assert_eq!(request.limit_price.to_string(), "100");
    }

    #[test]
    fn rejects_order_above_quote_limit() {
        let error = risk_manager()
            .approve(&signal(Side::Buy, 1.0, 501.0), &portfolio(0.0))
            .expect_err("signal should be rejected");

        assert!(error.to_string().contains("order value 501 exceeds max"));
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
