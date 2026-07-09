use crate::config::Config;
use crate::error::Result;
use crate::exchange::{Exchange, PaperExchange};
use crate::market::{MarketDataSource, ReplayMarketDataSource};
use crate::orders::OrderRequest;
use crate::portfolio::Portfolio;
use crate::risk::RiskManager;
use crate::storage::{SqliteStore, Store};
use crate::strategy::{SimpleMomentumStrategy, Strategy};

pub struct App {
    config: Config,
    exchange: PaperExchange,
    market_data: ReplayMarketDataSource,
    risk: RiskManager,
    strategy: SimpleMomentumStrategy,
    store: SqliteStore,
}

impl App {
    pub fn new(config: Config) -> Result<Self> {
        let store = SqliteStore::open(&config.storage.sqlite_path)?;
        let portfolio = store.load_portfolio()?.unwrap_or_else(|| {
            Portfolio::new(
                &config.bot.base_currency,
                &config.bot.quote_currency,
                config.bot.paper_starting_quote_balance,
            )
        });
        let replay_cursor = store.load_replay_cursor()?.unwrap_or(0);
        let market_data = ReplayMarketDataSource::from_prices_at_cursor(
            &config.bot.symbol,
            config.market_data.replay_prices.clone(),
            replay_cursor,
        );

        Ok(Self {
            exchange: PaperExchange::new(portfolio),
            market_data,
            risk: RiskManager::new(config.risk.clone()),
            strategy: SimpleMomentumStrategy::new(),
            store,
            config,
        })
    }

    pub fn run(&mut self) -> Result<()> {
        println!("starting trader for {}", self.config.bot.symbol);

        while let Some(event) = self.market_data.next_event()? {
            self.store.record_market_event(&event)?;

            let signals = self.strategy.on_market_event(&event);
            for signal in signals {
                let portfolio = self.exchange.portfolio();
                let order_request: OrderRequest = self.risk.approve(&signal, portfolio)?;
                let order = self.exchange.place_order(order_request)?;
                self.store.record_order(&order)?;
                println!("placed paper order: {order}");
            }

            self.store.save_portfolio(self.exchange.portfolio())?;
            self.store.save_replay_cursor(self.market_data.cursor())?;
        }

        println!("final paper portfolio: {}", self.exchange.portfolio());
        Ok(())
    }
}
