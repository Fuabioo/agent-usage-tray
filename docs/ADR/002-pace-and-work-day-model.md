# ADR-002: Pace Coloring and the Work-Day Model

| Field      | Value                                  |
| ---------- | -------------------------------------- |
| Status     | Accepted                               |
| Date       | 2026-06-12                             |
| Supersedes | ADR-001 §4 (weekly pace coloring)      |
| Authors    | fuabioo                                |

## Context

The weekly window is colored by **pace**: are you on track to spend your weekly budget evenly
across your work days, or burning it too fast? ADR-001 ported the original tool's algorithm
verbatim — `ratio = used / ceiling`, where `ceiling = work_day_index × daily_budget`, colored
green `< 0.75`, yellow `< 1.0`, red `≥ 1.0`. Two problems surfaced in real use:

1. **The cumulative ratio warns too early late in the week.** On work day 4 of 5 (ceiling 80%)
   with 60% used, `ratio = 0.75` → yellow. But 60% by day 4 is *exactly three days' worth* — a
   full day **under** pace, with today's entire allowance untouched. That should be green.

2. **The work-day count was wrong across timezones and reset times.** Days were counted from
   reset-aligned periods using UTC weekdays. A weekly reset at, say, 8 pm local lands at 2 am the
   *next* UTC date, so UTC counting saw the cycle "start Tuesday" and Friday read as day 4 instead
   of 5 — then a naive "count local calendar dates from the cycle start" over-corrected to 5 by
   double-counting the cycle-start Monday (whose 9–5 work happened *before* the 8 pm reset, i.e.
   in the previous cycle).

We also wanted a fourth state above green to celebrate being well ahead ("surplus").

## Decision

### 1. Color by *today's headroom*, not the cumulative ratio

`remaining = ceiling − used` is how much of the budget you can still spend today and stay on pace.
Bands are fractions of one day's budget, so they scale with `daily_budget` (for the default
20%/day they are the numbers in parentheses):

| Band        | Condition                              | Default (20%/day) |
| ----------- | -------------------------------------- | ----------------- |
| **Surplus** | `remaining ≥ 2 × daily_budget`         | ≥ 40% left        |
| **Green**   | `remaining > 0.5 × daily_budget`       | > 10% left        |
| **Yellow**  | `remaining > 0.25 × daily_budget`      | 5–10% left        |
| **Red**     | otherwise (incl. over the ceiling)     | ≤ 5% left or over |

Surplus = a full day's budget or more ahead of pace (banked budget you can spend freely). It is a
real `PaceColor` variant emitted by the core (tag `"surplus"`), so every frontend gets it; the
macOS app renders it mint with a glow. Session windows keep fixed thresholds (`≤50`/`≤80`); credit
pools are colored by burn-rate projection (red if projected to deplete before reset or `≥90%`
used, yellow at `≥75%`).

### 2. Count work days in local time, attributed to when the work happens

The cycle (`resets_at − 7 days` → `resets_at`) is divided into reset-aligned 24-hour periods. Each
period is attributed to **the local calendar day its working hours fall on**, approximated by the
date **12 hours into the period**:

- An **evening** reset (8 pm) maps a period (Mon 8 pm → Tue 8 pm) to *Tuesday's* work.
- A **morning** reset (6 am) maps a period to the *same* day's work.

Weekends are skipped by that attributed day (for `work_days ≤ 5`); the current partial period is
included; the result is clamped to `[1, work_days]`. The computation is timezone-generic (the CLI
passes `Local`; tests pass a fixed offset for determinism).

**Worked example** — reset Monday 8 pm, 5 work days: the cycle's work days are Tue, Wed, Thu, Fri,
and the *next* Monday (its 9–5 is before the next 8 pm reset). The cycle-start Monday's daytime
work belongs to the previous cycle. So Friday is **day 4 of 5**.

## Consequences

- **Positive:** coloring matches intuition (a full day under pace is green/surplus, not a
  warning); "day N/M" matches the wall clock regardless of timezone or reset time; the model is
  the same for every agent and every frontend because it lives in the core. Thresholds and the
  work-day attribution are pinned by unit tests (including the Monday-8 pm and timezone cases).
- **Negative:** the `+12h` attribution is a heuristic for "when working hours fall" and is
  ambiguous for unusual midday resets; we accept that as rare.

## Alternatives considered

- **Keep the cumulative `used/ceiling` ratio** — rejected: warns a full day early late in the
  week (the original bug report).
- **Count work days by UTC weekday** (the original) — rejected: off by one when the reset time
  crosses midnight relative to local time.
- **Count distinct local calendar dates from the cycle start, inclusive** — rejected: double-counts
  the cycle-start day, whose work is in the previous cycle (made Friday read 5/5 instead of 4/5).
- **Absolute thresholds (e.g. red at a literal 5%)** — rejected: they don't scale when `work_days`
  changes the daily budget; fractions of a day's budget do.
