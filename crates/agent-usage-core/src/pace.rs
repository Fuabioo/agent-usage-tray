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
use chrono::{DateTime, Datelike, Utc, Weekday};

const CYCLE_LENGTH: i64 = 7;

/// Pace-based color indicator. Green = on track, Yellow = approaching the line, Red = over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaceColor {
    Green,
    Yellow,
    Red,
}

impl PaceColor {
    /// Lowercase tag for the JSON contract.
    pub fn tag(self) -> &'static str {
        match self {
            PaceColor::Green => "green",
            PaceColor::Yellow => "yellow",
            PaceColor::Red => "red",
        }
    }
}

/// Computes the pace color for a multi-day budget (weekly) window.
///
/// Given the current work-day index within the cycle, the ceiling is `index * daily_budget`.
/// `ratio = utilization / ceiling`; `< 0.75` Green, `< 1.0` Yellow, else Red.
pub fn compute_weekly_pace_color(
    utilization: f64,
    daily_budget: f64,
    work_days: u8,
    resets_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> PaceColor {
    let work_day_index = days_into_cycle(resets_at, now, work_days);
    let ceiling = work_day_index as f64 * daily_budget;

    if ceiling <= 0.0 {
        return PaceColor::Red; // defensive: avoid division by zero
    }

    let ratio = utilization / ceiling;
    if ratio < 0.75 {
        PaceColor::Green
    } else if ratio < 1.00 {
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

/// Counts work days elapsed up to the current (possibly incomplete) period.
///
/// For `work_days <= 5` only weekdays (Mon–Fri) count; for `work_days > 5` all calendar days
/// count. The cycle starts at `resets_at - 7 days`; the current partial day is included so
/// today's budget contributes to the ceiling. Result is clamped to `[1, work_days]`.
pub fn days_into_cycle(resets_at: DateTime<Utc>, now: DateTime<Utc>, work_days: u8) -> u8 {
    let cycle_start = resets_at - chrono::Duration::days(CYCLE_LENGTH);

    if now <= cycle_start {
        return 1;
    }

    let completed = (now - cycle_start).num_days().min(CYCLE_LENGTH) as u64;
    let total_periods = (completed + 1).min(CYCLE_LENGTH as u64);

    if work_days > 5 {
        return (total_periods as u8).clamp(1, work_days);
    }

    let mut weekday_count = 0u8;
    for i in 0..total_periods {
        let period_start = cycle_start + chrono::Duration::days(i as i64);
        match period_start.weekday() {
            Weekday::Sat | Weekday::Sun => {}
            _ => weekday_count += 1,
        }
    }

    weekday_count.clamp(1, work_days)
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
    use chrono::TimeZone;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    #[test]
    fn day1_zero_is_green() {
        let now = utc(2024, 3, 7, 12, 0);
        let resets = utc(2024, 3, 14, 9, 0);
        assert_eq!(
            compute_weekly_pace_color(0.0, 20.0, 5, resets, now),
            PaceColor::Green
        );
    }

    #[test]
    fn day1_over_ceiling_is_red() {
        let now = utc(2024, 3, 7, 12, 0);
        let resets = utc(2024, 3, 14, 9, 0);
        assert_eq!(
            compute_weekly_pace_color(22.0, 20.0, 5, resets, now),
            PaceColor::Red
        );
    }

    #[test]
    fn zero_budget_is_red() {
        let now = utc(2024, 3, 7, 12, 0);
        let resets = utc(2024, 3, 14, 9, 0);
        assert_eq!(
            compute_weekly_pace_color(10.0, 0.0, 5, resets, now),
            PaceColor::Red
        );
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
    fn weekday_counting_matches_legacy() {
        // Mon after a Thu-reset cycle: Thu+Fri+Sat+Sun+Mon -> 3 weekdays.
        let now = utc(2024, 3, 11, 12, 0);
        let resets = utc(2024, 3, 14, 9, 0);
        assert_eq!(days_into_cycle(resets, now, 5), 3);
    }

    #[test]
    fn weekend_uses_last_work_day_ceiling() {
        let now = utc(2024, 3, 10, 12, 0); // Sunday
        let resets = utc(2024, 3, 14, 9, 0);
        assert_eq!(days_into_cycle(resets, now, 5), 2);
    }

    #[test]
    fn work_days_7_counts_all_calendar_days() {
        let now = utc(2024, 3, 11, 12, 0);
        let resets = utc(2024, 3, 14, 9, 0);
        assert_eq!(days_into_cycle(resets, now, 7), 5);
    }
}
