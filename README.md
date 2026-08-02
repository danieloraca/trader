# trader

Rust trading bot foundation for Raspberry Pi paper-live testing.

## Equity ETF Research

The equity path is an offline research command and is isolated from the Kraken daemon. It reads daily OHLCV CSV files or official close-only Excel histories and does not contact a broker or write to the trading database.

For VWRP, open Vanguard's [fund page](https://www.vanguard.co.uk/uk-fund-directory/product/etf/equity/9679/ftse-all-world-ucits-etf-usd-accumulating), select **Prices and distribution**, and use **Download prices** under **Historical Prices**. The downloaded `.xlsx` can be used directly:

```sh
target/release/trader \
  --config config/equity-research.example.toml \
  --backtest-equity data/vwrp-history.xlsx
```

The spreadsheet importer finds the header row, selects `Market price (GBP)` ahead of the USD NAV, accepts Vanguard's displayed date format and currency symbols, and reverses newest-first files into chronological order. Rows without a GBP market price, such as London market holidays where Vanguard only publishes a USD NAV, are omitted and counted in the report.

Other providers can supply a comma-separated file with daily rows:

```csv
date,open,high,low,close,volume
2025-01-02,100.10,101.20,99.80,100.90,123456
2025-01-03,100.95,102.00,100.50,101.70,135790
```

Run the comparison:

```sh
cargo run --release -- \
  --config config/equity-research.example.toml \
  --backtest-equity data/vwrp-daily.csv
```

Exercise the command with the small synthetic fixture (it is a parser/demo fixture, not research data):

```sh
cargo run -- \
  --config config/equity-research.example.toml \
  --backtest-equity examples/equity-daily-sample.csv
```

The Vanguard-shaped close-only fixture is available at
`examples/equity-vanguard-close-sample.csv`.

The report compares buy-and-hold, finite-cash monthly DCA, moving-average crossover, and channel breakout. Close-based decisions execute at the next session open for OHLCV data and at the next session close for close-only data, avoiding look-ahead bias in both cases. Close-only breakout channels use closing prices. The report includes return, CAGR, annualized volatility, Sharpe ratio, maximum drawdown, turnover, fees, execution friction, exposure, and performance versus buy-and-hold.

Run the rolling equity parameter sweep against the same file:

```sh
target/release/trader \
  --config config/equity-research.example.toml \
  --walk-forward-equity data/vwrp-history.xlsx
```

The default walk-forward plan uses three rolling training years and one-year non-overlapping held-out test windows. It tests configurable MA and breakout grids, warms each strategy only from its training window, resets the portfolio to the configured initial cash for every test window, and compares held-out results with cash and buy-and-hold. Reviewing the report consumes those test windows for research; selected parameters still require later unseen data before deployment.

The importer validates dates and OHLC relationships but cannot determine whether a vendor adjusted prices for splits or dividends. Set `prices_are_adjusted = true` only when the entire OHLC series is adjusted consistently. Otherwise the result is price return, not a reliable total-return comparison. The initial implementation keeps the portfolio and instrument in the configured instrument currency; `fx_bps` models a conversion charge but not changing FX rates.

## Multi-ETF Portfolio Research

Portfolio research loads two or more daily price histories, restricts them to common trading dates, and compares cash, each asset held alone, the configured static allocation, and periodic rebalancing. Run the synthetic 70/30 equity-bond example:

```sh
target/release/trader \
  --config config/equity-portfolio.example.toml \
  --backtest-equity-portfolio
```

Portfolio asset paths are resolved relative to the config file. Target weights use basis points and must total `10000`; for example, `7000` and `3000` represent 70% and 30%. Set `monthly_contribution` to model regular deposits. New cash is invested into underweight assets first; scheduled monthly, quarterly, or yearly rebalancing only sells when the drift threshold is breached. Rebalances are decided from the previous common-session close and execute on the next common session. Net P/L excludes deposits, while return, CAGR, volatility, Sharpe, and drawdown use a time-weighted cash-flow-adjusted curve. All inputs must represent prices in the configured portfolio currency because changing FX rates are not modeled.

Compare allocation weights and rebalance policies over rolling, non-overlapping held-out periods:

```sh
target/release/trader \
  --config config/equity-portfolio.example.toml \
  --walk-forward-equity-portfolio
```

Configure allocation vectors under `[equity_portfolio.walk_forward]`; every vector must match the asset order and total `10000` basis points. If omitted, a two-asset portfolio automatically tests weights in 10% steps. Each held-out window starts with the configured cash balance. The report ranks combinations by average held-out Sharpe and also shows average and worst return, worst drawdown, turnover, and return versus holding the first configured asset alone.

For independent proxy validation, download the complete historical-price spreadsheets for Vanguard's [FTSE Global All Cap Index Fund GBP Acc](https://www.vanguard.co.uk/professional/product/fund/equity/8617/ftse-global-all-cap-index-fund-gbp-acc) and [Global Bond Index Fund GBP Hedged Acc](https://www.vanguard.co.uk/professional/product/fund/bond/9142/global-bond-index-fund-hedged-acc). Save them as `global-equity-proxy-hist.xlsx` and `global-bond-proxy-hist.xlsx` in the repository root, then run:

```sh
target/release/trader \
  --config config/equity-portfolio-proxy.example.toml \
  --walk-forward-equity-portfolio
```

The proxy configuration requires at least 2,000 common sessions and five held-out windows. It uses accumulation-fund NAVs and fractional units to avoid proxy unit-price differences distorting the allocation test. These proxies validate the allocation concept over a longer, different history; they do not represent directly executable ETF prices.

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
