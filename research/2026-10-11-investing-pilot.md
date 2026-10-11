# £10 long-term investing pilot (11 October 2026)

## Decision

- Keep `trader.service` stopped. The crypto strategies tested so far have no demonstrated net edge, and this pilot needs no Raspberry Pi service.
- Treat £10 as a one-off test of the investing process, separate from existing investments. It is too small to test an expected return or the existing whole-share VWRP/VAGS planner.
- Do not infer a personal 80/20 allocation from the historical portfolio config. The target mix, ongoing contribution, and investment horizon still need to be chosen for any larger plan.

## Practical test

1. If the money can stay invested for at least five years and a real-money test is wanted, use a regulated UK investment account that permits fractional ETF purchases and has no fixed dealing charge. A Stocks and Shares ISA is an option if there is unused ISA allowance. Check the provider's current fees and the order preview before buying.
2. A possible single-fund test is a £10 fractional purchase of the GBP-listed VWRP global equity ETF, manually through the broker. This is a process test, not a forecast or an instruction to trade. The ETF can lose value; a short horizon calls for cash instead.
3. Record the deposit, executed quantity, price, fees, spread, and order date. Review the account process after settlement. Do not judge a long-term investment strategy from a few weeks of £10 returns.
4. Before any recurring or larger amount, set the actual time horizon, sustainable contribution, and tolerable drawdown. Then choose the stock/bond split and compare total broker, fund, and trading costs. Refresh both price histories before using the repo's contribution planner.

## Current constraints and sources

- Vanguard's VWRP page shows a GBP market close of £146.24 on 9 October 2026 and a 0.14% ongoing charge. A whole share is above this pilot budget: https://www.vanguard.co.uk/professional/product/etf/equity/9679/ftse-all-world-ucits-etf-usd-accumulating
- Trading 212 is one example of a platform that lists VWRP as fractionally tradable. Its published UK ISA terms currently say £1 minimum deposit and ETF order, no trading commission or custody fee, and 0.15% FX fee where currency conversion applies. The GBP listing avoids an order-currency FX conversion, but market spread and fund charges remain: https://www.trading212.com/trading-instruments/invest/VWRP.GB and https://www.trading212.com/terms/isa
- HMRC's 2026/27 overall ISA allowance is £20,000, shared across ISAs: https://www.gov.uk/individual-savings-accounts
- The FCA discusses investing for five years or more to allow time to ride out short-term moves: https://www.fca.org.uk/investsmart/golden-rules-investing
- The repo's price histories end on 31 July 2026, the holdings file is a placeholder, and the contribution planner only supports whole shares. It now refuses to suggest buys from prices more than seven calendar days old.
