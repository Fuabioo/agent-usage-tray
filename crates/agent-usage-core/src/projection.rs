//! Burn-rate projection for consumable credit pools.
//!
//! The prototype's alert — "Hyper — hypercredits out ~Thu at this rate · 610 of 5,000 left ·
//! burning ≈310/day" — is exactly this: given a remaining balance and an observed daily burn
//! rate, when does the pool hit zero, and is that before it would otherwise refill? This module
//! answers both questions purely from numbers so any frontend can render the same alert.

use crate::schema::{Metric, Window};
use chrono::{DateTime, Duration, Utc};

/// The outcome of projecting a pool forward at its current burn rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projection {
    /// When the balance is projected to reach zero, if a positive burn rate is known.
    pub depletes_at: Option<DateTime<Utc>>,
    /// True when depletion is projected to land before the window's reset/refill time.
    pub depletes_before_reset: bool,
}

/// Projects a window forward. Returns `None` for windows that aren't consumable pools or that
/// lack a usable (positive) burn rate — there is nothing to project in those cases.
pub fn project(window: &Window, now: DateTime<Utc>) -> Option<Projection> {
    let (remaining, burn_per_day) = match window.metric {
        Metric::Pool {
            remaining,
            burn_per_day: Some(burn),
            ..
        } => (remaining, burn),
        _ => return None,
    };

    if burn_per_day <= 0.0 {
        // Not burning (or refilling) -> never depletes at this rate.
        return Some(Projection {
            depletes_at: None,
            depletes_before_reset: false,
        });
    }

    let days_left = (remaining.max(0.0)) / burn_per_day;
    // Convert fractional days to whole seconds to keep the timestamp exact-ish.
    let secs = (days_left * 86_400.0).round() as i64;
    let depletes_at = now + Duration::seconds(secs);

    let depletes_before_reset = match window.resets_at {
        Some(reset) => depletes_at < reset,
        // No refill scheduled (e.g. "no auto-refill"): running out is always meaningful.
        None => true,
    };

    Some(Projection {
        depletes_at: Some(depletes_at),
        depletes_before_reset,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Window;
    use chrono::TimeZone;

    fn utc(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap()
    }

    #[test]
    fn hyper_scenario_depletes_in_about_two_days() {
        // 610 left, burning 310/day, no refill -> ~1.97 days from now.
        let now = utc(2026, 6, 10, 11); // Wed
        let w = Window::pool("hypercredits", 5000.0, 610.0, Some(310.0), None);
        let p = project(&w, now).unwrap();
        let depletes = p.depletes_at.unwrap();
        // ~1.97 days later lands on Friday; "out ~Thu/Fri" range.
        assert!(depletes > now + Duration::days(1));
        assert!(depletes < now + Duration::days(2) + Duration::hours(1));
        assert!(p.depletes_before_reset, "no refill -> always meaningful");
    }

    #[test]
    fn depletes_before_a_far_reset() {
        let now = utc(2026, 6, 10, 11);
        let reset = utc(2026, 6, 20, 0); // 10 days out
        let w = Window::pool("credits", 1000.0, 100.0, Some(200.0), Some(reset));
        let p = project(&w, now).unwrap();
        assert!(p.depletes_before_reset, "0.5 days < 10 days");
    }

    #[test]
    fn survives_until_reset() {
        let now = utc(2026, 6, 10, 11);
        let reset = utc(2026, 6, 11, 0); // ~13h out
        let w = Window::pool("credits", 1000.0, 900.0, Some(100.0), Some(reset));
        let p = project(&w, now).unwrap();
        // 9 days of runway, resets in 13h -> survives.
        assert!(!p.depletes_before_reset);
    }

    #[test]
    fn no_burn_rate_is_not_projectable() {
        let now = utc(2026, 6, 10, 11);
        let w = Window::pool("credits", 1000.0, 900.0, None, None);
        assert!(project(&w, now).is_none());
    }

    #[test]
    fn utilization_window_is_not_projectable() {
        let now = utc(2026, 6, 10, 11);
        let w = Window::utilization(crate::schema::WindowKind::Weekly, "weekly", 50.0, None);
        assert!(project(&w, now).is_none());
    }
}
