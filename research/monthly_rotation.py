"""Fixed monthly BTC/ETH/cash rotation test on saved Kraken daily data.

On each month's first day, use the prior completed close to select the asset
with the stronger trailing 365-day price return, if positive. Execute at that
day's open. Each new position deploys at most $200; prior profits stay in cash.
"""

import csv
import datetime as dt
import json
from pathlib import Path

UTC = dt.timezone.utc
FEE, SLIP, CAPITAL = 0.008, 0.0005, 200.0
LAST = dt.date(2026, 10, 9)
DATA = Path(__file__).parent / 'data'


def load(csv_path, json_path):
    source = {}
    for row in csv.reader(open(csv_path)):
        day = dt.datetime.fromtimestamp(int(row[0]), UTC).date()
        source[day] = (float(row[1]), float(row[4]))
    raw = json.load(open(json_path))
    assert raw['error'] == []
    key = next(k for k, v in raw['result'].items() if isinstance(v, list))
    for row in raw['result'][key][:-1]:
        day = dt.datetime.fromtimestamp(int(row[0]), UTC).date()
        values = (float(row[1]), float(row[4]))
        if day in source:
            assert abs(source[day][0] - values[0]) < 0.02
            assert abs(source[day][1] - values[1]) < 0.02
        else:
            source[day] = values
    return source


raw = {
    'BTC': load(DATA / 'kraken-XBTUSD-1440-through-2026Q2.csv',
                DATA / 'kraken-XBTUSD-1440-api-2026-10-10.json'),
    'ETH': load(DATA / 'kraken-ETHUSD-1440-through-2026Q2.csv',
                DATA / 'kraken-ETHUSD-1440-api-2026-10-10.json'),
}
days = []
day = dt.date(2016, 1, 1)
while day <= LAST:
    days.append(day)
    day += dt.timedelta(days=1)
prices = {}
for asset, by_day in raw.items():
    series = []
    previous = None
    gaps = []
    for day in days:
        bar = by_day.get(day)
        if bar is None:
            assert previous is not None
            bar = (previous[1], previous[1])
            gaps.append(day)
        series.append(bar)
        previous = bar
    print(asset, 'filled gaps', gaps)
    prices[asset] = series


def run(rule, start, end, fee=FEE, slippage=SLIP):
    first = days.index(start)
    last = days.index(end)
    cash, asset, qty, fills = CAPITAL, None, 0.0, 0
    peak, drawdown = CAPITAL, 0.0
    selected = []
    for i in range(first, last + 1):
        day = days[i]
        if rule == 'cash':
            choice = None
        elif rule in ('BTC', 'ETH'):
            choice = rule
        elif rule == 'rotate':
            if i == first or day.day == 1:
                assert i >= 366, 'need 365 days of completed prices before a signal'
                trailing = {a: prices[a][i - 1][1] / prices[a][i - 366][1] - 1
                            for a in ('BTC', 'ETH')}
                winner = max(trailing, key=trailing.get)
                choice = winner if trailing[winner] > 0 else None
                selected.append((day, choice, trailing))
            else:
                choice = asset
        else:
            raise ValueError(rule)
        if choice != asset:
            if asset:
                cash += qty * prices[asset][i][0] * (1 - slippage) * (1 - fee)
                qty = 0
                fills += 1
            asset = choice
            if asset:
                spend = min(CAPITAL, cash)
                qty = spend / (prices[asset][i][0] * (1 + slippage) * (1 + fee))
                cash -= spend
                fills += 1
        value = cash + (qty * prices[asset][i][1] if asset else 0)
        peak = max(peak, value)
        drawdown = max(drawdown, 100 * (peak - value) / peak)
    if asset:
        cash += qty * prices[asset][last][1] * (1 - slippage) * (1 - fee)
        fills += 1
    return round(cash - CAPITAL, 2), fills, round(drawdown, 1), selected


def run_half(start, end):
    first = days.index(start)
    last = days.index(end)
    quantities = {a: (CAPITAL / 2) / (prices[a][first][0] * (1 + SLIP) * (1 + FEE))
                  for a in ('BTC', 'ETH')}
    peak, drawdown = CAPITAL, 0.0
    for i in range(first, last + 1):
        value = sum(quantities[a] * prices[a][i][1] for a in quantities)
        peak = max(peak, value)
        drawdown = max(drawdown, 100 * (peak - value) / peak)
    proceeds = sum(quantities[a] * prices[a][last][1] * (1 - SLIP) * (1 - FEE)
                   for a in quantities)
    return round(proceeds - CAPITAL, 2), 4, round(drawdown, 1), []


periods = [
    ('2017-19', dt.date(2017,1,1), dt.date(2019,12,31)),
    ('2020-22', dt.date(2020,1,1), dt.date(2022,12,31)),
    ('2023-24', dt.date(2023,1,1), dt.date(2024,12,31)),
    ('2025', dt.date(2025,1,1), dt.date(2025,12,31)),
    ('2026H1', dt.date(2026,1,1), dt.date(2026,6,30)),
    ('2026H2', dt.date(2026,7,1), LAST),
    ('2025-26', dt.date(2025,1,1), LAST),
]
for label, start, end in periods:
    print(label)
    for rule in ('cash', 'BTC', 'ETH', 'half', 'rotate'):
        pnl, fills, drawdown, selected = (run_half(start, end) if rule == 'half'
                                           else run(rule, start, end))
        print(rule, pnl, fills, drawdown, 'choices',
              {s:sum(a==s for _,a,_ in selected) for s in ('BTC','ETH',None)} if rule=='rotate' else '')
print('2025-26 rotation with zero modeled costs', run('rotate', periods[-1][1], LAST, 0, 0)[:3])
