//! Pace-based color coding.
//!
//! The weekly window is colored by **pace**: given how many work days into the cycle we are,
//! we compute a ceiling (how much budget *should* be spent by now) and compare. Session
//! windows use fixed thresholds (too short-term to budget by day). Credit pools are colored by
//! whether they're projected to run dry before they refill (see [`crate::projection`]).
//!
//! The weekday-counting logic is ported verbatim from the original `cc-usage` tool so the new
//! agent-agnostic core stays bug-for-bug compatible with the behavior users already trust.

use crate::schema::{Window, WindowKind};
use chrono::{DateTime, Datelike, Local, TimeZone, Utc, Weekday};

const CYCLE_LENGTH: i64 = 7;

/// Pace-based color indicator. Surplus = a full day or more ahead (banked budget), Green = on
/// track, Yellow = approaching the line, Red = over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaceColor {
    Surplus,
    Green,
    Yellow,
    Red,
}

impl PaceColor {
    /// Lowercase tag for the JSON contract.
    pub fn tag(self) -> &'static str {
        match self {
            PaceColor::Surplus => "surplus",
            PaceColor::Green => "green",
            PaceColor::Yellow => "yellow",
            PaceColor::Red => "red",
        }
    }
}

/// Computes the pace color for a multi-day budget (weekly) window.
///
/// Colors by **today's headroom**, not the cumulative ratio. The ceiling is
/// `work_day_index * daily_budget`; `remaining = ceiling - utilization` is how much you can
/// still spend today and stay on pace. Thresholds are fractions of a day's budget so they scale
/// with `daily_budget` (for the default 20%/day: surplus ≥ 40%, green > 10%, yellow 5–10%,
/// red ≤ 5%):
///
/// - **Surplus** — a full day's budget or more ahead of pace (≥ 2 days of headroom banked).
/// - **Green**   — more than half a day's budget of headroom left.
/// - **Yellow**  — between a quarter and half a day's budget left (getting low).
/// - **Red**     — a quarter day's budget or less left, or already over.
///
/// Colors by headroom (not a `used / ceiling` ratio) so that being a full day under pace late
/// in the week reads Green rather than wrongly warning at exactly 0.75.
pub fn compute_weekly_pace_color(
    utilization: f64,
    daily_budget: f64,
    work_days: u8,
    resets_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> PaceColor {
    weekly_pace_color_in(utilization, daily_budget, work_days, resets_at, now, &Local)
}

fn weekly_pace_color_in<Tz: TimeZone>(
    utilization: f64,
    daily_budget: f64,
    work_days: u8,
    resets_at: DateTime<Utc>,
    now: DateTime<Utc>,
    tz: &Tz,
) -> PaceColor {
    if daily_budget <= 0.0 {
        return PaceColor::Red; // defensive
    }

    let work_day_index = work_days_elapsed(resets_at, now, work_days, tz);
    let ceiling = work_day_index as f64 * daily_budget;
    let remaining = ceiling - utilization;

    if remaining >= 2.0 * daily_budget {
        PaceColor::Surplus // a full day or more ahead of pace — banked budget
    } else if remaining > 0.5 * daily_budget {
        PaceColor::Green
    } else if remaining > 0.25 * daily_budget {
        PaceColor::Yellow
    } else {
        PaceColor::Red
    }
}

/// Computes the color for a short rolling (session) window using fixed thresholds:
/// `<= 50` Green, `<= 80` Yellow, else Red.
pub fn compute_session_color(utilization: f64) -> PaceColor {
    if utilization <= 50.0 {
        PaceColor::Green
    } else if utilization <= 80.0 {
        PaceColor::Yellow
    } else {
        PaceColor::Red
    }
}

/// Computes the color for a consumable credit pool.
///
/// A pool projected to deplete before it refills is always Red (you'll run out). Otherwise it
/// falls back to consumption thresholds: `>= 90%` used Red, `>= 75%` used Yellow, else Green.
pub fn compute_pool_color(used_pct: f64, depletes_before_reset: bool) -> PaceColor {
    if depletes_before_reset || used_pct >= 90.0 {
        PaceColor::Red
    } else if used_pct >= 75.0 {
        PaceColor::Yellow
    } else {
        PaceColor::Green
    }
}

/// Counts work days elapsed in the cycle, in the user's **local** timezone.
///
/// The cycle starts at `resets_at - 7 days` and is divided into reset-aligned 24h periods. Each
/// period is attributed to the **local calendar day its working hours fall on** — approximated by
/// the date 12h into the period, so an evening reset maps a period to the *next* day's work and a
/// morning reset to the same day. For `work_days <= 5` only Mon–Fri periods count; for
/// `work_days > 5` every period counts. The current (partial) period is included. Clamped to
/// `[1, work_days]`.
///
/// This is what makes "day N/M" match the wall clock. Example: a reset Monday 8 pm means the
/// cycle's first period (Mon 8 pm → Tue 8 pm) is *Tuesday's* work day, the Monday-daytime work
/// before the reset belongs to the previous cycle, and the next Monday (its 9–5 is before the
/// next 8 pm reset) is the cycle's 5th work day — so Friday reads as day 4 of 5, not 5.
pub fn days_into_cycle(resets_at: DateTime<Utc>, now: DateTime<Utc>, work_days: u8) -> u8 {
    work_days_elapsed(resets_at, now, work_days, &Local)
}

fn work_days_elapsed<Tz: TimeZone>(
    resets_at: DateTime<Utc>,
    now: DateTime<Utc>,
    work_days: u8,
    tz: &Tz,
) -> u8 {
    let cycle_start = resets_at - chrono::Duration::days(CYCLE_LENGTH);
    if now <= cycle_start {
        return 1;
    }

    let total_periods = ((now - cycle_start).num_days() + 1).clamp(1, CYCLE_LENGTH);

    let mut count: u8 = 0;
    for i in 0..total_periods {
        // The calendar day a reset-period's working hours fall on (period start + 12h).
        let work_day = (cycle_start + chrono::Duration::days(i) + chrono::Duration::hours(12))
            .with_timezone(tz)
            .date_naive();
        let weekend = matches!(work_day.weekday(), Weekday::Sat | Weekday::Sun);
        if work_days > 5 || !weekend {
            count = count.saturating_add(1);
        }
    }

    count.clamp(1, work_days)
}

/// The reset day as "Wed Apr 1, 3:00 PM" in the local timezone.
pub fn reset_day_name(resets_at: DateTime<Utc>) -> String {
    use chrono::Local;
    let local = resets_at.with_timezone(&Local);
    local.format("%a %b %-d, %-I:%M %p").to_string()
}

/// Default per-window color for a window that does *not* need pace context (no budget config,
/// no projection). Weekly windows can't be paced without config, so they fall back to session
/// thresholds here; callers with budget config should use [`compute_weekly_pace_color`].
pub fn default_window_color(window: &Window) -> PaceColor {
    match window.kind {
        WindowKind::Credits => compute_pool_color(window.used_pct(), false),
        _ => compute_session_color(window.used_pct()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    // The public functions use the machine's local timezone; the tests pin a fixed timezone so
    // they're deterministic regardless of where they run.
    fn wpc(u: f64, b: f64, wd: u8, r: DateTime<Utc>, n: DateTime<Utc>) -> PaceColor {
        weekly_pace_color_in(u, b, wd, r, n, &Utc)
    }
    fn wde(r: DateTime<Utc>, n: DateTime<Utc>, wd: u8) -> u8 {
        work_days_elapsed(r, n, wd, &Utc)
    }

    #[test]
    fn day1_zero_is_green() {
        let now = utc(2024, 3, 7, 12, 0);
        let resets = utc(2024, 3, 14, 9, 0);
        assert_eq!(wpc(0.0, 20.0, 5, resets, now), PaceColor::Green);
    }

    #[test]
    fn day1_over_ceiling_is_red() {
        let now = utc(2024, 3, 7, 12, 0);
        let resets = utc(2024, 3, 14, 9, 0);
        assert_eq!(wpc(22.0, 20.0, 5, resets, now), PaceColor::Red);
    }

    #[test]
    fn zero_budget_is_red() {
        let now = utc(2024, 3, 7, 12, 0);
        let resets = utc(2024, 3, 14, 9, 0);
        assert_eq!(wpc(10.0, 0.0, 5, resets, now), PaceColor::Red);
    }

    // Cycle: resets Thu Mar 14 09:00, started Thu Mar 7. Tue Mar 12 = work day 4 (Thu, Fri,
    // Mon, Tue), so ceiling = 4 * 20 = 80.
    #[test]
    fn day4_a_full_day_under_pace_is_green() {
        let now = utc(2024, 3, 12, 12, 0); // work day 4
        let resets = utc(2024, 3, 14, 9, 0);
        // 60% used = exactly day-3's ceiling; a whole day's budget (20%) still available.
        assert_eq!(
            wpc(60.0, 20.0, 5, resets, now),
            PaceColor::Green,
            "a full day under pace must be green, not a warning"
        );
        // Still green with more than half a day's budget left (remaining 15 > 0.5*20).
        assert_eq!(wpc(65.0, 20.0, 5, resets, now), PaceColor::Green);
    }

    #[test]
    fn way_under_pace_is_surplus() {
        let now = utc(2024, 3, 12, 12, 0); // work day 4, ceiling 80
        let resets = utc(2024, 3, 14, 9, 0);
        // 5% used at day 4 -> remaining 75 (≥ 2 days banked) -> surplus.
        assert_eq!(wpc(5.0, 20.0, 5, resets, now), PaceColor::Surplus);
        // Exactly one full day ahead (remaining 40 = 2*daily) -> surplus.
        assert_eq!(wpc(40.0, 20.0, 5, resets, now), PaceColor::Surplus);
        // Just under one day ahead (remaining 39) -> green, not surplus.
        assert_eq!(wpc(41.0, 20.0, 5, resets, now), PaceColor::Green);
    }

    #[test]
    fn day4_color_bands() {
        let now = utc(2024, 3, 12, 12, 0); // work day 4, ceiling 80
        let resets = utc(2024, 3, 14, 9, 0);
        // remaining 8 (between a quarter and half a day) -> yellow.
        assert_eq!(wpc(72.0, 20.0, 5, resets, now), PaceColor::Yellow);
        // remaining 5 (a quarter day's budget or less, "5% left") -> red.
        assert_eq!(wpc(75.0, 20.0, 5, resets, now), PaceColor::Red);
        // At/over the ceiling -> red.
        assert_eq!(wpc(80.0, 20.0, 5, resets, now), PaceColor::Red);
        assert_eq!(wpc(85.0, 20.0, 5, resets, now), PaceColor::Red);
    }

    #[test]
    fn session_thresholds() {
        assert_eq!(compute_session_color(50.0), PaceColor::Green);
        assert_eq!(compute_session_color(51.0), PaceColor::Yellow);
        assert_eq!(compute_session_color(80.0), PaceColor::Yellow);
        assert_eq!(compute_session_color(81.0), PaceColor::Red);
    }

    #[test]
    fn pool_color_projection_dominates() {
        // Plenty left, but projected to deplete before reset -> Red.
        assert_eq!(compute_pool_color(20.0, true), PaceColor::Red);
        // Not depleting, low usage -> Green.
        assert_eq!(compute_pool_color(20.0, false), PaceColor::Green);
        // 88% used, not depleting -> Yellow.
        assert_eq!(compute_pool_color(88.0, false), PaceColor::Yellow);
        // 90% used -> Red regardless.
        assert_eq!(compute_pool_color(90.0, false), PaceColor::Red);
    }

    #[test]
    fn weekday_counting() {
        // Mon after a Thu-reset cycle: Thu+Fri+Sat+Sun+Mon -> 3 weekdays.
        let now = utc(2024, 3, 11, 12, 0);
        let resets = utc(2024, 3, 14, 9, 0);
        assert_eq!(wde(resets, now, 5), 3);
    }

    #[test]
    fn weekend_uses_last_work_day_ceiling() {
        let now = utc(2024, 3, 10, 12, 0); // Sunday
        let resets = utc(2024, 3, 14, 9, 0);
        assert_eq!(wde(resets, now, 5), 2);
    }

    #[test]
    fn work_days_7_counts_all_calendar_days() {
        let now = utc(2024, 3, 11, 12, 0);
        let resets = utc(2024, 3, 14, 9, 0);
        assert_eq!(wde(resets, now, 7), 5);
    }

    /// A reset Monday 8pm: the cycle's work days are Tue, Wed, Thu, Fri, and the *next* Monday
    /// (its 9–5 is before the next 8pm reset). The Monday-daytime work at the cycle start belongs
    /// to the previous cycle. So Friday is day 4, and the following Monday is day 5.
    #[test]
    fn friday_with_monday_8pm_reset_is_day_4() {
        let cst = FixedOffset::west_opt(6 * 3600).unwrap();
        // Reset Mon Jun 15 2026 8pm CST (= Jun 16 02:00 UTC); cycle started Mon Jun 8 8pm CST.
        let resets = utc(2026, 6, 16, 2, 0);
        // Fri Jun 12, 4pm CST -> Tue, Wed, Thu, Fri elapsed.
        assert_eq!(work_days_elapsed(resets, utc(2026, 6, 12, 22, 0), 5, &cst), 4);
        // Next Mon Jun 15, noon CST (before the 8pm reset) -> the 5th work day.
        assert_eq!(work_days_elapsed(resets, utc(2026, 6, 15, 18, 0), 5, &cst), 5);
        // Sat/Sun don't advance it (still 4).
        assert_eq!(work_days_elapsed(resets, utc(2026, 6, 13, 22, 0), 5, &cst), 4);
        assert_eq!(work_days_elapsed(resets, utc(2026, 6, 14, 22, 0), 5, &cst), 4);
    }
}
