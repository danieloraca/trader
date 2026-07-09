use rusqlite::{Connection, OptionalExtension};
use std::env;
use std::fmt::Write as _;
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
    order_count: i64,
    latest_market_event: Option<MarketEventRow>,
    heartbeat: Option<HeartbeatRow>,
    portfolio: Option<PortfolioRow>,
    latest_orders: Vec<OrderRow>,
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
            order_count: count_rows(&connection, "orders")?,
            latest_market_event: latest_market_event(&connection)?,
            heartbeat: heartbeat(&connection)?,
            portfolio: portfolio(&connection)?,
            latest_orders: latest_orders(&connection)?,
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
.label {{ color: #9aa0a6; font-size: 12px; text-transform: uppercase; letter-spacing: .04em; }}
.value {{ margin-top: 6px; font-size: 22px; font-weight: 650; overflow-wrap: anywhere; }}
table {{ width: 100%; border-collapse: collapse; background: #161b22; border: 1px solid #30363d; border-radius: 8px; overflow: hidden; }}
th, td {{ border-bottom: 1px solid #30363d; padding: 10px; text-align: left; font-size: 14px; vertical-align: top; }}
th {{ color: #9aa0a6; font-size: 12px; text-transform: uppercase; letter-spacing: .04em; }}
tr:last-child td {{ border-bottom: 0; }}
.status {{ display: inline-block; padding: 3px 7px; border-radius: 999px; background: #243b2a; color: #9ee493; font-size: 12px; }}
@media (max-width: 720px) {{ main {{ padding: 16px; }} table {{ display: block; overflow-x: auto; }} }}
</style>
</head>
<body>
<main>
<h1>Trader Dashboard</h1>
<div class="muted">Read-only SQLite view. Auto-refreshes every 10 seconds. DB: {}</div>
"#,
        escape_html(db_path)
    );

    render_summary(&mut html, snapshot);
    render_orders(&mut html, &snapshot.latest_orders);

    html.push_str("</main></body></html>");
    html
}

fn render_summary(html: &mut String, snapshot: &Snapshot) {
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
    let latest_price_time = snapshot
        .latest_market_event
        .as_ref()
        .map(|event| format_time(event.recorded_at_ms))
        .unwrap_or_else(|| "n/a".to_string());
    let heartbeat = snapshot
        .heartbeat
        .as_ref()
        .map(|heartbeat| format_time(heartbeat.updated_at_ms))
        .unwrap_or_else(|| "n/a".to_string());
    let heartbeat_age = snapshot
        .heartbeat
        .as_ref()
        .map(|heartbeat| format!("{}s ago", age_seconds(heartbeat.updated_at_ms)))
        .unwrap_or_else(|| "n/a".to_string());
    let run_id = snapshot
        .heartbeat
        .as_ref()
        .map(|heartbeat| heartbeat.run_id.as_str())
        .unwrap_or("n/a");
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
    let portfolio_time = snapshot
        .portfolio
        .as_ref()
        .map(|portfolio| format_time(portfolio.updated_at_ms))
        .unwrap_or_else(|| "n/a".to_string());

    let _ = write!(
        html,
        r#"<section class="grid">
<div class="tile"><div class="label">Market Events</div><div class="value">{}</div></div>
<div class="tile"><div class="label">Orders</div><div class="value">{}</div></div>
<div class="tile"><div class="label">Latest Price</div><div class="value">{}</div><div class="muted">{}</div></div>
<div class="tile"><div class="label">Heartbeat</div><div class="value">{}</div><div class="muted">{}</div></div>
<div class="tile"><div class="label">Run ID</div><div class="value">{}</div></div>
<div class="tile"><div class="label">Portfolio</div><div class="value">{}</div><div class="muted">{}</div></div>
</section>"#,
        snapshot.market_event_count,
        snapshot.order_count,
        escape_html(&latest_price),
        escape_html(&latest_price_time),
        escape_html(&heartbeat),
        escape_html(&heartbeat_age),
        escape_html(run_id),
        escape_html(&portfolio),
        escape_html(&portfolio_time),
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
                escape_html(&format_time(order.recorded_at_ms)),
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

fn format_time(ms: i64) -> String {
    let seconds = ms / 1000;
    let remainder_ms = ms.rem_euclid(1000);
    format!("{seconds}.{remainder_ms:03}")
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
