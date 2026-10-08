use crate::config::RiskConfig;
use crate::decimal::Decimal;
use crate::error::{BotError, Result};
use crate::orders::OrderRequest;
use crate::portfolio::{FuturesPositionSide, Portfolio};
use crate::strategy::{Signal, SignalIntent};
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
        let quote_value = request.checked_quote_value().ok_or_else(|| {
            BotError::Risk(
                "signal rejected: order value is outside the supported range".to_string(),
            )
        })?;
        if request.quantity_base <= Decimal::ZERO
            || request.limit_price <= Decimal::ZERO
            || quote_value <= Decimal::ZERO
        {
            return Err(BotError::Risk(
                "signal rejected: order quantity, price, and value must be positive".to_string(),
            ));
        }

        if matches!(
            signal.intent,
            SignalIntent::IncreaseLong | SignalIntent::IncreaseShort
        ) && quote_value > self.config.max_order_quote_value
        {
            return Err(BotError::Risk(format!(
                "signal rejected: order value {} exceeds max {}",
                quote_value, self.config.max_order_quote_value
            )));
        }

        if portfolio.futures_enabled {
            self.approve_futures(signal, &request, portfolio)?;
        } else {
            self.approve_spot(signal, &request, portfolio)?;
        }

        info!(
            symbol = %signal.symbol,
            side = ?signal.side,
            intent = ?signal.intent,
            quantity_base = %signal.quantity_base,
            price = %signal.price,
            reason = %signal.reason,
            "approved signal"
        );
        Ok(request)
    }

    fn approve_spot(
        &self,
        signal: &Signal,
        request: &OrderRequest,
        portfolio: &Portfolio,
    ) -> Result<()> {
        if signal.intent == SignalIntent::IncreaseShort {
            return Err(BotError::Risk(
                "signal rejected: short entries require futures mode".to_string(),
            ));
        }

        let projected_position = portfolio.base_balance.checked_add(request.quantity_base);
        if signal.intent == SignalIntent::IncreaseLong
            && projected_position.is_none_or(|value| value > self.config.max_position_base)
        {
            return Err(BotError::Risk(format!(
                "signal rejected: resulting position exceeds max {} or supported range",
                self.config.max_position_base
            )));
        }

        if signal.intent == SignalIntent::DecreaseLong
            && portfolio.base_balance < request.quantity_base
        {
            return Err(BotError::Risk(format!(
                "signal rejected: sell quantity {} exceeds position {}",
                request.quantity_base, portfolio.base_balance
            )));
        }

        Ok(())
    }

    fn approve_futures(
        &self,
        signal: &Signal,
        request: &OrderRequest,
        portfolio: &Portfolio,
    ) -> Result<()> {
        match signal.intent {
            SignalIntent::IncreaseLong => {
                let projected_long = match portfolio.futures_position_side {
                    FuturesPositionSide::Long => {
                        portfolio
                            .futures_position_base
                            .checked_add(request.quantity_base)
                            .ok_or_else(|| {
                                BotError::Risk(
                                    "signal rejected: resulting long position is outside the supported range"
                                        .to_string(),
                                )
                            })?
                    }
                    FuturesPositionSide::Short => {
                        if request.quantity_base > portfolio.futures_position_base {
                            request.quantity_base - portfolio.futures_position_base
                        } else {
                            Decimal::ZERO
                        }
                    }
                    FuturesPositionSide::Flat => request.quantity_base,
                };

                if projected_long > self.config.max_position_base {
                    return Err(BotError::Risk(format!(
                        "signal rejected: resulting long position {} exceeds max {}",
                        projected_long, self.config.max_position_base
                    )));
                }
            }
            SignalIntent::DecreaseLong => {
                if portfolio.futures_position_side != FuturesPositionSide::Long {
                    return Err(BotError::Risk(
                        "signal rejected: no long futures position to reduce".to_string(),
                    ));
                }

                if request.quantity_base > portfolio.futures_position_base {
                    return Err(BotError::Risk(format!(
                        "signal rejected: reduce-long quantity {} exceeds position {}",
                        request.quantity_base, portfolio.futures_position_base
                    )));
                }
            }
            SignalIntent::IncreaseShort => {
                if !self.config.allow_short {
                    return Err(BotError::Risk(
                        "signal rejected: short entries are disabled for this account".to_string(),
                    ));
                }

                let projected_short = match portfolio.futures_position_side {
                    FuturesPositionSide::Short => {
                        portfolio
                            .futures_position_base
                            .checked_add(request.quantity_base)
                            .ok_or_else(|| {
                                BotError::Risk(
                                    "signal rejected: resulting short position is outside the supported range"
                                        .to_string(),
                                )
                            })?
                    }
                    FuturesPositionSide::Long => {
                        if request.quantity_base > portfolio.futures_position_base {
                            request.quantity_base - portfolio.futures_position_base
                        } else {
                            Decimal::ZERO
                        }
                    }
                    FuturesPositionSide::Flat => request.quantity_base,
                };

                if projected_short > self.config.max_short_position_base {
                    return Err(BotError::Risk(format!(
                        "signal rejected: resulting short position {} exceeds max short {}",
                        projected_short, self.config.max_short_position_base
                    )));
                }
            }
            SignalIntent::DecreaseShort => {
                if portfolio.futures_position_side != FuturesPositionSide::Short {
                    return Err(BotError::Risk(
                        "signal rejected: no short futures position to reduce".to_string(),
                    ));
                }

                if request.quantity_base > portfolio.futures_position_base {
                    return Err(BotError::Risk(format!(
                        "signal rejected: reduce-short quantity {} exceeds position {}",
                        request.quantity_base, portfolio.futures_position_base
                    )));
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::RiskManager;
    use crate::config::RiskConfig;
    use crate::decimal::Decimal;
    use crate::orders::Side;
    use crate::portfolio::Portfolio;
    use crate::strategy::{Signal, SignalIntent};

    fn risk_manager() -> RiskManager {
        RiskManager::new(RiskConfig {
            max_order_quote_value: Decimal::from_f64(500.0).expect("decimal should parse"),
            max_position_base: Decimal::from_f64(0.25).expect("decimal should parse"),
            allow_short: false,
            max_short_position_base: Decimal::ZERO,
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

    fn futures_portfolio() -> Portfolio {
        Portfolio::paper_futures(
            "BTC",
            "USD",
            Decimal::from_f64(10_000.0).expect("decimal should parse"),
        )
    }

    fn signal(side: Side, quantity_base: f64, price: f64) -> Signal {
        Signal {
            symbol: "BTC-USD".to_string(),
            side,
            intent: if side == Side::Buy {
                SignalIntent::IncreaseLong
            } else {
                SignalIntent::DecreaseLong
            },
            quantity_base: Decimal::from_f64(quantity_base).expect("decimal should parse"),
            price: Decimal::from_f64(price).expect("decimal should parse"),
            reason: "test signal".to_string(),
        }
    }

    #[test]
    fn rejects_short_entry_in_spot_mode() {
        let mut short_signal = signal(Side::Sell, 0.01, 100.0);
        short_signal.intent = SignalIntent::IncreaseShort;

        let error = risk_manager()
            .approve(&short_signal, &portfolio(0.0))
            .expect_err("short signal should be rejected");

        assert!(error.to_string().contains("short entries require futures"));
    }

    #[test]
    fn allows_exposure_reducing_futures_order_above_entry_value_limit() {
        let mut portfolio = futures_portfolio();
        portfolio.futures_position_side = crate::portfolio::FuturesPositionSide::Long;
        portfolio.futures_position_base = Decimal::from_f64(0.25).expect("quantity should parse");
        portfolio.futures_entry_price = Decimal::from_f64(10_000.0).expect("price should parse");
        let mut close_signal = signal(Side::Sell, 0.25, 10_000.0);
        close_signal.intent = SignalIntent::DecreaseLong;

        let request = risk_manager()
            .approve(&close_signal, &portfolio)
            .expect("risk-reducing close should be approved");

        assert_eq!(request.quantity_base, portfolio.futures_position_base);
        assert!(request.quote_value() > Decimal::from_f64(500.0).expect("limit should parse"));
    }

    #[test]
    fn rejects_futures_short_entry_when_shorting_is_disabled() {
        let mut short_signal = signal(Side::Sell, 0.01, 100.0);
        short_signal.intent = SignalIntent::IncreaseShort;

        let error = risk_manager()
            .approve(&short_signal, &futures_portfolio())
            .expect_err("short signal should be rejected");

        assert!(error.to_string().contains("short entries are disabled"));
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
    fn rejects_order_value_that_overflows_fixed_point_range() {
        let mut oversized = signal(Side::Buy, 1_000_000.0, 10_000_000.0);
        oversized.intent = SignalIntent::IncreaseLong;

        let error = risk_manager()
            .approve(&oversized, &portfolio(0.0))
            .expect_err("overflowing order should be rejected");

        assert!(error.to_string().contains("outside the supported range"));
    }

    #[test]
    fn rejects_order_value_rounded_down_to_zero() {
        let tiny = signal(Side::Buy, 0.000001, 0.000001);

        let error = risk_manager()
            .approve(&tiny, &portfolio(0.0))
            .expect_err("zero-value order should be rejected");

        assert!(error.to_string().contains("must be positive"));
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
