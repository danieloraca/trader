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

## Longer daily-history check

On 10 October I also fetched Kraken's public daily BTC/USD OHLC endpoint. It returned 720 completed daily candles from 20 October 2024 through 9 October 2026; I excluded the final, unfinished 10 October candle as [Kraken's API documentation](https://docs.kraken.com/api-reference/market-data/get-ohlc-data) instructs. This check used a fixed 0.002 BTC target, 80 bps fee and 5 bps slippage per side, signals formed after a completed daily candle, fills at the **next day's open**, and a modeled sale at each period's end. Each period started with USD 10,000 and no BTC; older closes were available for indicator warmup. Results are USD P/L:

| Daily rule | 2025 | Jan–Jun 2026 | Jul–9 Oct 2026 |
| --- | ---: | ---: | ---: |
| Cash | $0 | $0 | $0 |
| Buy and hold 0.002 BTC | −$14.82 | −$60.42 | +$45.66 |
| Hold only above 50-day moving average | −$33.73 | −$20.49 | +$21.58 |
| Hold only above 100-day moving average | −$46.24 | −$16.77 | +$23.97 |
| Hold only above 200-day moving average | −$7.32 | $0 (no trades) | +$23.97 |
| 50/200-day moving-average crossover | −$38.54 | $0 (no trades) | +$5.48 |
| 20-day high entry / 10-day low exit | −$85.45 | −$16.73 | +$6.12 |

The 200-day filter is the least bad active rule in this small fixed set, but it still lost in 2025 and made only two fills in the recent period. It is not a validated profitable strategy. The recent period was already known to be rising from the Pi data, so it is not a fresh blind holdout.

## Wider-band daily trend idea

I then tested one cost-motivated rule against a much longer [Kraken daily BTC/USD archive](https://support.kraken.com/articles/360047124832-downloadable-historical-ohlcvt-open-high-low-close-volume-trades-data), extended with a saved [Kraken daily OHLC API](https://docs.kraken.com/api-reference/market-data/get-ohlc-data) response. The resulting 4,740 daily bars span 6 October 2013 through 9 October 2026; the last, unfinished API candle was excluded. The study begins in 2017, after the archive's early gaps. The 200-day average uses completed daily closes and the signal fills at the **following day's open**.

The candidate buys when the close rises at least **3% above** its 200-day average, holds while above **3% below** the average, and sells when it drops below that lower band. The 3% band was chosen before this longer-history test to exceed the modeled roughly 1.7% round-trip cost, not tuned to the best backtest row. Each entry uses at most a **$200 trading sleeve**; prior gains remain in cash. Every result includes a modeled final sale so strategies end in cash. Costs are **80 bps fee plus 5 bps slippage per side**. The plain 200-day rule and buy-and-hold are comparators using the same sleeve and costs. These are simulated USD profits after costs, with each row independently starting with $200 cash:

| Period | Buy and hold | Plain 200-day filter | 3% band | Band fills | Band maximum drawdown |
| --- | ---: | ---: | ---: | ---: | ---: |
| 2017–2019 | +$1,254.96 | +$1,253.49 | +$1,293.90 | 8 | 65.1% |
| 2020–2022 | +$253.37 | +$333.68 | +$457.88 | 8 | 45.6% |
| 2023–2024 | +$910.76 | +$306.20 | +$255.87 | 10 | 29.1% |
| 2025 | −$15.73 | −$55.03 | **−$9.54** | 4 | 25.1% |
| Jan–Jun 2026 | −$68.47 | $0 | $0 | 0 | 0% |
| Jul–9 Oct 2026 | +$77.35 | +$34.30 | +$22.37 | 2 | 7.0% |
| Jan 2025–9 Oct 2026, uninterrupted | −$26.14 | −$30.17 | **+$11.77** | 6 | 25.1% |

The band reduced churning, but it still lost money in 2025 and trails a passive BTC position by **$654.89** in 2023–2024. Its recent combined $11.77 gain is only **5.9% of the $200 sleeve over about 21 months**, or about **0.12% of the existing $10,000 paper balance**. The 2026 Jul–Oct gain came after that rising period had already been inspected, so it is not independent validation. The 2017–2022 gains came with 46–65% simulated sleeve drawdowns. A 15 bps slippage assumption, triple the base assumption, lowers the combined recent band gain to $10.50; these small sums are sensitive to execution and the account's actual fees. The final sale at the last daily close is approximate and may not be executable at that exact price.

**Decision:** retain this as a documented research hypothesis, not a Pi trading change. It is not a reliable money-making result at the intended position size. The Pi service can remain stopped; restarting the existing five-minute RSI would reactivate a rule that lost in every tested historical period. Implementing this daily rule in the daemon would also require a 200-day historical warmup on startup and a daily OHLC source, neither of which the current ticker-based candle path provides. A new deployment should be justified by materially stronger evidence, not another month of ticker collection.

The saved inputs and [reproduction script](daily_trend.py) are in `research/data/` and `research/daily_trend.py`. Run `python3 research/daily_trend.py` from the repository root to reproduce the table and cost check. The script asserts that the archive and API overlap agree, and that the test period has no missing days.

## Practical implication

The Pi service is useful for checking connectivity, candle handling, storage, and paper execution. Its current RSI parameters are not supported as a money-making spot strategy. Keep `enable_order_placement = false`; update the Pi's modeled fee to the account's actual tier before interpreting any further paper P/L. Future research can begin with already available Kraken history and reserve genuinely later data for one final check. More data collection alone does not repair a losing rule.
