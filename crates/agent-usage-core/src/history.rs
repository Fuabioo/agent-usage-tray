//! Rolling usage history: burn rate, projected exhaustion, and the short-horizon "burst" brake.
//!
//! A usage API reports a single number — "31% of the week is gone" — with no sense of *how fast*
//! it got there. That is fine while an agent also exposes a short rolling window: burning through
//! a 5-hour limit stops you long before the weekly one is at risk. When an agent drops that short
//! window and bills a single multi-day quota, nothing is left to notice a sitting that eats the
//! whole cycle in an afternoon.
//!
//! This module reconstructs the missing signal from the only thing available: successive readings
//! over time. Sampling `used_pct` on every live fetch yields three derived numbers —
//!
//! - **burn rate** — percent of the cycle consumed per day, over the trailing [`BURN_LOOKBACK`],
//! - **projected exhaustion** — when the cycle hits 100% at that rate, and whether that lands
//!   before it would have reset,
//! - **burst** — percent consumed inside the trailing [`BURST_WINDOW`], which is the replacement
//!   brake: it answers "how hot am I running *right now*" rather than "how much is left".
//!
//! All of it is pure arithmetic over a sample series; persistence is the caller's business.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// How long samples are kept — a full weekly cycle plus slack, so a cycle's burn rate survives
/// right up to its reset.
pub const RETENTION_SECS: i64 = 8 * 86_400;
/// Trailing span the burn rate is measured over. A day smooths out single bursts while still
/// reacting within hours.
pub const BURN_LOOKBACK_SECS: i64 = 86_400;
/// The short-horizon window that stands in for a fixed session window the agent no longer
/// exposes. Five hours to match the window Codex retired.
pub const BURST_WINDOW_SECS: i64 = 5 * 3_600;

/// Shortest history a burn rate may be extrapolated from. Below a couple of hours a brief flurry
/// projects an absurd daily rate, so we report nothing rather than something alarming and wrong.
const MIN_BURN_SPAN_SECS: i64 = 2 * 3_600;
/// Shortest history a burst is computed over. The burst is deliberately twitchier than the burn
/// rate — that is its job — but under half an hour it is mostly polling jitter.
const MIN_BURST_SPAN_SECS: i64 = 30 * 60;
/// A reading this far below its predecessor means the window reset and a new cycle began.
/// Utilization never falls within a cycle, so any real drop is a rollover.
const RESET_DROP_PCT: f64 = 1.0;

/// One observation of a window's utilization at a point in time.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub at: DateTime<Utc>,
    pub used_pct: f64,
}

/// Percent of the cycle consumed inside a trailing window, and the span actually covered by
/// samples — which is shorter than the nominal window until enough history accumulates, so a
/// young series reads honestly ("8% in the last 2h") instead of pretending to a full window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Burst {
    pub used_pct: f64,
    pub span: Duration,
}

/// What a sample series says about the pace of consumption.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trend {
    /// Percent of the cycle consumed per day at the recently observed rate.
    pub burn_per_day: Option<f64>,
    /// Span the burn rate was measured over, for callers that want to qualify it.
    pub measured_over: Option<Duration>,
    /// When the cycle reaches 100% at that rate.
    pub exhausts_at: Option<DateTime<Utc>>,
    /// True when exhaustion is projected to land before the cycle would have reset — the
    /// condition worth warning about.
    pub exhausts_before_reset: bool,
    /// Consumption inside the trailing [`BURST_WINDOW_SECS`].
    pub recent: Option<Burst>,
}

/// A time-ordered series of utilization readings for one window of one agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct History {
    #[serde(default)]
    pub samples: Vec<Sample>,
}

impl History {
    /// Append a live reading, dropping anything that can no longer be compared against it.
    ///
    /// Two series-level invariants are maintained here so every reader can assume them: samples
    /// are strictly increasing in time, and they all belong to the *current* cycle. A reading
    /// below its predecessor means the window rolled over, and deltas spanning that boundary are
    /// meaningless (they would read as negative consumption), so the series restarts.
    pub fn record(&mut self, at: DateTime<Utc>, used_pct: f64) {
        if let Some(last) = self.samples.last() {
            if used_pct + RESET_DROP_PCT < last.used_pct {
                self.samples.clear();
            } else if at <= last.at {
                // A duplicate or out-of-order poll (clock adjustment, concurrent frontends).
                return;
            }
        }

        // Collapse a run of identical readings to its endpoints: while usage is flat, the pair
        // (run start, run end) carries everything a delta needs, and idle polling every few
        // minutes for a week would otherwise grow the file without adding information.
        let len = self.samples.len();
        if len >= 2
            && self.samples[len - 1].used_pct == used_pct
            && self.samples[len - 2].used_pct == used_pct
        {
            self.samples[len - 1].at = at;
        } else {
            self.samples.push(Sample { at, used_pct });
        }

        let cutoff = at - Duration::seconds(RETENTION_SECS);
        self.samples.retain(|s| s.at >= cutoff);
    }

    /// Consumption over the trailing `span`, plus the span the samples actually cover.
    ///
    /// The anchor is the reading as of `now - span` when the series reaches that far back, and
    /// the oldest reading otherwise — so a young series still reports, over its real (shorter)
    /// span. `None` until that span reaches `min_span`.
    ///
    /// The span is measured from `now - span` rather than from the anchor's own timestamp
    /// whenever the anchor predates the window: the anchor's *value* is what utilization stood at
    /// when the window opened, so the whole delta accrued inside the window. Dividing by the
    /// anchor's age instead would smear a burst across however long ago the last poll happened
    /// to be — a series with a day-old reading would report a 24-hour rate over 30 hours.
    fn delta_over(
        &self,
        now: DateTime<Utc>,
        span: Duration,
        min_span: Duration,
    ) -> Option<(f64, Duration)> {
        let last = self.samples.last()?;
        let from = now - span;
        let anchor = self
            .samples
            .iter()
            .rev()
            .find(|s| s.at <= from)
            .or_else(|| self.samples.first())?;

        // A series that stopped updating before the window opened covers none of it, and falls
        // out here rather than reporting a rate from data that predates what it claims to measure.
        let covered = last.at - anchor.at.max(from);
        if covered < min_span {
            return None;
        }
        // Clamped at zero: a mid-cycle rollover is handled in `record`, so a negative delta here
        // could only come from a corrupt file, and a negative burn rate is never meaningful.
        Some(((last.used_pct - anchor.used_pct).max(0.0), covered))
    }

    /// Percent of the cycle consumed per day, over the trailing [`BURN_LOOKBACK_SECS`].
    pub fn burn_per_day(&self, now: DateTime<Utc>) -> Option<(f64, Duration)> {
        let (delta, covered) = self.delta_over(
            now,
            Duration::seconds(BURN_LOOKBACK_SECS),
            Duration::seconds(MIN_BURN_SPAN_SECS),
        )?;
        let hours = covered.num_seconds() as f64 / 3_600.0;
        if hours <= 0.0 {
            return None;
        }
        Some((delta / hours * 24.0, covered))
    }

    /// Percent of the cycle consumed inside the trailing [`BURST_WINDOW_SECS`].
    pub fn burst(&self, now: DateTime<Utc>) -> Option<Burst> {
        let (used_pct, span) = self.delta_over(
            now,
            Duration::seconds(BURST_WINDOW_SECS),
            Duration::seconds(MIN_BURST_SPAN_SECS),
        )?;
        Some(Burst { used_pct, span })
    }

    /// Everything the series can say about the pace of a window currently at `used_pct` and
    /// resetting at `resets_at`. `None` when the history is too young for either signal.
    pub fn trend(
        &self,
        now: DateTime<Utc>,
        used_pct: f64,
        resets_at: Option<DateTime<Utc>>,
    ) -> Option<Trend> {
        let burn = self.burn_per_day(now);
        let recent = self.burst(now);
        if burn.is_none() && recent.is_none() {
            return None;
        }

        let exhausts_at = burn.and_then(|(rate, _)| {
            if rate <= 0.0 {
                return None;
            }
            let days_left = (100.0 - used_pct).max(0.0) / rate;
            Some(now + Duration::seconds((days_left * 86_400.0).round() as i64))
        });

        Some(Trend {
            burn_per_day: burn.map(|(rate, _)| rate),
            measured_over: burn.map(|(_, covered)| covered),
            exhausts_at,
            // Without a reset time there is nothing to beat, so exhaustion is not "early".
            exhausts_before_reset: match (exhausts_at, resets_at) {
                (Some(exhausts), Some(reset)) => exhausts < reset,
                _ => false,
            },
            recent,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    /// A series climbing `step` percent every `every_mins`, ending at `now`.
    fn series(now: DateTime<Utc>, count: i64, every_mins: i64, step: f64) -> History {
        let mut h = History::default();
        for i in 0..count {
            let at = now - Duration::minutes(every_mins * (count - 1 - i));
            h.record(at, step * i as f64);
        }
        h
    }

    #[test]
    fn burn_rate_extrapolates_to_a_day() {
        let now = utc(2026, 7, 27, 18, 0);
        // 5 samples an hour apart, +3% each -> 12% over 4h -> 72%/day.
        let h = series(now, 5, 60, 3.0);
        let (rate, covered) = h.burn_per_day(now).unwrap();
        assert!((rate - 72.0).abs() < 0.001);
        assert_eq!(covered, Duration::hours(4));
    }

    #[test]
    fn burn_rate_needs_a_couple_of_hours_of_history() {
        let now = utc(2026, 7, 27, 18, 0);
        // Only 1h of history: a brief flurry must not project a daily rate.
        assert!(series(now, 3, 30, 4.0).burn_per_day(now).is_none());
    }

    #[test]
    fn burst_reports_a_short_partial_span_honestly() {
        let now = utc(2026, 7, 27, 18, 0);
        // 45 minutes of history, +8% total — under the 5h window but over the burst minimum.
        let h = series(now, 4, 15, 2.0);
        let b = h.burst(now).unwrap();
        assert!((b.used_pct - 6.0).abs() < 0.001);
        assert_eq!(b.span, Duration::minutes(45));
        // Too young for a burn rate, but the burst alone is still a usable signal.
        assert!(h.burn_per_day(now).is_none());
        assert!(h.trend(now, 6.0, None).is_some());
    }

    #[test]
    fn burst_ignores_consumption_older_than_its_window() {
        let now = utc(2026, 7, 27, 18, 0);
        let mut h = History::default();
        h.record(now - Duration::hours(20), 0.0);
        h.record(now - Duration::hours(19), 40.0); // a big sitting, long ago
        h.record(now - Duration::hours(3), 40.0);
        h.record(now, 42.0); // only 2% in the burst window
        let b = h.burst(now).unwrap();
        assert!((b.used_pct - 2.0).abs() < 0.001, "got {}", b.used_pct);
        assert_eq!(b.span, Duration::seconds(BURST_WINDOW_SECS));
    }

    /// An anchor older than the window bounds the *value*, not the span — otherwise a burst gets
    /// smeared over however long ago the previous poll happened to land, understating the rate.
    #[test]
    fn a_rate_is_measured_over_its_window_not_the_anchors_age() {
        let now = utc(2026, 7, 27, 18, 0);
        let mut h = History::default();
        h.record(now - Duration::hours(30), 9.0); // last reading before the lookback opened
        h.record(now - Duration::hours(6), 9.0);
        h.record(now, 33.0); // 24% consumed, all of it inside the last day
        let (rate, covered) = h.burn_per_day(now).unwrap();
        assert_eq!(covered, Duration::hours(24));
        assert!((rate - 24.0).abs() < 0.001, "got {rate}");
    }

    #[test]
    fn a_series_that_stopped_before_the_window_reports_nothing() {
        let now = utc(2026, 7, 27, 18, 0);
        let mut h = History::default();
        h.record(now - Duration::days(4), 10.0);
        h.record(now - Duration::days(3), 40.0);
        // Nothing polled for three days: there is no rate to claim for the last 24 hours.
        assert!(h.burn_per_day(now).is_none());
        assert!(h.burst(now).is_none());
    }

    #[test]
    fn projects_exhaustion_and_flags_an_early_one() {
        let now = utc(2026, 7, 27, 18, 0);
        let h = series(now, 5, 60, 3.0); // 72%/day
                                         // At 12% used, the remaining 88% lasts ~1.22 days; the cycle resets in 5.
        let t = h.trend(now, 12.0, Some(now + Duration::days(5))).unwrap();
        let exhausts = t.exhausts_at.unwrap();
        assert!(exhausts > now + Duration::hours(29));
        assert!(exhausts < now + Duration::hours(30));
        assert!(t.exhausts_before_reset);
    }

    #[test]
    fn a_rate_that_lasts_until_reset_is_not_flagged() {
        let now = utc(2026, 7, 27, 18, 0);
        let h = series(now, 5, 60, 0.5); // 12%/day
                                         // 40% left at 12%/day is ~3.3 days of runway; the cycle resets in 12 hours.
        let t = h.trend(now, 60.0, Some(now + Duration::hours(12))).unwrap();
        assert!(!t.exhausts_before_reset);
    }

    #[test]
    fn idle_history_projects_no_exhaustion() {
        let now = utc(2026, 7, 27, 18, 0);
        let h = series(now, 5, 60, 0.0); // flat: not burning at all
        let t = h.trend(now, 30.0, Some(now + Duration::days(3))).unwrap();
        assert_eq!(t.burn_per_day, Some(0.0));
        assert!(t.exhausts_at.is_none());
        assert!(!t.exhausts_before_reset);
    }

    #[test]
    fn a_window_reset_starts_a_fresh_series() {
        let now = utc(2026, 7, 27, 18, 0);
        let mut h = series(now - Duration::hours(1), 5, 60, 18.0); // climbs to 72%
        h.record(now, 2.0); // the cycle rolled over
        assert_eq!(h.samples.len(), 1);
        // The pre-reset climb must not leak into the new cycle's rate.
        assert!(h.burn_per_day(now).is_none());
    }

    #[test]
    fn flat_readings_collapse_instead_of_accumulating() {
        let now = utc(2026, 7, 27, 18, 0);
        let mut h = History::default();
        for i in 0..50 {
            h.record(now - Duration::minutes(50 - i), 25.0);
        }
        // A run of identical readings keeps only its endpoints.
        assert_eq!(h.samples.len(), 2);
        assert_eq!(h.samples[0].at, now - Duration::minutes(50));
        assert_eq!(h.samples[1].at, now - Duration::minutes(1));
    }

    #[test]
    fn out_of_order_and_duplicate_polls_are_dropped() {
        let now = utc(2026, 7, 27, 18, 0);
        let mut h = History::default();
        h.record(now - Duration::hours(1), 10.0);
        h.record(now, 12.0);
        h.record(now - Duration::minutes(30), 11.0); // late arrival
        h.record(now, 12.0); // duplicate timestamp
        assert_eq!(h.samples.len(), 2);
        assert_eq!(h.samples.last().unwrap().used_pct, 12.0);
    }

    #[test]
    fn samples_beyond_retention_are_pruned() {
        let now = utc(2026, 7, 27, 18, 0);
        let mut h = History::default();
        h.record(now - Duration::days(9), 1.0);
        h.record(now - Duration::days(2), 2.0);
        h.record(now, 3.0);
        assert_eq!(h.samples.len(), 2);
        assert!(h.samples.iter().all(|s| s.at >= now - Duration::days(8)));
    }

    #[test]
    fn empty_history_has_no_trend() {
        let now = utc(2026, 7, 27, 18, 0);
        assert!(History::default().trend(now, 0.0, None).is_none());
    }
}
