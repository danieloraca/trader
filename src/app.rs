use crate::config::Config;
use crate::error::Result;
use crate::exchange::{Exchange, PaperExchange};
use crate::market::{MarketEvent, PriceTick};
use crate::orders::OrderRequest;
use crate::portfolio::Portfolio;
use crate::risk::RiskManager;
use crate::storage::{InMemoryStore, Store};
use crate::strategy::{SimpleMomentumStrategy, Strategy};

pub struct App {
    config: Config,
    exchange: PaperExchange,
    risk: RiskManager,
    strategy: SimpleMomentumStrategy,
    store: InMemoryStore,
}

impl App {
    pub fn new(config: Config) -> Result<Self> {
        let portfolio = Portfolio::new(
            &config.bot.base_currency,
            &config.bot.quote_currency,
            config.bot.paper_starting_quote_balance,
        );

        Ok(Self {
            exchange: PaperExchange::new(portfolio),
            risk: RiskManager::new(config.risk.clone()),
            strategy: SimpleMomentumStrategy::new(),
            store: InMemoryStore::new(),
            config,
        })
    }

    pub fn run(&mut self) -> Result<()> {
        println!("starting trader for {}", self.config.bot.symbol);

        for price in [100.0, 101.0, 102.0, 101.5, 99.0] {
            let event = MarketEvent::PriceTick(PriceTick::new(&self.config.bot.symbol, price));
            self.store.record_market_event(&event)?;

            let signals = self.strategy.on_market_event(&event);
            for signal in signals {
                let portfolio = self.exchange.portfolio();
                let order_request: OrderRequest = self.risk.approve(&signal, portfolio)?;
                let order = self.exchange.place_order(order_request)?;
                self.store.record_order(&order)?;
                println!("placed paper order: {order}");
            }
        }

        println!("final paper portfolio: {}", self.exchange.portfolio());
        Ok(())
    }
}
