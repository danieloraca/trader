use crate::decimal::Decimal;
use crate::error::{BotError, Result};
use crate::exchange::Exchange;
use crate::orders::{ExchangeOrder, OrderRequest, OrderStatus, Side};
use crate::portfolio::{FuturesPositionSide, Portfolio};
use std::collections::HashMap;

pub struct PaperFuturesExchange {
    portfolio: Portfolio,
    leverage: Decimal,
    orders: HashMap<String, ExchangeOrder>,
    next_order_id: u64,
}

impl PaperFuturesExchange {
    pub fn new(mut portfolio: Portfolio, leverage: Decimal) -> Self {
        portfolio.futures_enabled = true;
        portfolio.base_balance = Decimal::ZERO;
        Self {
            portfolio,
            leverage,
            orders: HashMap::new(),
            next_order_id: 1,
        }
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_order_id;
        self.next_order_id += 1;
        id
    }

    fn apply_order(&mut self, request: &OrderRequest) -> Result<()> {
        let mut candidate = self.portfolio.clone();
        apply_futures_fill(&mut candidate, request, self.leverage)?;

        if candidate.futures_margin_used_quote > candidate.quote_balance {
            return Err(BotError::Exchange(format!(
                "insufficient futures equity: margin {} exceeds cash equity {}",
                candidate.futures_margin_used_quote, candidate.quote_balance
            )));
        }

        self.portfolio = candidate;
        Ok(())
    }
}

impl Exchange for PaperFuturesExchange {
    fn portfolio(&self) -> &Portfolio {
        &self.portfolio
    }

    fn sync_portfolio(&mut self) -> Result<Portfolio> {
        Ok(self.portfolio.clone())
    }

    fn place_order(&mut self, request: OrderRequest) -> Result<ExchangeOrder> {
        let client_order_id = request.client_order_id.clone().ok_or_else(|| {
            BotError::Exchange("order request missing client order id".to_string())
        })?;

        self.apply_order(&request)?;

        let exchange_order_id = self.next_id().to_string();
        let order = ExchangeOrder {
            exchange_order_id,
            client_order_id,
            status: OrderStatus::Filled,
        };
        self.orders
            .insert(order.exchange_order_id.clone(), order.clone());

        Ok(order)
    }

    fn order_status(&self, exchange_order_id: &str) -> Result<ExchangeOrder> {
        self.orders
            .get(exchange_order_id)
            .cloned()
            .ok_or_else(|| BotError::Exchange(format!("unknown order id {exchange_order_id}")))
    }

    fn order_status_by_client_id(&self, client_order_id: &str) -> Result<Option<ExchangeOrder>> {
        Ok(self
            .orders
            .values()
            .find(|order| order.client_order_id == client_order_id)
            .cloned())
    }

    fn cancel_order(&mut self, exchange_order_id: &str) -> Result<ExchangeOrder> {
        let order = self.order_status(exchange_order_id)?;

        match order.status {
            OrderStatus::Submitted => {
                let cancelled_order = ExchangeOrder {
                    exchange_order_id: exchange_order_id.to_string(),
                    client_order_id: order.client_order_id,
                    status: OrderStatus::Cancelled,
                };
                self.orders
                    .insert(exchange_order_id.to_string(), cancelled_order.clone());
                Ok(cancelled_order)
            }
            OrderStatus::Cancelled => Ok(order),
            OrderStatus::Filled => Err(BotError::Exchange(format!(
                "cannot cancel filled order {exchange_order_id}"
            ))),
            OrderStatus::Rejected => Err(BotError::Exchange(format!(
                "cannot cancel rejected order {exchange_order_id}"
            ))),
        }
    }
}

fn apply_futures_fill(
    portfolio: &mut Portfolio,
    request: &OrderRequest,
    leverage: Decimal,
) -> Result<()> {
    if request.quantity_base <= Decimal::ZERO {
        return Err(BotError::Exchange(
            "futures order quantity must be positive".to_string(),
        ));
    }

    match (portfolio.futures_position_side, request.side) {
        (FuturesPositionSide::Flat, Side::Buy) => {
            open_position(
                portfolio,
                FuturesPositionSide::Long,
                request.quantity_base,
                request.limit_price,
            );
        }
        (FuturesPositionSide::Flat, Side::Sell) => {
            open_position(
                portfolio,
                FuturesPositionSide::Short,
                request.quantity_base,
                request.limit_price,
            );
        }
        (FuturesPositionSide::Long, Side::Buy) => {
            increase_position(portfolio, request.quantity_base, request.limit_price);
        }
        (FuturesPositionSide::Short, Side::Sell) => {
            increase_position(portfolio, request.quantity_base, request.limit_price);
        }
        (FuturesPositionSide::Long, Side::Sell) => {
            reduce_or_flip_long(portfolio, request.quantity_base, request.limit_price);
        }
        (FuturesPositionSide::Short, Side::Buy) => {
            reduce_or_flip_short(portfolio, request.quantity_base, request.limit_price);
        }
    }

    refresh_margin(portfolio, leverage);
    Ok(())
}

fn open_position(
    portfolio: &mut Portfolio,
    side: FuturesPositionSide,
    quantity: Decimal,
    entry_price: Decimal,
) {
    portfolio.futures_position_side = side;
    portfolio.futures_position_base = quantity;
    portfolio.futures_entry_price = entry_price;
}

fn increase_position(portfolio: &mut Portfolio, quantity: Decimal, price: Decimal) {
    let old_notional = portfolio.futures_position_base * portfolio.futures_entry_price;
    let new_notional = quantity * price;
    let new_quantity = portfolio.futures_position_base + quantity;
    portfolio.futures_entry_price = (old_notional + new_notional) / new_quantity;
    portfolio.futures_position_base = new_quantity;
}

fn reduce_or_flip_long(portfolio: &mut Portfolio, quantity: Decimal, price: Decimal) {
    let close_quantity = min_decimal(quantity, portfolio.futures_position_base);
    let realized = (price - portfolio.futures_entry_price) * close_quantity;
    portfolio.quote_balance += realized;
    portfolio.futures_realized_pnl_quote += realized;

    if quantity < portfolio.futures_position_base {
        portfolio.futures_position_base -= quantity;
        return;
    }

    let remainder = quantity - close_quantity;
    clear_position(portfolio);
    if remainder > Decimal::ZERO {
        open_position(portfolio, FuturesPositionSide::Short, remainder, price);
    }
}

fn reduce_or_flip_short(portfolio: &mut Portfolio, quantity: Decimal, price: Decimal) {
    let close_quantity = min_decimal(quantity, portfolio.futures_position_base);
    let realized = (portfolio.futures_entry_price - price) * close_quantity;
    portfolio.quote_balance += realized;
    portfolio.futures_realized_pnl_quote += realized;

    if quantity < portfolio.futures_position_base {
        portfolio.futures_position_base -= quantity;
        return;
    }

    let remainder = quantity - close_quantity;
    clear_position(portfolio);
    if remainder > Decimal::ZERO {
        open_position(portfolio, FuturesPositionSide::Long, remainder, price);
    }
}

fn clear_position(portfolio: &mut Portfolio) {
    portfolio.futures_position_side = FuturesPositionSide::Flat;
    portfolio.futures_position_base = Decimal::ZERO;
    portfolio.futures_entry_price = Decimal::ZERO;
    portfolio.futures_margin_used_quote = Decimal::ZERO;
}

fn refresh_margin(portfolio: &mut Portfolio, leverage: Decimal) {
    if portfolio.futures_position_side == FuturesPositionSide::Flat {
        portfolio.futures_margin_used_quote = Decimal::ZERO;
    } else {
        portfolio.futures_margin_used_quote =
            (portfolio.futures_position_base * portfolio.futures_entry_price) / leverage;
    }
}

fn min_decimal(lhs: Decimal, rhs: Decimal) -> Decimal {
    if lhs <= rhs { lhs } else { rhs }
}

#[cfg(test)]
mod tests {
    use super::PaperFuturesExchange;
    use crate::decimal::Decimal;
    use crate::exchange::Exchange;
    use crate::orders::{OrderRequest, OrderStatus, Side};
    use crate::portfolio::{FuturesPositionSide, Portfolio};

    fn decimal(value: &str) -> Decimal {
        Decimal::from_decimal_str(value).expect("decimal should parse")
    }

    fn request(side: Side, quantity_base: &str, limit_price: &str) -> OrderRequest {
        OrderRequest {
            symbol: "BTC-USD".to_string(),
            side,
            quantity_base: decimal(quantity_base),
            limit_price: decimal(limit_price),
            client_order_id: Some("test-client-order".to_string()),
        }
    }

    fn exchange() -> PaperFuturesExchange {
        PaperFuturesExchange::new(
            Portfolio::paper_futures("BTC", "USD", decimal("10000")),
            decimal("2"),
        )
    }

    #[test]
    fn opens_long_position() {
        let mut exchange = exchange();
        let order = exchange
            .place_order(request(Side::Buy, "0.1", "10000"))
            .expect("long order should fill");

        assert_eq!(order.status, OrderStatus::Filled);
        assert_eq!(
            exchange.portfolio().futures_position_side,
            FuturesPositionSide::Long
        );
        assert_eq!(exchange.portfolio().futures_position_base, decimal("0.1"));
        assert_eq!(
            exchange.portfolio().futures_margin_used_quote,
            decimal("500")
        );
    }

    #[test]
    fn closes_short_with_profit() {
        let mut exchange = exchange();
        exchange
            .place_order(request(Side::Sell, "0.1", "10000"))
            .expect("short order should fill");
        exchange
            .place_order(request(Side::Buy, "0.1", "9000"))
            .expect("cover order should fill");

        assert_eq!(
            exchange.portfolio().futures_position_side,
            FuturesPositionSide::Flat
        );
        assert_eq!(exchange.portfolio().quote_balance, decimal("10100"));
        assert_eq!(
            exchange.portfolio().futures_realized_pnl_quote,
            decimal("100")
        );
    }

    #[test]
    fn flips_from_long_to_short() {
        let mut exchange = exchange();
        exchange
            .place_order(request(Side::Buy, "0.1", "10000"))
            .expect("long order should fill");
        exchange
            .place_order(request(Side::Sell, "0.15", "11000"))
            .expect("flip order should fill");

        assert_eq!(
            exchange.portfolio().futures_position_side,
            FuturesPositionSide::Short
        );
        assert_eq!(exchange.portfolio().futures_position_base, decimal("0.05"));
        assert_eq!(exchange.portfolio().quote_balance, decimal("10100"));
    }
}
