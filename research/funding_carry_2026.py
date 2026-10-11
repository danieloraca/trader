"""Illustrative $200 BTC spot/short-perpetual carry cost check.

This is a funding and explicit transaction-cost screen, not a full backtest:
the daily spot close approximates the hourly perpetual notional, and futures
basis, spreads, margin and liquidation risk are not modeled.
"""

import datetime as dt
import json
from pathlib import Path

from daily_trend import load_bars

START = dt.date(2026, 1, 1)
END = dt.date(2026, 10, 9)
INITIAL_SPOT_USD = 200.0
SPOT_FEE = 0.008
SPOT_SLIPPAGE = 0.0005
FUTURES_TAKER_FEE = 0.0005
DATA = Path(__file__).parent / "data"
FUNDING = DATA / "kraken-PF_XBTUSD-funding-api-2026-10-11.json"


def main() -> None:
    spot = {bar.day: bar for bar in load_bars()}
    with FUNDING.open() as source:
        response = json.load(source)
    assert response["result"] == "success"

    entry_price = spot[START].opening
    exit_price = spot[END].closing
    btc_quantity = INITIAL_SPOT_USD / entry_price
    end_notional = btc_quantity * exit_price
    rate_by_hour = {}
    for row in response["rates"]:
        timestamp = dt.datetime.fromisoformat(row["timestamp"].replace("Z", "+00:00"))
        if START <= timestamp.date() <= END:
            assert timestamp not in rate_by_hour
            rate_by_hour[timestamp] = float(row["relativeFundingRate"])

    first_hour = dt.datetime.combine(START, dt.time(0), dt.timezone.utc)
    hours = (END - START).days * 24 + 24
    missing = [first_hour + dt.timedelta(hours=i) for i in range(hours)
               if first_hour + dt.timedelta(hours=i) not in rate_by_hour]
    assert len(missing) <= 3, "unexpectedly incomplete funding history"

    # Kraken's positive funding rate means the short receives funding. The
    # daily spot close stands in for the unrecorded hourly notional.
    funding = sum(btc_quantity * spot[timestamp.date()].closing * rate
                  for timestamp, rate in rate_by_hour.items())
    spot_costs = (INITIAL_SPOT_USD + end_notional) * (SPOT_FEE + SPOT_SLIPPAGE)
    futures_fees = (INITIAL_SPOT_USD + end_notional) * FUTURES_TAKER_FEE

    print(f"Period: {START} to {END}")
    print(f"Funding hours: {len(rate_by_hour)}/{hours}; missing: {len(missing)}")
    print(f"Initial BTC quantity: {btc_quantity:.8f}")
    print(f"Estimated gross short funding: ${funding:.2f}")
    print(f"Estimated spot fees and slippage: ${spot_costs:.2f}")
    print(f"Estimated futures taker fees: ${futures_fees:.2f}")
    print(f"Funding less explicit costs: ${funding - spot_costs - futures_fees:.2f}")


if __name__ == "__main__":
    main()
