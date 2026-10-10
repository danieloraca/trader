"""Reproduce the fixed BTC/USD daily trend comparison in the spot review.

The Kraken archive provides history through 2026-06-30. The saved OHLC API
response extends it through 2026-10-09; its last, unfinished candle is ignored.
All signals use yesterday's completed close and fill at today's opening price.
"""

import csv
import datetime as dt
import json
from collections import deque
from dataclasses import dataclass
from pathlib import Path

DATA = Path(__file__).parent / "data"
ARCHIVE = DATA / "kraken-XBTUSD-1440-through-2026Q2.csv"
RECENT = DATA / "kraken-XBTUSD-1440-api-2026-10-10.json"
UTC = dt.timezone.utc
CAPITAL = 200.0
FEE = 0.008
SLIPPAGE = 0.0005
SMA_DAYS = 200
BAND = 0.03


@dataclass(frozen=True)
class Bar:
    day: dt.date
    opening: float
    closing: float


@dataclass(frozen=True)
class Result:
    pnl: float
    fills: int
    days_held: int
    max_drawdown_pct: float


def date_of(timestamp: int) -> dt.date:
    assert timestamp % 86400 == 0, "daily candle must open at UTC midnight"
    return dt.datetime.fromtimestamp(timestamp, UTC).date()


def load_bars() -> list[Bar]:
    bars: list[Bar] = []
    archive_by_day: dict[dt.date, Bar] = {}
    with ARCHIVE.open(newline="") as source:
        for row in csv.reader(source):
            bar = Bar(date_of(int(row[0])), float(row[1]), float(row[4]))
            assert bar.day not in archive_by_day
            archive_by_day[bar.day] = bar
            bars.append(bar)

    with RECENT.open() as source:
        response = json.load(source)
    assert response["error"] == []
    pair = next(key for key, value in response["result"].items() if isinstance(value, list))
    for row in response["result"][pair][:-1]:
        bar = Bar(date_of(int(row[0])), float(row[1]), float(row[4]))
        old = archive_by_day.get(bar.day)
        if old is not None:
            assert abs(old.opening - bar.opening) < 0.02
            assert abs(old.closing - bar.closing) < 0.02
        elif bar.day > bars[-1].day:
            bars.append(bar)

    assert bars[0].day == dt.date(2013, 10, 6)
    assert bars[-1].day == dt.date(2026, 10, 9)
    for previous, current in zip(bars, bars[1:]):
        if current.day >= dt.date(2017, 1, 1):
            assert (current.day - previous.day).days == 1, "missing test-period day"
    return bars


def averages(bars: list[Bar]) -> list[float | None]:
    values: deque[float] = deque()
    total = 0.0
    result: list[float | None] = []
    for bar in bars:
        values.append(bar.closing)
        total += bar.closing
        if len(values) > SMA_DAYS:
            total -= values.popleft()
        result.append(total / SMA_DAYS if len(values) == SMA_DAYS else None)
    return result


def simulate(
    bars: list[Bar], moving_averages: list[float | None], rule: str,
    start: dt.date, end: dt.date, fee: float = FEE, slippage: float = SLIPPAGE,
) -> Result:
    first = next(i for i, bar in enumerate(bars) if bar.day >= start)
    last = max(i for i, bar in enumerate(bars) if bar.day <= end)
    cash, btc, fills, days_held = CAPITAL, 0.0, 0, 0
    peak, max_drawdown_pct = CAPITAL, 0.0
    for i in range(first, last + 1):
        bar = bars[i]
        if rule == "cash":
            wanted = False
        elif rule == "hold":
            wanted = True
        else:
            previous_close, previous_sma = bars[i - 1].closing, moving_averages[i - 1]
            if previous_sma is None:
                wanted = False
            elif rule == "sma200":
                wanted = previous_close > previous_sma
            elif rule == "band3":
                threshold = 1 - BAND if btc else 1 + BAND
                wanted = previous_close >= previous_sma * threshold
            else:
                raise ValueError(f"unknown rule: {rule}")

        if wanted and btc == 0:
            # Each entry deploys at most $200. Gains from earlier trades stay in cash.
            spend = min(CAPITAL, cash)
            if spend > 0:
                btc = spend / (bar.opening * (1 + slippage) * (1 + fee))
                cash -= spend
                fills += 1
        elif not wanted and btc > 0:
            cash += btc * bar.opening * (1 - slippage) * (1 - fee)
            btc = 0
            fills += 1

        days_held += btc > 0
        value = cash + btc * bar.closing
        peak = max(peak, value)
        max_drawdown_pct = max(max_drawdown_pct, 100 * (peak - value) / peak)

    # Compare all rules in cash at the period's end. This last-close sale is an
    # approximation; it still pays modeled fee and slippage on the exit.
    if btc:
        cash += btc * bars[last].closing * (1 - slippage) * (1 - fee)
        fills += 1
    return Result(cash - CAPITAL, fills, days_held, max_drawdown_pct)


def main() -> None:
    bars = load_bars()
    moving_averages = averages(bars)
    periods = [
        ("2017–2019", dt.date(2017, 1, 1), dt.date(2019, 12, 31)),
        ("2020–2022", dt.date(2020, 1, 1), dt.date(2022, 12, 31)),
        ("2023–2024", dt.date(2023, 1, 1), dt.date(2024, 12, 31)),
        ("2025", dt.date(2025, 1, 1), dt.date(2025, 12, 31)),
        ("2026 H1", dt.date(2026, 1, 1), dt.date(2026, 6, 30)),
        ("2026 Jul–Oct 9", dt.date(2026, 7, 1), dt.date(2026, 10, 9)),
        ("2025–Oct 9 2026", dt.date(2025, 1, 1), dt.date(2026, 10, 9)),
    ]
    print(f"{len(bars)} daily bars: {bars[0].day} to {bars[-1].day}")
    print("period,rule,pnl_usd,return_pct,fills,exposure_pct,max_drawdown_pct")
    for label, start, end in periods:
        days = (end - start).days + 1
        for rule in ("cash", "hold", "sma200", "band3"):
            result = simulate(bars, moving_averages, rule, start, end)
            print(f"{label},{rule},{result.pnl:.2f},{result.pnl / CAPITAL * 100:.2f},"
                  f"{result.fills},{result.days_held / days * 100:.1f},"
                  f"{result.max_drawdown_pct:.1f}")
    print("cost_sensitivity_2025_to_2026_oct9,fee_bps,slippage_bps,pnl_usd")
    for fee, slippage in ((0.008, 0.0005), (0.0026, 0.0005), (0.008, 0.0015)):
        result = simulate(bars, moving_averages, "band3", periods[-1][1], periods[-1][2], fee, slippage)
        print(f"band3,{fee * 10000:.0f},{slippage * 10000:.0f},{result.pnl:.2f}")


if __name__ == "__main__":
    main()
