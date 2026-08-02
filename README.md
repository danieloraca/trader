# trader

Rust trading bot foundation for Raspberry Pi paper-live testing.

## Equity ETF Research

The equity path is an offline research command and is isolated from the Kraken daemon. It reads daily OHLCV CSV data and does not contact a broker or write to the trading database.

Prepare a comma-separated file with strictly increasing daily rows:

```csv
date,open,high,low,close,volume
2025-01-02,100.10,101.20,99.80,100.90,123456
2025-01-03,100.95,102.00,100.50,101.70,135790
```

Run the comparison:

```sh
cargo run --release -- \
  --config config/equity-research.example.toml \
  --backtest-equity-csv data/vwrp-daily.csv
```

Exercise the command with the small synthetic fixture (it is a parser/demo fixture, not research data):

```sh
cargo run -- \
  --config config/equity-research.example.toml \
  --backtest-equity-csv examples/equity-daily-sample.csv
```

The report compares buy-and-hold, finite-cash monthly DCA, moving-average crossover, and channel breakout. Close-based decisions execute at the next session open to avoid look-ahead bias. It reports return, CAGR, annualized volatility, Sharpe ratio, maximum drawdown, turnover, fees, execution friction, exposure, and performance versus buy-and-hold.

The importer validates dates and OHLC relationships but cannot determine whether a vendor adjusted prices for splits or dividends. Set `prices_are_adjusted = true` only when the entire OHLC series is adjusted consistently. Otherwise the result is price return, not a reliable total-return comparison. The initial implementation keeps the portfolio and instrument in the configured instrument currency; `fx_bps` models a conversion charge but not changing FX rates.

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

Sweep moving-average crossover, RSI mean-reversion, regime-filtered RSI, and breakout parameters against 1m/5m candles built from recorded Pi market data:

```sh
cargo run -- --config config/pi-paper-live.toml --sweep-candles-sqlite /var/lib/trader/trader.sqlite
```

The candle sweep uses a chronological 70/30 train/test split, ranks rows with at least 3 test fills first, and saves its latest ranked results into SQLite. It includes MA crossover, RSI mean reversion, regime-filtered RSI, breakout, all-in hold, fixed-size hold, DCA, and trend-filtered DCA research rows. The dashboard reads those cached rows in the Strategy Research section; it does not recompute sweeps on each page refresh.
Sweep alpha means strategy P/L minus full-account buy-and-hold P/L over the same train or test slice. Match alpha means strategy P/L minus a capital-matched passive benchmark: buy-only rows deploy the same total capital at the first buy, while active rows hold their own buy lots to the end of the slice.
A candidate row requires at least 3 test fills, positive test P/L, positive test alpha, positive match alpha, and train P/L no worse than -10 quote units.

Validate active strategies across rolling train/test windows before treating a single split as meaningful:

```sh
cargo run -- --config config/pi-paper-futures-live.toml --walk-forward-sqlite /var/lib/trader/trader.sqlite
```

Regime-filtered RSI permits pullback longs only while price is above a rising long moving average, and rally shorts only while price is below a falling long moving average. Open futures positions use the actual persisted portfolio for full-position take-profit, stop-loss, maximum-holding-period, and regime-invalidation exits. The sweep tests 60- and 120-candle regime windows with `tight`, `balanced`, and `wide` exit profiles. A walk-forward candidate must pass the per-window candidate rule in at least 70% of windows and have positive average P/L, worst-window P/L, average alpha, and average match alpha.

The walk-forward report keeps ranking on net performance and displays average gross P/L, fees, and slippage separately. For paper futures it evaluates every strategy under both the configured `base` and `stress` cost profiles. Its `exits tp/sl/t/r` column counts take-profit, stop-loss, maximum-holding-time, and regime-invalidation exits across all test windows. Perpetual funding is not modeled and is called out in the report.

Configure cost assumptions and optional CSV output:

```toml
[backtest]
fee_bps = 26
slippage_bps = 5
futures_fee_bps = 5
futures_slippage_bps = 5
futures_stress_fee_bps = 5
futures_stress_slippage_bps = 10
trade_log_csv_path = "data/backtest-trades.csv"
```

The futures defaults model Kraken Tier 1 taker fees rather than assuming maker execution. Recheck the exchange fee schedule before using results for live decisions.

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
