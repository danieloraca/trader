# Kraken BTC spot/perpetual funding-carry screen — 11 October 2026

## Question and decision

Could a small, roughly market-neutral position—buy BTC spot and short the same BTC quantity in a perpetual future—earn enough funding to cover its costs? **The recent observed funding offers no usable margin at the intended $200 trade size. Do not build or deploy a futures trader from this result.** The spot RSI trader remains stopped.

## Source and method

- Saved [Kraken public historical funding rates](https://docs.kraken.com/api-reference/historical-funding-rates/historical-funding-rates) for `PF_XBTUSD`, queried on 11 October 2026, and the saved BTC/USD daily prices used in the [spot strategy review](2026-10-10-spot-review.md). The calculation covers 1 January–9 October 2026. The funding API has 6,765 of the expected 6,768 hourly observations in that period; the three absent hours contribute zero in this estimate.
- On 1 January, buy approximately $200 of BTC at that day's Kraken spot open and short the same BTC quantity in the perpetual. Keep the BTC quantity fixed. Estimate each hourly funding payment using that day's spot close as the contract notional; actual hourly prices and perpetual prices can differ. On 9 October, estimate closing both legs.
- Kraken states that [positive perpetual funding is paid by longs to shorts](https://support.kraken.com/articles/4844359082772-linear-multi-collateral-derivatives-contract-specifications). The calculation uses those signed historical rates; it is **not** a full hedge P/L backtest.
- Assume [Kraken Pro spot Tier 1 taker fee of 0.80%](https://www.kraken.com/features/fee-schedule) and 0.05% spot slippage on **each** spot fill; assume [derivatives Tier 1 taker fee of 0.05%](https://support.kraken.com/articles/360048917612-fee-schedule) on **each** futures fill, with zero futures slippage for this generous screen. Actual account fees may differ.

| $200 initial spot position, 1 Jan–9 Oct 2026 | USD estimate |
| --- | ---: |
| Funding received by short perpetual | +$3.37 |
| Spot entry and exit fees plus modeled slippage | −$3.30 |
| Futures entry and exit taker fees | −$0.19 |
| **Remainder before other costs and risks** | **−$0.13** |

The remainder excludes spot/perpetual entry and exit price differences, futures spread and slippage, changes in the hedge ratio, collateral costs, liquidation risk, and any missing funding observations. Those effects can be larger than the apparent few-cent margin. The daily-close approximation also limits precision, so treat the result as **roughly break-even before unmodeled costs**, not a measured trading loss.

There is also an eligibility gate: [Kraken says UK retail clients cannot trade derivatives; UK access is limited to eligible professional clients](https://support.kraken.com/articles/360023786632-kraken-derivatives-eligibility). We do not know the user's account classification or residence, so this is conditional rather than an assertion about their account.

## Reproduce

Run `python3 research/funding_carry_2026.py` from the repository root. It reads the saved funding API response in `research/data/` and the saved BTC daily candles. This is an offline research artifact; nothing here changes the Pi service.
