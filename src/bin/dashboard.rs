use rusqlite::{Connection, OptionalExtension};
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_ADDR: &str = "127.0.0.1:3040";
const DEFAULT_DB_PATH: &str = "/var/lib/trader/trader.sqlite";

#[derive(Debug)]
struct Dashboard {
    db_path: String,
}

#[derive(Debug, Default)]
struct Snapshot {
    market_event_count: i64,
    market_events_last_hour: i64,
    order_count: i64,
    db_size_bytes: u64,
    latest_market_event: Option<MarketEventRow>,
    heartbeat: Option<HeartbeatRow>,
    portfolio: Option<PortfolioRow>,
    recent_prices: Vec<MarketEventRow>,
    latest_orders: Vec<OrderRow>,
    strategy_research_run: Option<StrategyResearchRunRow>,
    strategy_research_results: Vec<StrategyResearchResultRow>,
}

#[derive(Debug)]
struct MarketEventRow {
    recorded_at_ms: i64,
    symbol: String,
    price_micro_units: i64,
}

#[derive(Debug)]
struct HeartbeatRow {
    updated_at_ms: i64,
    run_id: String,
}

#[derive(Debug)]
struct PortfolioRow {
    updated_at_ms: i64,
    base_currency: String,
    quote_currency: String,
    base_balance_micro_units: i64,
    quote_balance_micro_units: i64,
}

#[derive(Debug)]
struct OrderRow {
    recorded_at_ms: i64,
    bot_order_id: i64,
    client_order_id: String,
    exchange_order_id: Option<String>,
    symbol: String,
    side: String,
    quantity_base_micro_units: i64,
    limit_price_micro_units: i64,
    quote_value_micro_units: i64,
    status: String,
    status_reason: Option<String>,
}

#[derive(Debug)]
struct StrategyResearchRunRow {
    recorded_at_ms: i64,
    kind: String,
    symbol: String,
    runnable_count: i64,
    skipped_under_warmed_count: i64,
    train_split_bps: i64,
}

#[derive(Debug)]
struct StrategyResearchResultRow {
    rank: i64,
    interval_seconds: i64,
    candle_count: i64,
    train_candle_count: i64,
    test_candle_count: i64,
    fast_window: i64,
    slow_window: i64,
    quantity_base_micro_units: i64,
    train_pnl_micro_units: i64,
    train_return_pct: f64,
    train_buy_and_hold_delta_micro_units: i64,
    train_max_drawdown_pct: f64,
    train_filled_order_count: i64,
    train_rejected_order_count: i64,
    train_buy_count: i64,
    train_sell_count: i64,
    train_exposure_pct: f64,
    test_pnl_micro_units: i64,
    test_return_pct: f64,
    test_buy_and_hold_delta_micro_units: i64,
    test_max_drawdown_pct: f64,
    test_filled_order_count: i64,
    test_rejected_order_count: i64,
    test_buy_count: i64,
    test_sell_count: i64,
    test_exposure_pct: f64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = env::var("TRADER_DASHBOARD_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.to_string());
    let db_path = env::var("TRADER_DASHBOARD_DB").unwrap_or_else(|_| DEFAULT_DB_PATH.to_string());
    let dashboard = Dashboard { db_path };
    let listener = TcpListener::bind(&addr)?;

    eprintln!("dashboard listening on http://{addr}");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(error) = handle_stream(stream, &dashboard) {
                    eprintln!("dashboard request failed: {error}");
                }
            }
            Err(error) => eprintln!("dashboard connection failed: {error}"),
        }
    }

    Ok(())
}

fn handle_stream(mut stream: TcpStream, dashboard: &Dashboard) -> std::io::Result<()> {
    let mut buffer = [0_u8; 1024];
    let bytes_read = stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer[..bytes_read]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");

    let (status, content_type, body) = match path {
        "/" => match dashboard.render() {
            Ok(body) => ("200 OK", "text/html; charset=utf-8", body),
            Err(error) => (
                "500 Internal Server Error",
                "text/plain; charset=utf-8",
                format!("dashboard error: {error}\n"),
            ),
        },
        "/health" => ("200 OK", "text/plain; charset=utf-8", "ok\n".to_string()),
        _ => (
            "404 Not Found",
            "text/plain; charset=utf-8",
            "not found\n".to_string(),
        ),
    };

    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())
}

impl Dashboard {
    fn render(&self) -> rusqlite::Result<String> {
        let snapshot = self.snapshot()?;
        Ok(render_html(&self.db_path, &snapshot))
    }

    fn snapshot(&self) -> rusqlite::Result<Snapshot> {
        let connection = Connection::open_with_flags(
            &self.db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_URI
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;

        Ok(Snapshot {
            market_event_count: count_rows(&connection, "market_events")?,
            market_events_last_hour: recent_market_event_count(&connection, now_ms()?)?,
            order_count: count_rows(&connection, "orders")?,
            db_size_bytes: sqlite_file_size(&self.db_path),
            latest_market_event: latest_market_event(&connection)?,
            heartbeat: heartbeat(&connection)?,
            portfolio: portfolio(&connection)?,
            recent_prices: recent_prices(&connection)?,
            latest_orders: latest_orders(&connection)?,
            strategy_research_run: latest_strategy_research_run(&connection)?,
            strategy_research_results: latest_strategy_research_results(&connection)?,
        })
    }
}

fn count_rows(connection: &Connection, table: &str) -> rusqlite::Result<i64> {
    connection.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
}

fn latest_market_event(connection: &Connection) -> rusqlite::Result<Option<MarketEventRow>> {
    connection
        .query_row(
            "
            SELECT recorded_at_ms, symbol, price_micro_units
            FROM market_events
            ORDER BY id DESC
            LIMIT 1
            ",
            [],
            |row| {
                Ok(MarketEventRow {
                    recorded_at_ms: row.get(0)?,
                    symbol: row.get(1)?,
                    price_micro_units: row.get(2)?,
                })
            },
        )
        .optional()
}

fn recent_market_event_count(connection: &Connection, now_ms: i64) -> rusqlite::Result<i64> {
    connection.query_row(
        "SELECT COUNT(*) FROM market_events WHERE recorded_at_ms >= ?1",
        [now_ms - 3_600_000],
        |row| row.get(0),
    )
}

fn recent_prices(connection: &Connection) -> rusqlite::Result<Vec<MarketEventRow>> {
    let mut statement = connection.prepare(
        "
        SELECT recorded_at_ms, symbol, price_micro_units
        FROM market_events
        ORDER BY id DESC
        LIMIT 100
        ",
    )?;
    let mut rows = statement
        .query_map([], |row| {
            Ok(MarketEventRow {
                recorded_at_ms: row.get(0)?,
                symbol: row.get(1)?,
                price_micro_units: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.reverse();
    Ok(rows)
}

fn heartbeat(connection: &Connection) -> rusqlite::Result<Option<HeartbeatRow>> {
    connection
        .query_row(
            "SELECT updated_at_ms, run_id FROM heartbeat_state WHERE id = 1",
            [],
            |row| {
                Ok(HeartbeatRow {
                    updated_at_ms: row.get(0)?,
                    run_id: row.get(1)?,
                })
            },
        )
        .optional()
}

fn portfolio(connection: &Connection) -> rusqlite::Result<Option<PortfolioRow>> {
    connection
        .query_row(
            "
            SELECT
                updated_at_ms,
                base_currency,
                quote_currency,
                base_balance_micro_units,
                quote_balance_micro_units
            FROM portfolio_state
            WHERE id = 1
            ",
            [],
            |row| {
                Ok(PortfolioRow {
                    updated_at_ms: row.get(0)?,
                    base_currency: row.get(1)?,
                    quote_currency: row.get(2)?,
                    base_balance_micro_units: row.get(3)?,
                    quote_balance_micro_units: row.get(4)?,
                })
            },
        )
        .optional()
}

fn latest_orders(connection: &Connection) -> rusqlite::Result<Vec<OrderRow>> {
    let mut statement = connection.prepare(
        "
        SELECT
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
        FROM orders
        ORDER BY id DESC
        LIMIT 20
        ",
    )?;

    statement
        .query_map([], |row| {
            Ok(OrderRow {
                recorded_at_ms: row.get(0)?,
                bot_order_id: row.get(1)?,
                client_order_id: row.get(2)?,
                exchange_order_id: row.get(3)?,
                symbol: row.get(4)?,
                side: row.get(5)?,
                quantity_base_micro_units: row.get(6)?,
                limit_price_micro_units: row.get(7)?,
                quote_value_micro_units: row.get(8)?,
                status: row.get(9)?,
                status_reason: row.get(10)?,
            })
        })?
        .collect()
}

fn latest_strategy_research_run(
    connection: &Connection,
) -> rusqlite::Result<Option<StrategyResearchRunRow>> {
    if !table_exists(connection, "strategy_research_runs")? {
        return Ok(None);
    }

    let split_projection =
        if column_exists(connection, "strategy_research_runs", "train_split_bps")? {
            "train_split_bps"
        } else {
            "7000"
        };

    connection
        .query_row(
            &format!(
                "
            SELECT
                recorded_at_ms,
                kind,
                symbol,
                runnable_count,
                skipped_under_warmed_count,
                {split_projection}
            FROM strategy_research_runs
            ORDER BY id DESC
            LIMIT 1
            "
            ),
            [],
            |row| {
                Ok(StrategyResearchRunRow {
                    recorded_at_ms: row.get(0)?,
                    kind: row.get(1)?,
                    symbol: row.get(2)?,
                    runnable_count: row.get(3)?,
                    skipped_under_warmed_count: row.get(4)?,
                    train_split_bps: row.get(5)?,
                })
            },
        )
        .optional()
}

fn latest_strategy_research_results(
    connection: &Connection,
) -> rusqlite::Result<Vec<StrategyResearchResultRow>> {
    if !table_exists(connection, "strategy_research_runs")?
        || !table_exists(connection, "strategy_research_results")?
        || !column_exists(
            connection,
            "strategy_research_results",
            "test_pnl_micro_units",
        )?
    {
        return Ok(Vec::new());
    }

    let Some(run_id) = connection
        .query_row(
            "SELECT id FROM strategy_research_runs ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
    else {
        return Ok(Vec::new());
    };

    let mut statement = connection.prepare(
        "
        SELECT
            rank,
            interval_seconds,
            candle_count,
            train_candle_count,
            test_candle_count,
            fast_window,
            slow_window,
            quantity_base_micro_units,
            train_pnl_micro_units,
            train_return_pct,
            train_buy_and_hold_delta_micro_units,
            train_max_drawdown_pct,
            train_filled_order_count,
            train_rejected_order_count,
            train_buy_count,
            train_sell_count,
            train_exposure_pct,
            test_pnl_micro_units,
            test_return_pct,
            test_buy_and_hold_delta_micro_units,
            test_max_drawdown_pct,
            test_filled_order_count,
            test_rejected_order_count,
            test_buy_count,
            test_sell_count,
            test_exposure_pct
        FROM strategy_research_results
        WHERE run_id = ?1
        ORDER BY rank ASC
        LIMIT 10
        ",
    )?;

    statement
        .query_map([run_id], |row| {
            Ok(StrategyResearchResultRow {
                rank: row.get(0)?,
                interval_seconds: row.get(1)?,
                candle_count: row.get(2)?,
                train_candle_count: row.get(3)?,
                test_candle_count: row.get(4)?,
                fast_window: row.get(5)?,
                slow_window: row.get(6)?,
                quantity_base_micro_units: row.get(7)?,
                train_pnl_micro_units: row.get(8)?,
                train_return_pct: row.get(9)?,
                train_buy_and_hold_delta_micro_units: row.get(10)?,
                train_max_drawdown_pct: row.get(11)?,
                train_filled_order_count: row.get(12)?,
                train_rejected_order_count: row.get(13)?,
                train_buy_count: row.get(14)?,
                train_sell_count: row.get(15)?,
                train_exposure_pct: row.get(16)?,
                test_pnl_micro_units: row.get(17)?,
                test_return_pct: row.get(18)?,
                test_buy_and_hold_delta_micro_units: row.get(19)?,
                test_max_drawdown_pct: row.get(20)?,
                test_filled_order_count: row.get(21)?,
                test_rejected_order_count: row.get(22)?,
                test_buy_count: row.get(23)?,
                test_sell_count: row.get(24)?,
                test_exposure_pct: row.get(25)?,
            })
        })?
        .collect()
}

fn table_exists(connection: &Connection, table: &str) -> rusqlite::Result<bool> {
    connection.query_row(
        "
        SELECT EXISTS (
            SELECT 1
            FROM sqlite_master
            WHERE type = 'table' AND name = ?1
        )
        ",
        [table],
        |row| row.get(0),
    )
}

fn column_exists(connection: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = statement.query([])?;

    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }

    Ok(false)
}

fn render_html(db_path: &str, snapshot: &Snapshot) -> String {
    let mut html = String::new();
    let _ = write!(
        html,
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="refresh" content="10">
<title>Trader Dashboard</title>
<style>
:root {{ color-scheme: light dark; font-family: system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
body {{ margin: 0; background: #101214; color: #e8eaed; }}
main {{ max-width: 1180px; margin: 0 auto; padding: 24px; }}
h1 {{ margin: 0 0 4px; font-size: 28px; }}
h2 {{ margin: 28px 0 12px; font-size: 18px; }}
.muted {{ color: #9aa0a6; font-size: 13px; }}
.grid {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr)); gap: 12px; margin-top: 20px; }}
.tile {{ border: 1px solid #30363d; border-radius: 8px; padding: 14px; background: #161b22; }}
.tile.ok {{ border-color: #2f8f46; }}
.tile.warn {{ border-color: #b88722; }}
.tile.bad {{ border-color: #b94b4b; }}
.label {{ color: #9aa0a6; font-size: 12px; text-transform: uppercase; letter-spacing: .04em; }}
.value {{ margin-top: 6px; font-size: 22px; font-weight: 650; overflow-wrap: anywhere; }}
.subgrid {{ display: grid; grid-template-columns: 1fr 1fr; gap: 12px; margin-top: 12px; }}
.spark {{ width: 100%; height: 150px; display: block; background: #161b22; border: 1px solid #30363d; border-radius: 8px; }}
table {{ width: 100%; border-collapse: collapse; background: #161b22; border: 1px solid #30363d; border-radius: 8px; overflow: hidden; }}
th, td {{ border-bottom: 1px solid #30363d; padding: 10px; text-align: left; font-size: 14px; vertical-align: top; }}
th {{ color: #9aa0a6; font-size: 12px; text-transform: uppercase; letter-spacing: .04em; }}
tr:last-child td {{ border-bottom: 0; }}
.status {{ display: inline-block; padding: 3px 7px; border-radius: 999px; background: #243b2a; color: #9ee493; font-size: 12px; }}
@media (max-width: 720px) {{ main {{ padding: 16px; }} table {{ display: block; overflow-x: auto; }} .subgrid {{ grid-template-columns: 1fr; }} }}
</style>
<script>
function formatTimes() {{
  document.querySelectorAll("[data-ms]").forEach((node) => {{
    const ms = Number(node.dataset.ms);
    if (Number.isFinite(ms) && ms > 0) {{
      node.textContent = new Date(ms).toLocaleString();
    }}
  }});
}}
window.addEventListener("DOMContentLoaded", formatTimes);
</script>
</head>
<body>
<main>
<h1>Trader Dashboard</h1>
<div class="muted">Read-only SQLite view. Auto-refreshes every 10 seconds. DB: {}</div>
"#,
        escape_html(db_path)
    );

    render_summary(&mut html, snapshot);
    render_price_chart(&mut html, &snapshot.recent_prices);
    render_strategy_research(
        &mut html,
        snapshot.strategy_research_run.as_ref(),
        &snapshot.strategy_research_results,
    );
    render_orders(&mut html, &snapshot.latest_orders);

    html.push_str("</main></body></html>");
    html
}

fn render_summary(html: &mut String, snapshot: &Snapshot) {
    let latest_price_micro_units = snapshot
        .latest_market_event
        .as_ref()
        .map(|event| event.price_micro_units);
    let latest_price = snapshot
        .latest_market_event
        .as_ref()
        .map(|event| {
            format!(
                "{} {}",
                event.symbol,
                format_micro_units(event.price_micro_units)
            )
        })
        .unwrap_or_else(|| "n/a".to_string());
    let latest_price_time_ms = snapshot
        .latest_market_event
        .as_ref()
        .map(|event| event.recorded_at_ms);
    let heartbeat_time_ms = snapshot
        .heartbeat
        .as_ref()
        .map(|heartbeat| heartbeat.updated_at_ms);
    let heartbeat_age_seconds = snapshot
        .heartbeat
        .as_ref()
        .map(|heartbeat| age_seconds(heartbeat.updated_at_ms));
    let heartbeat_age = heartbeat_age_seconds
        .map(|age| format!("{age}s ago"))
        .unwrap_or_else(|| "n/a".to_string());
    let heartbeat_class = match heartbeat_age_seconds {
        Some(age) if age <= 20 => "ok",
        Some(age) if age <= 60 => "warn",
        Some(_) => "bad",
        None => "bad",
    };
    let run_id = snapshot
        .heartbeat
        .as_ref()
        .map(|heartbeat| heartbeat.run_id.as_str())
        .unwrap_or("n/a");
    let run_uptime = snapshot
        .heartbeat
        .as_ref()
        .and_then(|heartbeat| run_start_ms(&heartbeat.run_id))
        .map(|started_at_ms| format_duration(age_seconds(started_at_ms)))
        .unwrap_or_else(|| "n/a".to_string());
    let portfolio = snapshot
        .portfolio
        .as_ref()
        .map(|portfolio| {
            format!(
                "{} {}, {} {}",
                portfolio.base_currency,
                format_micro_units(portfolio.base_balance_micro_units),
                portfolio.quote_currency,
                format_micro_units(portfolio.quote_balance_micro_units)
            )
        })
        .unwrap_or_else(|| "n/a".to_string());
    let portfolio_time_ms = snapshot
        .portfolio
        .as_ref()
        .map(|portfolio| portfolio.updated_at_ms);
    let marked_value = snapshot
        .portfolio
        .as_ref()
        .zip(latest_price_micro_units)
        .map(|(portfolio, price)| {
            format_micro_units(
                portfolio.quote_balance_micro_units
                    + scaled_product(portfolio.base_balance_micro_units, price),
            )
        })
        .unwrap_or_else(|| "n/a".to_string());

    let _ = write!(
        html,
        r#"<section class="grid">
<div class="tile"><div class="label">Market Events</div><div class="value">{}</div></div>
<div class="tile"><div class="label">Events Last Hour</div><div class="value">{}</div></div>
<div class="tile"><div class="label">Orders</div><div class="value">{}</div></div>
<div class="tile"><div class="label">DB Size</div><div class="value">{}</div></div>
<div class="tile"><div class="label">Latest Price</div><div class="value">{}</div><div class="muted" data-ms="{}">{}</div></div>
<div class="tile {}"><div class="label">Heartbeat</div><div class="value">{}</div><div class="muted" data-ms="{}">{}</div></div>
<div class="tile"><div class="label">Run Uptime</div><div class="value">{}</div><div class="muted">{}</div></div>
<div class="tile"><div class="label">Portfolio</div><div class="value">{}</div><div class="muted" data-ms="{}">{}</div></div>
<div class="tile"><div class="label">Marked Value</div><div class="value">{}</div></div>
</section>"#,
        snapshot.market_event_count,
        snapshot.market_events_last_hour,
        snapshot.order_count,
        escape_html(&format_bytes(snapshot.db_size_bytes)),
        escape_html(&latest_price),
        latest_price_time_ms.unwrap_or_default(),
        escape_html(&time_fallback(latest_price_time_ms)),
        heartbeat_class,
        escape_html(&heartbeat_age),
        heartbeat_time_ms.unwrap_or_default(),
        escape_html(&time_fallback(heartbeat_time_ms)),
        escape_html(&run_uptime),
        escape_html(run_id),
        escape_html(&portfolio),
        portfolio_time_ms.unwrap_or_default(),
        escape_html(&time_fallback(portfolio_time_ms)),
        escape_html(&marked_value),
    );
}

fn render_price_chart(html: &mut String, prices: &[MarketEventRow]) {
    html.push_str(r#"<h2>Recent Price</h2>"#);

    if prices.len() < 2 {
        html.push_str(r#"<div class="tile muted">Not enough price history yet.</div>"#);
        return;
    }

    let min_price = prices
        .iter()
        .map(|price| price.price_micro_units)
        .min()
        .unwrap_or(0);
    let max_price = prices
        .iter()
        .map(|price| price.price_micro_units)
        .max()
        .unwrap_or(0);
    let range = (max_price - min_price).max(1);
    let last_price = prices
        .last()
        .map(|price| format_micro_units(price.price_micro_units))
        .unwrap_or_else(|| "n/a".to_string());

    let mut points = String::new();
    for (index, price) in prices.iter().enumerate() {
        let x = if prices.len() == 1 {
            0.0
        } else {
            index as f64 / (prices.len() - 1) as f64 * 100.0
        };
        let normalized = (price.price_micro_units - min_price) as f64 / range as f64;
        let y = 90.0 - (normalized * 80.0);
        let _ = write!(points, "{x:.2},{y:.2} ");
    }

    let _ = write!(
        html,
        r##"<svg class="spark" viewBox="0 0 100 100" preserveAspectRatio="none" role="img" aria-label="Recent price sparkline">
<polyline points="{}" fill="none" stroke="#58a6ff" stroke-width="2" vector-effect="non-scaling-stroke"></polyline>
</svg>
<div class="subgrid">
<div class="tile"><div class="label">Chart Samples</div><div class="value">{}</div></div>
<div class="tile"><div class="label">Last Chart Price</div><div class="value">{}</div></div>
<div class="tile"><div class="label">Chart Low</div><div class="value">{}</div></div>
<div class="tile"><div class="label">Chart High</div><div class="value">{}</div></div>
</div>"##,
        escape_html(points.trim()),
        prices.len(),
        escape_html(&last_price),
        escape_html(&format_micro_units(min_price)),
        escape_html(&format_micro_units(max_price))
    );
}

fn render_orders(html: &mut String, orders: &[OrderRow]) {
    html.push_str(
        r#"<h2>Latest Order Events</h2>
<table>
<thead>
<tr>
<th>Time</th>
<th>Bot ID</th>
<th>Status</th>
<th>Side</th>
<th>Qty</th>
<th>Limit</th>
<th>Quote</th>
<th>Client ID</th>
<th>Exchange ID</th>
<th>Reason</th>
</tr>
</thead>
<tbody>"#,
    );

    if orders.is_empty() {
        html.push_str(r#"<tr><td colspan="10" class="muted">No order events yet.</td></tr>"#);
    } else {
        for order in orders {
            let _ = write!(
                html,
                r#"<tr>
<td>{}</td>
<td>{}</td>
<td><span class="status">{}</span></td>
<td>{} {}</td>
<td>{}</td>
<td>{}</td>
<td>{}</td>
<td>{}</td>
<td>{}</td>
<td>{}</td>
</tr>"#,
                format!(
                    r#"<span data-ms="{}">{}</span>"#,
                    order.recorded_at_ms,
                    escape_html(&time_fallback(Some(order.recorded_at_ms)))
                ),
                order.bot_order_id,
                escape_html(&order.status),
                escape_html(&order.symbol),
                escape_html(&order.side),
                escape_html(&format_micro_units(order.quantity_base_micro_units)),
                escape_html(&format_micro_units(order.limit_price_micro_units)),
                escape_html(&format_micro_units(order.quote_value_micro_units)),
                escape_html(&order.client_order_id),
                escape_html(order.exchange_order_id.as_deref().unwrap_or("")),
                escape_html(order.status_reason.as_deref().unwrap_or(""))
            );
        }
    }

    html.push_str("</tbody></table>");
}

fn render_strategy_research(
    html: &mut String,
    run: Option<&StrategyResearchRunRow>,
    results: &[StrategyResearchResultRow],
) {
    html.push_str(r#"<h2>Strategy Research</h2>"#);

    let Some(run) = run else {
        html.push_str(
            r#"<div class="tile muted">No saved sweep yet. Run <code>target/release/trader --config config/pi-paper-live.toml --sweep-candles-sqlite /var/lib/trader/trader.sqlite</code>.</div>"#,
        );
        return;
    };

    let _ = write!(
        html,
        r#"<section class="grid">
<div class="tile"><div class="label">Latest Sweep</div><div class="value" data-ms="{}">{}</div><div class="muted">{} {}</div></div>
<div class="tile"><div class="label">Runnable</div><div class="value">{}</div></div>
<div class="tile"><div class="label">Skipped Warmup</div><div class="value">{}</div></div>
<div class="tile"><div class="label">Train/Test</div><div class="value">{}% / {}%</div></div>
</section>"#,
        run.recorded_at_ms,
        escape_html(&time_fallback(Some(run.recorded_at_ms))),
        escape_html(&run.symbol),
        escape_html(&run.kind),
        run.runnable_count,
        run.skipped_under_warmed_count,
        run.train_split_bps / 100,
        100 - (run.train_split_bps / 100),
    );

    html.push_str(
        r#"<table>
<thead>
<tr>
<th>Rank</th>
<th>Interval</th>
<th>Candles</th>
<th>MA</th>
<th>Qty</th>
<th>Train P/L</th>
<th>Test P/L</th>
<th>Train Ret</th>
<th>Test Ret</th>
<th>Train Vs Hold</th>
<th>Test Vs Hold</th>
<th>Train DD</th>
<th>Test DD</th>
<th>Train Fills</th>
<th>Test Fills</th>
<th>Exposure</th>
</tr>
</thead>
<tbody>"#,
    );

    if results.is_empty() {
        html.push_str(
            r#"<tr><td colspan="15" class="muted">No runnable train/test sweep rows yet.</td></tr>"#,
        );
    } else {
        for result in results {
            let _ = write!(
                html,
                r#"<tr>
<td>{}</td>
<td>{}s</td>
<td>{} <span class="muted">{} / {}</span></td>
<td>{}/{}</td>
<td>{}</td>
<td>{}</td>
<td>{}</td>
<td>{:.2}%</td>
<td>{:.2}%</td>
<td>{}</td>
<td>{}</td>
<td>{:.2}%</td>
<td>{:.2}%</td>
<td>{} <span class="muted">rej {}</span> <span class="muted">{} / {}</span></td>
<td>{} <span class="muted">rej {}</span> <span class="muted">{} / {}</span></td>
<td>{:.2}% / {:.2}%</td>
</tr>"#,
                result.rank,
                result.interval_seconds,
                result.candle_count,
                result.train_candle_count,
                result.test_candle_count,
                result.fast_window,
                result.slow_window,
                escape_html(&format_micro_units(result.quantity_base_micro_units)),
                escape_html(&format_micro_units(result.train_pnl_micro_units)),
                escape_html(&format_micro_units(result.test_pnl_micro_units)),
                result.train_return_pct,
                result.test_return_pct,
                escape_html(&format_micro_units(
                    result.train_buy_and_hold_delta_micro_units
                )),
                escape_html(&format_micro_units(
                    result.test_buy_and_hold_delta_micro_units
                )),
                result.train_max_drawdown_pct,
                result.test_max_drawdown_pct,
                result.train_filled_order_count,
                result.train_rejected_order_count,
                result.train_buy_count,
                result.train_sell_count,
                result.test_filled_order_count,
                result.test_rejected_order_count,
                result.test_buy_count,
                result.test_sell_count,
                result.train_exposure_pct,
                result.test_exposure_pct,
            );
        }
    }

    html.push_str("</tbody></table>");
}

fn format_micro_units(value: i64) -> String {
    let sign = if value < 0 { "-" } else { "" };
    let absolute = value.abs();
    let whole = absolute / 1_000_000;
    let fractional = absolute % 1_000_000;

    if fractional == 0 {
        format!("{sign}{whole}")
    } else {
        let mut fractional = format!("{fractional:06}");
        while fractional.ends_with('0') {
            fractional.pop();
        }
        format!("{sign}{whole}.{fractional}")
    }
}

fn time_fallback(ms: Option<i64>) -> String {
    ms.map(|value| format!("{}s", value / 1000))
        .unwrap_or_else(|| "n/a".to_string())
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = UNITS[0];

    for next_unit in UNITS.iter().skip(1) {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = next_unit;
    }

    if unit == "B" {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {unit}")
    }
}

fn format_duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;

    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

fn run_start_ms(run_id: &str) -> Option<i64> {
    run_id.strip_prefix("run-")?.split('-').next()?.parse().ok()
}

fn scaled_product(lhs_micro_units: i64, rhs_micro_units: i64) -> i64 {
    ((lhs_micro_units as i128 * rhs_micro_units as i128) / 1_000_000_i128) as i64
}

fn sqlite_file_size(db_path: &str) -> u64 {
    [
        db_path.to_string(),
        format!("{db_path}-wal"),
        format!("{db_path}-shm"),
    ]
    .into_iter()
    .filter_map(|path| fs::metadata(path).ok())
    .map(|metadata| metadata.len())
    .sum()
}

fn now_ms() -> rusqlite::Result<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
}

fn age_seconds(ms: i64) -> i64 {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(ms);
    ((now_ms - ms) / 1000).max(0)
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::{format_micro_units, render_html};

    #[test]
    fn formats_micro_units_like_decimal_values() {
        assert_eq!(format_micro_units(1_000_000), "1");
        assert_eq!(format_micro_units(1_230_000), "1.23");
        assert_eq!(format_micro_units(-5_000), "-0.005");
    }

    #[test]
    fn renders_empty_snapshot() {
        let html = render_html("/tmp/trader.sqlite", &Default::default());

        assert!(html.contains("Trader Dashboard"));
        assert!(html.contains("No order events yet."));
    }
}
