# BTC/USD spot strategy review — 10 October 2026

## Decision

Keep real order placement disabled. The defensible spot trading rule from this review is **stay in cash** until a fixed rule earns a positive result after realistic costs on a later period that was not used to choose it. The current five-minute RSI rule lost money in all three historical periods tested. None of the slower MA, RSI, breakout, or simple trend-filter grids produced a repeatable positive result across the two older quarters and the later Pi period. This is a finding about the tested rules and data, not proof that no profitable rule exists.

## Data and costs

- **Selection and historical checks:** Kraken's official `XBTUSD_5.csv` five-minute candles for 2026 Q1 (25,915 rows, 1 January–31 March) and Q2 (26,207 rows, 1 April–30 June). [Kraken historical OHLCVT downloads](https://support.kraken.com/articles/360047124832-downloadable-historical-ohlcvt-open-high-low-close-volume-trades-data).
- **Later check:** a consistent copy of the Pi's `market_events` database, 1,609,069 BTC/USD ticker records from 9 July to 10 October 2026. These formed 26,823 five-minute candles; 25 candle slots were missing in one gap. The ticker feed and Kraken trade candles are different price sources.
- **Spot costs:** 80 bps fee and 5 bps slippage on each fill. The fee is Kraken Pro's published Tier 1 spot **taker** rate as of this review, not a claim about this account's actual tier. [Kraken fee schedule](https://www.kraken.com/features/fee-schedule). A completed round trip at unchanged price therefore costs about 170 bps before spread and market impact.
- **Position:** 0.002 BTC per order with the existing $500 per-order cap and $10,000 starting paper balance. The backtest marks any open BTC at the final price; selling it would incur another fee and possible slippage.

## Results

The repository's backtest fills at the signal candle's close. Live paper trading instead acts on the first tick of the next candle, so these backtests give optimistic timing for a strategy chosen from candle closes. All figures below are simulated USD P/L after modeled fees and slippage.

| Fixed rule | Q1 2026 | Q2 2026 | Pi July–October |
| --- | ---: | ---: | ---: |
| Current RSI 21, 25/65, five-minute candles, 0.002 BTC; 80 bps fee | −$666.61 (470 fills) | −$550.59 (473 fills) | −$495.30 (434 fills) |
| Same RSI at the old 26 bps fee | — | — | −$164.32 (434 fills) |
| Cash, no trades | $0 | $0 | $0 |
| Passive 0.002 BTC bought at the start and sold at the end; 80 bps fee | −$41.21 | −$21.53 | +$38.31 |

The old five-minute sweep screened 1,328 rows per quarter, including duplicated 60-second/300-second variants on five-minute data and spot RSI-regime profiles whose protective exits apply only to futures. Its Q1 top `rsi_regime 14:25/65@60` row showed −$6.91 in its training slice and +$4.68 in its selected test slice. Holding those parameters fixed gave **−$45.92 in Q2 and −$15.09 on the Pi period even at the old 26 bps fee**. Selecting the row by its test result makes that slice part of the search, not independent validation.

Additional screens used the existing Rust backtest with 80/5 bps costs and hourly, four-hour, or daily closes:

- MA crossover: fast windows 3/5/8/10 against slow windows 15/30/60/120 (daily windows through 60). No rule with trades made a profit in both Q1 and Q2.
- RSI mean reversion: windows 7/14/21, oversold 25/30/35, overbought 65/70/75. No rule made a profit in both Q1 and Q2.
- Breakout: windows 15/30/60. Positive Pi-period rows lost substantially in both earlier quarters.
- A separate simple trend test bought 0.002 BTC only while the hourly close was above a 7/14/30/60-day average, optionally requiring that average to rise over 1 or 7 days. It used the **next hour's opening price** for fills and 80/5 bps costs. The 30-day variants lost $20–$39 in Q1, were near flat in Q2, and gained $14–$26 in the Pi period; none passed all periods.

These are exploratory parameter grids. The later Pi period was inspected after the earlier screens and should not be reused as a fresh holdout for another search.

## Practical implication

The Pi service is useful for checking connectivity, candle handling, storage, and paper execution. Its current RSI parameters are not supported as a money-making spot strategy. Keep `enable_order_placement = false`; update the Pi's modeled fee to the account's actual tier before interpreting any further paper P/L. Future research can begin with already available Kraken history and reserve genuinely later data for one final check. More data collection alone does not repair a losing rule.
