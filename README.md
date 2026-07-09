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

Backtest mode uses `market_data.replay_prices`, the configured strategy, risk limits, and paper fills. It does not call Kraken or write to SQLite.

```sh
cargo run -- --config config/trader.example.toml --backtest
```

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

## Safety Notes

Keep `enable_order_placement = false` until the bot has run in paper-live mode for days. Watch order frequency, risk rejections, DB growth, heartbeat freshness, and restart behavior before considering tiny real orders.
