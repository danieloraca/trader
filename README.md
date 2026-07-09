# trader

Rust trading bot foundation for Raspberry Pi paper-live testing.

## Safe Modes

Use these modes in order:

1. Replay backtest:

```sh
cargo run -- --config config/trader.example.toml --backtest
```

2. Live Kraken ticker with paper execution:

```toml
[exchange]
kind = "paper"

[exchange.kraken]
enable_order_placement = false

[market_data]
kind = "kraken_ticker"
replay_prices = []
```

3. Real Kraken execution only after extended paper-live soak testing.

## Backtest Report

Backtest mode uses `market_data.replay_prices`, the configured strategy, risk limits, fee/slippage assumptions, and simulated fills. It does not call Kraken or write to SQLite.

```sh
cargo run -- --config config/trader.example.toml --backtest
```

Backtest against recorded Pi market data:

```sh
cargo run -- --config config/pi-paper-live.toml --backtest-sqlite /var/lib/trader/trader.sqlite
```

Sweep simple momentum parameters against recorded Pi market data:

```sh
cargo run -- --config config/pi-paper-live.toml --sweep-sqlite /var/lib/trader/trader.sqlite
```

Sweep moving-average crossover parameters against 1m/5m candles built from recorded Pi market data:

```sh
cargo run -- --config config/pi-paper-live.toml --sweep-candles-sqlite /var/lib/trader/trader.sqlite
```

The candle sweep uses a chronological 70/30 train/test split and saves its latest ranked results into SQLite. The dashboard reads those cached rows in the Strategy Research section; it does not recompute sweeps on each page refresh.

Configure cost assumptions and optional CSV output:

```toml
[backtest]
fee_bps = 26
slippage_bps = 5
trade_log_csv_path = "data/backtest-trades.csv"
```

The report includes net P/L, buy-and-hold benchmark, max drawdown, total fees, total slippage, exposure, realized sell win/loss counts, and final balances. When `trade_log_csv_path` is set, each simulated fill is written to CSV.

## Raspberry Pi Install

Build a release binary on the Pi:

```sh
sudo useradd --system --home /var/lib/trader --shell /usr/sbin/nologin trader
sudo mkdir -p /opt/trader /etc/trader /var/lib/trader
sudo chown trader:trader /var/lib/trader
cargo build --release
sudo cp target/release/trader /opt/trader/trader
sudo cp config/pi-paper-live.example.toml /etc/trader/trader.toml
sudo cp deploy/trader.env.example /etc/trader/trader.env
sudo cp deploy/trader.service /etc/systemd/system/trader.service
sudo chown -R root:root /opt/trader /etc/trader
sudo chmod 600 /etc/trader/trader.env
sudo systemctl daemon-reload
sudo systemctl enable trader
sudo systemctl start trader
```

Watch it:

```sh
systemctl status trader
journalctl -u trader -f
```

Stop it gracefully:

```sh
sudo systemctl stop trader
```

The systemd unit sends `SIGTERM`; the app handles it by flushing portfolio state, replay cursor when applicable, and heartbeat before exiting.

## Dashboard

The dashboard is a separate read-only binary. It does not control trading and only reads SQLite.

Build it on the Pi:

```sh
cd /home/user/Development/trader
cargo build --release --bin dashboard
```

Run manually:

```sh
TRADER_DASHBOARD_ADDR=127.0.0.1:3040 \
TRADER_DASHBOARD_DB=/var/lib/trader/trader.sqlite \
target/release/dashboard
```

Install as a systemd service on your current Pi layout:

```sh
sudo cp deploy/trader-dashboard.service /etc/systemd/system/trader-dashboard.service
sudo systemctl daemon-reload
sudo systemctl enable trader-dashboard
sudo systemctl start trader-dashboard
journalctl -u trader-dashboard -f
```

If you want it reachable directly on your LAN, change `TRADER_DASHBOARD_ADDR` in the service to `0.0.0.0:3040`. Keep it behind your trusted local network; there is no authentication in v1.

## Safety Notes

Keep `enable_order_placement = false` until the bot has run in paper-live mode for days. Watch order frequency, risk rejections, DB growth, heartbeat freshness, and restart behavior before considering tiny real orders.
