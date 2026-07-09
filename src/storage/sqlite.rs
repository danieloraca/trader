use crate::decimal::Decimal;
use crate::error::{BotError, Result};
use crate::market::MarketEvent;
use crate::orders::{Order, OrderRequest, Side};
use crate::portfolio::Portfolio;
use crate::storage::Store;
use rusqlite::{Connection, OptionalExtension, params};
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|error| {
                BotError::Storage(format!("failed to set sqlite busy timeout: {error}"))
            })?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|error| BotError::Storage(format!("failed to enable sqlite WAL: {error}")))?;

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
                    price_micro_units INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS orders (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    recorded_at_ms INTEGER NOT NULL,
                    bot_order_id INTEGER NOT NULL,
                    client_order_id TEXT NOT NULL,
                    exchange_order_id INTEGER,
                    symbol TEXT NOT NULL,
                    side TEXT NOT NULL,
                    quantity_base_micro_units INTEGER NOT NULL,
                    limit_price_micro_units INTEGER NOT NULL,
                    quote_value_micro_units INTEGER NOT NULL,
                    status TEXT NOT NULL,
                    status_reason TEXT
                );

                CREATE TABLE IF NOT EXISTS portfolio_state (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    updated_at_ms INTEGER NOT NULL,
                    base_currency TEXT NOT NULL,
                    quote_currency TEXT NOT NULL,
                    base_balance_micro_units INTEGER NOT NULL,
                    quote_balance_micro_units INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS replay_state (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    updated_at_ms INTEGER NOT NULL,
                    cursor INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS order_manager_state (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    updated_at_ms INTEGER NOT NULL,
                    next_order_id INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS heartbeat_state (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    updated_at_ms INTEGER NOT NULL,
                    run_id TEXT NOT NULL
                );
                ",
            )
            .map_err(|error| BotError::Storage(format!("failed to migrate sqlite: {error}")))?;

        self.ensure_orders_schema()?;
        self.ensure_market_events_schema()?;
        self.ensure_portfolio_schema()?;
        Ok(())
    }

    fn ensure_orders_schema(&self) -> Result<()> {
        if !self.column_exists("orders", "bot_order_id")? {
            self.connection
                .execute(
                    "ALTER TABLE orders ADD COLUMN bot_order_id INTEGER NOT NULL DEFAULT 0",
                    [],
                )
                .map_err(|error| {
                    BotError::Storage(format!("failed to add bot_order_id column: {error}"))
                })?;
        }

        if !self.column_exists("orders", "status_reason")? {
            self.connection
                .execute("ALTER TABLE orders ADD COLUMN status_reason TEXT", [])
                .map_err(|error| {
                    BotError::Storage(format!("failed to add status_reason column: {error}"))
                })?;
        }

        if !self.column_exists("orders", "client_order_id")? {
            self.connection
                .execute(
                    "ALTER TABLE orders ADD COLUMN client_order_id TEXT NOT NULL DEFAULT ''",
                    [],
                )
                .map_err(|error| {
                    BotError::Storage(format!("failed to add client_order_id column: {error}"))
                })?;
        }

        if self.column_not_null("orders", "exchange_order_id")? {
            self.rebuild_orders_with_nullable_exchange_order_id()?;
        }

        if self.column_exists("orders", "quantity_base")? {
            self.rebuild_orders_with_micro_units()?;
        }

        Ok(())
    }

    fn ensure_market_events_schema(&self) -> Result<()> {
        if self.column_exists("market_events", "price")? {
            self.rebuild_market_events_with_micro_units()?;
        }

        Ok(())
    }

    fn ensure_portfolio_schema(&self) -> Result<()> {
        if self.column_exists("portfolio_state", "base_balance")? {
            self.rebuild_portfolio_with_micro_units()?;
        }

        Ok(())
    }

    fn column_exists(&self, table: &str, column: &str) -> Result<bool> {
        let mut statement = self
            .connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|error| BotError::Storage(format!("failed to inspect schema: {error}")))?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|error| BotError::Storage(format!("failed to read schema: {error}")))?;

        for name in columns {
            if name.map_err(|error| BotError::Storage(format!("failed to read column: {error}")))?
                == column
            {
                return Ok(true);
            }
        }

        Ok(false)
    }

    fn column_not_null(&self, table: &str, column: &str) -> Result<bool> {
        let mut statement = self
            .connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|error| BotError::Storage(format!("failed to inspect schema: {error}")))?;
        let mut rows = statement
            .query([])
            .map_err(|error| BotError::Storage(format!("failed to read schema: {error}")))?;

        while let Some(row) = rows
            .next()
            .map_err(|error| BotError::Storage(format!("failed to read schema row: {error}")))?
        {
            let name: String = row.get(1).map_err(|error| {
                BotError::Storage(format!("failed to read column name: {error}"))
            })?;
            if name == column {
                let not_null: i64 = row.get(3).map_err(|error| {
                    BotError::Storage(format!("failed to read column nullability: {error}"))
                })?;
                return Ok(not_null != 0);
            }
        }

        Ok(false)
    }

    fn rebuild_orders_with_nullable_exchange_order_id(&self) -> Result<()> {
        self.connection
            .execute_batch(
                "
                CREATE TABLE orders_new (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    recorded_at_ms INTEGER NOT NULL,
                    bot_order_id INTEGER NOT NULL,
                    client_order_id TEXT NOT NULL,
                    exchange_order_id INTEGER,
                    symbol TEXT NOT NULL,
                    side TEXT NOT NULL,
                    quantity_base REAL NOT NULL,
                    limit_price REAL NOT NULL,
                    quote_value REAL NOT NULL,
                    status TEXT NOT NULL,
                    status_reason TEXT
                );

                INSERT INTO orders_new (
                    id,
                    recorded_at_ms,
                    bot_order_id,
                    client_order_id,
                    exchange_order_id,
                    symbol,
                    side,
                    quantity_base,
                    limit_price,
                    quote_value,
                    status,
                    status_reason
                )
                SELECT
                    id,
                    recorded_at_ms,
                    bot_order_id,
                    client_order_id,
                    NULLIF(exchange_order_id, 0),
                    symbol,
                    side,
                    quantity_base,
                    limit_price,
                    quote_value,
                    status,
                    status_reason
                FROM orders;

                DROP TABLE orders;
                ALTER TABLE orders_new RENAME TO orders;
                ",
            )
            .map_err(|error| {
                BotError::Storage(format!(
                    "failed to rebuild orders table for nullable exchange ids: {error}"
                ))
            })?;

        Ok(())
    }

    fn rebuild_orders_with_micro_units(&self) -> Result<()> {
        self.connection
            .execute_batch(
                "
                CREATE TABLE orders_new (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    recorded_at_ms INTEGER NOT NULL,
                    bot_order_id INTEGER NOT NULL,
                    client_order_id TEXT NOT NULL,
                    exchange_order_id INTEGER,
                    symbol TEXT NOT NULL,
                    side TEXT NOT NULL,
                    quantity_base_micro_units INTEGER NOT NULL,
                    limit_price_micro_units INTEGER NOT NULL,
                    quote_value_micro_units INTEGER NOT NULL,
                    status TEXT NOT NULL,
                    status_reason TEXT
                );

                INSERT INTO orders_new (
                    id,
                    recorded_at_ms,
                    bot_order_id,
                    client_order_id,
                    exchange_order_id,
                    symbol,
                    side,
                    quantity_base_micro_units,
                    limit_price_micro_units,
                    quote_value_micro_units,
                    status,
                    status_reason
                )
                SELECT
                    id,
                    recorded_at_ms,
                    bot_order_id,
                    client_order_id,
                    exchange_order_id,
                    symbol,
                    side,
                    CAST(ROUND(quantity_base * 1000000) AS INTEGER),
                    CAST(ROUND(limit_price * 1000000) AS INTEGER),
                    CAST(ROUND(quote_value * 1000000) AS INTEGER),
                    status,
                    status_reason
                FROM orders;

                DROP TABLE orders;
                ALTER TABLE orders_new RENAME TO orders;
                ",
            )
            .map_err(|error| {
                BotError::Storage(format!(
                    "failed to rebuild orders table for fixed-point values: {error}"
                ))
            })?;

        Ok(())
    }

    fn rebuild_market_events_with_micro_units(&self) -> Result<()> {
        self.connection
            .execute_batch(
                "
                CREATE TABLE market_events_new (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    recorded_at_ms INTEGER NOT NULL,
                    symbol TEXT NOT NULL,
                    price_micro_units INTEGER NOT NULL
                );

                INSERT INTO market_events_new (id, recorded_at_ms, symbol, price_micro_units)
                SELECT id, recorded_at_ms, symbol, CAST(ROUND(price * 1000000) AS INTEGER)
                FROM market_events;

                DROP TABLE market_events;
                ALTER TABLE market_events_new RENAME TO market_events;
                ",
            )
            .map_err(|error| {
                BotError::Storage(format!(
                    "failed to rebuild market events table for fixed-point values: {error}"
                ))
            })?;

        Ok(())
    }

    fn rebuild_portfolio_with_micro_units(&self) -> Result<()> {
        self.connection
            .execute_batch(
                "
                CREATE TABLE portfolio_state_new (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    updated_at_ms INTEGER NOT NULL,
                    base_currency TEXT NOT NULL,
                    quote_currency TEXT NOT NULL,
                    base_balance_micro_units INTEGER NOT NULL,
                    quote_balance_micro_units INTEGER NOT NULL
                );

                INSERT INTO portfolio_state_new (
                    id,
                    updated_at_ms,
                    base_currency,
                    quote_currency,
                    base_balance_micro_units,
                    quote_balance_micro_units
                )
                SELECT
                    id,
                    updated_at_ms,
                    base_currency,
                    quote_currency,
                    CAST(ROUND(base_balance * 1000000) AS INTEGER),
                    CAST(ROUND(quote_balance * 1000000) AS INTEGER)
                FROM portfolio_state;

                DROP TABLE portfolio_state;
                ALTER TABLE portfolio_state_new RENAME TO portfolio_state;
                ",
            )
            .map_err(|error| {
                BotError::Storage(format!(
                    "failed to rebuild portfolio table for fixed-point values: {error}"
                ))
            })?;

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

    #[cfg(test)]
    fn count_null_exchange_order_ids(&self) -> Result<i64> {
        self.connection
            .query_row(
                "SELECT COUNT(*) FROM orders WHERE exchange_order_id IS NULL",
                [],
                |row| row.get(0),
            )
            .map_err(|error| {
                BotError::Storage(format!("failed to count null exchange order ids: {error}"))
            })
    }

    #[cfg(test)]
    fn heartbeat_run_id(&self) -> Result<Option<String>> {
        self.connection
            .query_row(
                "SELECT run_id FROM heartbeat_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| BotError::Storage(format!("failed to load heartbeat: {error}")))
    }
}

impl Store for SqliteStore {
    fn load_portfolio(&self) -> Result<Option<Portfolio>> {
        self.connection
            .query_row(
                "
                SELECT base_currency, quote_currency, base_balance_micro_units, quote_balance_micro_units
                FROM portfolio_state
                WHERE id = 1
                ",
                [],
                |row| {
                    Ok(Portfolio {
                        base_currency: row.get(0)?,
                        quote_currency: row.get(1)?,
                        base_balance: Decimal::from_micro_units(row.get(2)?),
                        quote_balance: Decimal::from_micro_units(row.get(3)?),
                    })
                },
            )
            .optional()
            .map_err(|error| BotError::Storage(format!("failed to load portfolio: {error}")))
    }

    fn save_portfolio(&mut self, portfolio: &Portfolio) -> Result<()> {
        self.connection
            .execute(
                "
                INSERT INTO portfolio_state (
                    id,
                    updated_at_ms,
                    base_currency,
                    quote_currency,
                    base_balance_micro_units,
                    quote_balance_micro_units
                )
                VALUES (1, ?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(id) DO UPDATE SET
                    updated_at_ms = excluded.updated_at_ms,
                    base_currency = excluded.base_currency,
                    quote_currency = excluded.quote_currency,
                    base_balance_micro_units = excluded.base_balance_micro_units,
                    quote_balance_micro_units = excluded.quote_balance_micro_units
                ",
                params![
                    Self::now_ms()?,
                    portfolio.base_currency.as_str(),
                    portfolio.quote_currency.as_str(),
                    portfolio.base_balance.micro_units(),
                    portfolio.quote_balance.micro_units(),
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

    fn load_next_order_id(&self) -> Result<Option<u64>> {
        let explicit_next_order_id = self
            .connection
            .query_row(
                "SELECT next_order_id FROM order_manager_state WHERE id = 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| BotError::Storage(format!("failed to load next order id: {error}")))?;

        if let Some(value) = explicit_next_order_id {
            return u64::try_from(value).map(Some).map_err(|_| {
                BotError::Storage(format!("stored next order id is invalid: {value}"))
            });
        }

        let max_seen_id = self
            .connection
            .query_row("SELECT MAX(bot_order_id) FROM orders", [], |row| {
                row.get::<_, Option<i64>>(0)
            })
            .map_err(|error| {
                BotError::Storage(format!("failed to infer next order id: {error}"))
            })?;

        max_seen_id
            .map(|value| {
                let value = u64::try_from(value).map_err(|_| {
                    BotError::Storage(format!("stored bot order id is invalid: {value}"))
                })?;

                value
                    .checked_add(1)
                    .ok_or_else(|| BotError::Storage("next order id overflowed u64".to_string()))
            })
            .transpose()
    }

    fn save_next_order_id(&mut self, next_order_id: u64) -> Result<()> {
        let next_order_id = i64::try_from(next_order_id)
            .map_err(|_| BotError::Storage("next order id is too large to store".to_string()))?;

        self.connection
            .execute(
                "
                INSERT INTO order_manager_state (id, updated_at_ms, next_order_id)
                VALUES (1, ?1, ?2)
                ON CONFLICT(id) DO UPDATE SET
                    updated_at_ms = excluded.updated_at_ms,
                    next_order_id = excluded.next_order_id
                ",
                params![Self::now_ms()?, next_order_id],
            )
            .map_err(|error| BotError::Storage(format!("failed to save next order id: {error}")))?;

        Ok(())
    }

    fn save_heartbeat(&mut self, run_id: &str) -> Result<()> {
        self.connection
            .execute(
                "
                INSERT INTO heartbeat_state (id, updated_at_ms, run_id)
                VALUES (1, ?1, ?2)
                ON CONFLICT(id) DO UPDATE SET
                    updated_at_ms = excluded.updated_at_ms,
                    run_id = excluded.run_id
                ",
                params![Self::now_ms()?, run_id],
            )
            .map_err(|error| BotError::Storage(format!("failed to save heartbeat: {error}")))?;

        Ok(())
    }

    fn load_unresolved_submitted_orders(&self) -> Result<Vec<Order>> {
        let mut statement = self
            .connection
            .prepare(
                "
                SELECT
                    submitted.bot_order_id,
                    submitted.client_order_id,
                    submitted.symbol,
                    submitted.side,
                    submitted.quantity_base_micro_units,
                    submitted.limit_price_micro_units
                FROM orders submitted
                WHERE submitted.status = 'Submitted'
                  AND NOT EXISTS (
                      SELECT 1
                      FROM orders terminal
                      WHERE terminal.bot_order_id = submitted.bot_order_id
                        AND terminal.status IN ('Filled', 'Rejected', 'Cancelled')
                  )
                ORDER BY submitted.bot_order_id ASC
                ",
            )
            .map_err(|error| {
                BotError::Storage(format!("failed to prepare unresolved order query: {error}"))
            })?;

        let orders = statement
            .query_map([], |row| {
                let bot_order_id: i64 = row.get(0)?;
                let client_order_id: String = row.get(1)?;
                let symbol: String = row.get(2)?;
                let side: String = row.get(3)?;
                let quantity_base_micro_units: i64 = row.get(4)?;
                let limit_price_micro_units: i64 = row.get(5)?;

                Ok((
                    bot_order_id,
                    client_order_id,
                    symbol,
                    side,
                    quantity_base_micro_units,
                    limit_price_micro_units,
                ))
            })
            .map_err(|error| {
                BotError::Storage(format!("failed to query unresolved orders: {error}"))
            })?
            .map(|row| {
                let (
                    bot_order_id,
                    client_order_id,
                    symbol,
                    side,
                    quantity_base_micro_units,
                    limit_price_micro_units,
                ) = row.map_err(|error| {
                    BotError::Storage(format!("failed to read unresolved order row: {error}"))
                })?;

                let id = u64::try_from(bot_order_id).map_err(|_| {
                    BotError::Storage(format!("stored bot order id is invalid: {bot_order_id}"))
                })?;

                let side = parse_side(&side)?;
                Ok(Order::submitted(
                    id,
                    OrderRequest {
                        symbol,
                        side,
                        quantity_base: Decimal::from_micro_units(quantity_base_micro_units),
                        limit_price: Decimal::from_micro_units(limit_price_micro_units),
                        client_order_id: Some(client_order_id),
                    },
                ))
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(orders)
    }

    fn record_market_event(&mut self, event: &MarketEvent) -> Result<()> {
        self.connection
            .execute(
                "
                INSERT INTO market_events (recorded_at_ms, symbol, price_micro_units)
                VALUES (?1, ?2, ?3)
                ",
                params![Self::now_ms()?, event.symbol(), event.price().micro_units()],
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
                    bot_order_id,
                    client_order_id,
                    exchange_order_id,
                    symbol,
                    side,
                    quantity_base_micro_units,
                    limit_price_micro_units,
                    quote_value_micro_units,
                    status,
                    status_reason
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                ",
                params![
                    Self::now_ms()?,
                    order.id,
                    order.request.client_order_id.as_deref().unwrap_or(""),
                    optional_i64(order.exchange_order_id)?,
                    order.request.symbol.as_str(),
                    side_name(order.request.side),
                    order.request.quantity_base.micro_units(),
                    order.request.limit_price.micro_units(),
                    order.request.quote_value().micro_units(),
                    format!("{:?}", order.status),
                    order.status_reason.as_deref(),
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

fn parse_side(side: &str) -> Result<Side> {
    match side {
        "buy" => Ok(Side::Buy),
        "sell" => Ok(Side::Sell),
        _ => Err(BotError::Storage(format!(
            "stored order side is invalid: {side}"
        ))),
    }
}

fn optional_i64(value: Option<u64>) -> Result<Option<i64>> {
    value
        .map(|value| {
            i64::try_from(value).map_err(|_| {
                BotError::Storage(format!(
                    "value is too large to store as sqlite integer: {value}"
                ))
            })
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::SqliteStore;
    use crate::decimal::Decimal;
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

    fn decimal(value: f64) -> Decimal {
        Decimal::from_f64(value).expect("decimal should parse")
    }

    fn order(id: u64) -> Order {
        Order {
            id,
            exchange_order_id: Some(id),
            request: OrderRequest {
                symbol: "BTC-USD".to_string(),
                side: Side::Buy,
                quantity_base: decimal(0.01),
                limit_price: decimal(100.0),
                client_order_id: Some(format!("test-client-{id}")),
            },
            status: OrderStatus::Filled,
            status_reason: None,
        }
    }

    fn submitted_order(id: u64) -> Order {
        Order {
            id,
            exchange_order_id: None,
            request: OrderRequest {
                symbol: "BTC-USD".to_string(),
                side: Side::Buy,
                quantity_base: decimal(0.01),
                limit_price: decimal(100.0),
                client_order_id: Some(format!("test-client-{id}")),
            },
            status: OrderStatus::Submitted,
            status_reason: None,
        }
    }

    #[test]
    fn creates_database_and_records_events_and_orders() {
        let path = db_path("records");
        let mut store = SqliteStore::open(&path).expect("store should open");

        store
            .record_market_event(&MarketEvent::PriceTick(PriceTick::new(
                "BTC-USD",
                decimal(100.0),
            )))
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
    fn stores_missing_exchange_order_id_as_null() {
        let path = db_path("nullable-exchange-order-id");
        let mut store = SqliteStore::open(&path).expect("store should open");

        store
            .record_order(&submitted_order(1))
            .expect("order should record");

        assert_eq!(
            store
                .count_null_exchange_order_ids()
                .expect("count should work"),
            1
        );

        drop(store);
        fs::remove_file(path).expect("test database should be removed");
    }

    #[test]
    fn loads_unresolved_submitted_orders() {
        let path = db_path("unresolved-orders");
        let mut store = SqliteStore::open(&path).expect("store should open");

        store
            .record_order(&submitted_order(7))
            .expect("submitted order should record");

        let orders = store
            .load_unresolved_submitted_orders()
            .expect("unresolved orders should load");

        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].id, 7);
        assert_eq!(orders[0].status, OrderStatus::Submitted);
        assert_eq!(
            orders[0].request.client_order_id.as_deref(),
            Some("test-client-7")
        );

        drop(store);
        fs::remove_file(path).expect("test database should be removed");
    }

    #[test]
    fn ignores_submitted_orders_with_terminal_transition() {
        let path = db_path("resolved-orders");
        let mut store = SqliteStore::open(&path).expect("store should open");

        store
            .record_order(&submitted_order(7))
            .expect("submitted order should record");
        store
            .record_order(&order(7))
            .expect("terminal order should record");

        let orders = store
            .load_unresolved_submitted_orders()
            .expect("unresolved orders should load");

        assert!(orders.is_empty());

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
    fn saves_and_loads_portfolio_state() {
        let path = db_path("portfolio-state");
        let mut store = SqliteStore::open(&path).expect("store should open");
        let mut portfolio = Portfolio::new("BTC", "USD", decimal(9_998.47));
        portfolio.base_balance = decimal(0.015);

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
        assert_eq!(loaded.base_balance.to_string(), "0.015");
        assert_eq!(loaded.quote_balance.to_string(), "9998.47");

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

    #[test]
    fn saves_heartbeat_state() {
        let path = db_path("heartbeat");
        let mut store = SqliteStore::open(&path).expect("store should open");

        assert_eq!(
            store.heartbeat_run_id().expect("heartbeat should load"),
            None
        );

        store
            .save_heartbeat("run-test")
            .expect("heartbeat should save");
        store
            .save_heartbeat("run-test-2")
            .expect("heartbeat should update");

        assert_eq!(
            store.heartbeat_run_id().expect("heartbeat should load"),
            Some("run-test-2".to_string())
        );
        assert_eq!(
            store
                .count_rows("heartbeat_state")
                .expect("count should work"),
            1
        );

        drop(store);
        fs::remove_file(path).expect("test database should be removed");
    }

    #[test]
    fn saves_and_loads_next_order_id() {
        let path = db_path("next-order-id");
        let mut store = SqliteStore::open(&path).expect("store should open");

        assert_eq!(
            store
                .load_next_order_id()
                .expect("next order id load should work"),
            None
        );

        store
            .save_next_order_id(12)
            .expect("next order id should save");
        drop(store);

        let store = SqliteStore::open(&path).expect("store should reopen");

        assert_eq!(
            store
                .load_next_order_id()
                .expect("next order id load should work"),
            Some(12)
        );

        drop(store);
        fs::remove_file(path).expect("test database should be removed");
    }

    #[test]
    fn infers_next_order_id_from_order_history_when_state_is_missing() {
        let path = db_path("next-order-id-history");
        let mut store = SqliteStore::open(&path).expect("store should open");

        store.record_order(&order(7)).expect("order should record");
        drop(store);

        let store = SqliteStore::open(&path).expect("store should reopen");

        assert_eq!(
            store
                .load_next_order_id()
                .expect("next order id load should work"),
            Some(8)
        );

        drop(store);
        fs::remove_file(path).expect("test database should be removed");
    }
}
