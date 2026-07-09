use crate::market::MarketEvent;
use crate::orders::Side;
use crate::strategy::{Signal, Strategy};

pub struct SimpleMomentumStrategy {
    last_price: Option<f64>,
}

impl SimpleMomentumStrategy {
    pub fn new() -> Self {
        Self { last_price: None }
    }
}

impl Strategy for SimpleMomentumStrategy {
    fn on_market_event(&mut self, event: &MarketEvent) -> Vec<Signal> {
        let previous_price = self.last_price.replace(event.price());

        let Some(previous_price) = previous_price else {
            return Vec::new();
        };

        let change = (event.price() - previous_price) / previous_price;

        if change > 0.005 {
            vec![Signal {
                symbol: event.symbol().to_string(),
                side: Side::Buy,
                quantity_base: 0.01,
                price: event.price(),
                reason: format!("price rose {:.2}%", change * 100.0),
            }]
        } else if change < -0.01 {
            vec![Signal {
                symbol: event.symbol().to_string(),
                side: Side::Sell,
                quantity_base: 0.005,
                price: event.price(),
                reason: format!("price fell {:.2}%", change * 100.0),
            }]
        } else {
            Vec::new()
        }
    }
}
