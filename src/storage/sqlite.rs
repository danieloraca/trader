use crate::error::{BotError, Result};
use crate::market::MarketEvent;
use crate::orders::{Order, Side};
use crate::portfolio::Portfolio;
use crate::storage::Store;
use rusqlite::{Connection, OptionalExtension, params};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct SqliteStore {
    connection: Connection,
}

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();

        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|error| {
                BotError::Storage(format!(
                    "failed to create storage directory {}: {error}",
                    parent.to_string_lossy()
                ))
            })?;
        }

        let connection = Connection::open(path).map_err(|error| {
            BotError::Storage(format!(
                "failed to open sqlite database {}: {error}",
                path.to_string_lossy()
            ))
        })?;

        let store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        self.connection
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS market_events (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    recorded_at_ms INTEGER NOT NULL,
                    symbol TEXT NOT NULL,
                    price REAL NOT NULL
                );

                CREATE TABLE IF NOT EXISTS orders (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    recorded_at_ms INTEGER NOT NULL,
                    exchange_order_id INTEGER NOT NULL,
                    symbol TEXT NOT NULL,
                    side TEXT NOT NULL,
                    quantity_base REAL NOT NULL,
                    limit_price REAL NOT NULL,
                    quote_value REAL NOT NULL,
                    status TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS portfolio_state (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    updated_at_ms INTEGER NOT NULL,
                    base_currency TEXT NOT NULL,
                    quote_currency TEXT NOT NULL,
                    base_balance REAL NOT NULL,
                    quote_balance REAL NOT NULL
                );

                CREATE TABLE IF NOT EXISTS replay_state (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    updated_at_ms INTEGER NOT NULL,
                    cursor INTEGER NOT NULL
                );
                ",
            )
            .map_err(|error| BotError::Storage(format!("failed to migrate sqlite: {error}")))?;

        Ok(())
    }

    fn now_ms() -> Result<i64> {
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| {
                BotError::Storage(format!("system clock is before unix epoch: {error}"))
            })?;

        Ok(duration.as_millis() as i64)
    }

    #[cfg(test)]
    fn count_rows(&self, table: &str) -> Result<i64> {
        let sql = format!("SELECT COUNT(*) FROM {table}");
        self.connection
            .query_row(&sql, [], |row| row.get(0))
            .map_err(|error| BotError::Storage(format!("failed to count rows: {error}")))
    }
}

impl Store for SqliteStore {
    fn load_portfolio(&self) -> Result<Option<Portfolio>> {
        self.connection
            .query_row(
                "
                SELECT base_currency, quote_currency, base_balance, quote_balance
                FROM portfolio_state
                WHERE id = 1
                ",
                [],
                |row| {
                    Ok(Portfolio {
                        base_currency: row.get(0)?,
                        quote_currency: row.get(1)?,
                        base_balance: row.get(2)?,
                        quote_balance: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(|error| BotError::Storage(format!("failed to load portfolio: {error}")))
    }

    fn save_portfolio(&mut self, portfolio: &Portfolio) -> Result<()> {
        if !portfolio.base_balance.is_finite() || !portfolio.quote_balance.is_finite() {
            return Err(BotError::Storage(
                "cannot save portfolio with non-finite balances".to_string(),
            ));
        }

        self.connection
            .execute(
                "
                INSERT INTO portfolio_state (
                    id,
                    updated_at_ms,
                    base_currency,
                    quote_currency,
                    base_balance,
                    quote_balance
                )
                VALUES (1, ?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(id) DO UPDATE SET
                    updated_at_ms = excluded.updated_at_ms,
                    base_currency = excluded.base_currency,
                    quote_currency = excluded.quote_currency,
                    base_balance = excluded.base_balance,
                    quote_balance = excluded.quote_balance
                ",
                params![
                    Self::now_ms()?,
                    portfolio.base_currency.as_str(),
                    portfolio.quote_currency.as_str(),
                    portfolio.base_balance,
                    portfolio.quote_balance,
                ],
            )
            .map_err(|error| BotError::Storage(format!("failed to save portfolio: {error}")))?;

        Ok(())
    }

    fn load_replay_cursor(&self) -> Result<Option<usize>> {
        let cursor = self
            .connection
            .query_row("SELECT cursor FROM replay_state WHERE id = 1", [], |row| {
                row.get::<_, i64>(0)
            })
            .optional()
            .map_err(|error| BotError::Storage(format!("failed to load replay cursor: {error}")))?;

        cursor
            .map(|value| {
                usize::try_from(value).map_err(|_| {
                    BotError::Storage(format!("stored replay cursor is invalid: {value}"))
                })
            })
            .transpose()
    }

    fn save_replay_cursor(&mut self, cursor: usize) -> Result<()> {
        let cursor = i64::try_from(cursor)
            .map_err(|_| BotError::Storage("replay cursor is too large to store".to_string()))?;

        self.connection
            .execute(
                "
                INSERT INTO replay_state (id, updated_at_ms, cursor)
                VALUES (1, ?1, ?2)
                ON CONFLICT(id) DO UPDATE SET
                    updated_at_ms = excluded.updated_at_ms,
                    cursor = excluded.cursor
                ",
                params![Self::now_ms()?, cursor],
            )
            .map_err(|error| BotError::Storage(format!("failed to save replay cursor: {error}")))?;

        Ok(())
    }

    fn record_market_event(&mut self, event: &MarketEvent) -> Result<()> {
        if !event.price().is_finite() {
            return Err(BotError::Storage(
                "cannot record non-finite market price".to_string(),
            ));
        }

        self.connection
            .execute(
                "
                INSERT INTO market_events (recorded_at_ms, symbol, price)
                VALUES (?1, ?2, ?3)
                ",
                params![Self::now_ms()?, event.symbol(), event.price()],
            )
            .map_err(|error| {
                BotError::Storage(format!("failed to record market event: {error}"))
            })?;

        Ok(())
    }

    fn record_order(&mut self, order: &Order) -> Result<()> {
        self.connection
            .execute(
                "
                INSERT INTO orders (
                    recorded_at_ms,
                    exchange_order_id,
                    symbol,
                    side,
                    quantity_base,
                    limit_price,
                    quote_value,
                    status
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ",
                params![
                    Self::now_ms()?,
                    order.id,
                    order.request.symbol.as_str(),
                    side_name(order.request.side),
                    order.request.quantity_base,
                    order.request.limit_price,
                    order.request.quote_value(),
                    format!("{:?}", order.status),
                ],
            )
            .map_err(|error| BotError::Storage(format!("failed to record order: {error}")))?;

        Ok(())
    }
}

fn side_name(side: Side) -> &'static str {
    match side {
        Side::Buy => "buy",
        Side::Sell => "sell",
    }
}

#[cfg(test)]
mod tests {
    use super::SqliteStore;
    use crate::market::{MarketEvent, PriceTick};
    use crate::orders::{Order, OrderRequest, OrderStatus, Side};
    use crate::portfolio::Portfolio;
    use crate::storage::Store;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn db_path(name: &str) -> PathBuf {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_millis();
        std::env::temp_dir().join(format!("trader-{name}-{millis}.sqlite"))
    }

    fn order(id: u64) -> Order {
        Order {
            id,
            request: OrderRequest {
                symbol: "BTC-USD".to_string(),
                side: Side::Buy,
                quantity_base: 0.01,
                limit_price: 100.0,
            },
            status: OrderStatus::Filled,
        }
    }

    #[test]
    fn creates_database_and_records_events_and_orders() {
        let path = db_path("records");
        let mut store = SqliteStore::open(&path).expect("store should open");

        store
            .record_market_event(&MarketEvent::PriceTick(PriceTick::new("BTC-USD", 100.0)))
            .expect("market event should record");
        store.record_order(&order(1)).expect("order should record");

        assert_eq!(
            store
                .count_rows("market_events")
                .expect("count should work"),
            1
        );
        assert_eq!(store.count_rows("orders").expect("count should work"), 1);

        drop(store);
        fs::remove_file(path).expect("test database should be removed");
    }

    #[test]
    fn reopens_existing_database_without_losing_rows() {
        let path = db_path("reopen");
        let mut store = SqliteStore::open(&path).expect("store should open");

        store.record_order(&order(42)).expect("order should record");
        drop(store);

        let store = SqliteStore::open(&path).expect("store should reopen");

        assert_eq!(store.count_rows("orders").expect("count should work"), 1);

        drop(store);
        fs::remove_file(path).expect("test database should be removed");
    }

    #[test]
    fn rejects_non_finite_market_price() {
        let path = db_path("invalid-price");
        let mut store = SqliteStore::open(&path).expect("store should open");

        let error = store
            .record_market_event(&MarketEvent::PriceTick(PriceTick::new("BTC-USD", f64::NAN)))
            .expect_err("event should fail");

        assert!(error.to_string().contains("non-finite market price"));

        drop(store);
        fs::remove_file(path).expect("test database should be removed");
    }

    #[test]
    fn saves_and_loads_portfolio_state() {
        let path = db_path("portfolio-state");
        let mut store = SqliteStore::open(&path).expect("store should open");
        let mut portfolio = Portfolio::new("BTC", "USD", 9_998.47);
        portfolio.base_balance = 0.015;

        assert!(
            store
                .load_portfolio()
                .expect("portfolio load should work")
                .is_none()
        );

        store
            .save_portfolio(&portfolio)
            .expect("portfolio should save");
        drop(store);

        let store = SqliteStore::open(&path).expect("store should reopen");
        let loaded = store
            .load_portfolio()
            .expect("portfolio load should work")
            .expect("portfolio should exist");

        assert_eq!(loaded.base_currency, "BTC");
        assert_eq!(loaded.quote_currency, "USD");
        assert_eq!(loaded.base_balance, 0.015);
        assert_eq!(loaded.quote_balance, 9_998.47);

        drop(store);
        fs::remove_file(path).expect("test database should be removed");
    }

    #[test]
    fn saves_and_loads_replay_cursor() {
        let path = db_path("replay-cursor");
        let mut store = SqliteStore::open(&path).expect("store should open");

        assert_eq!(
            store.load_replay_cursor().expect("cursor load should work"),
            None
        );

        store.save_replay_cursor(5).expect("cursor should save");
        drop(store);

        let store = SqliteStore::open(&path).expect("store should reopen");

        assert_eq!(
            store.load_replay_cursor().expect("cursor load should work"),
            Some(5)
        );

        drop(store);
        fs::remove_file(path).expect("test database should be removed");
    }
}
