//! The normalized usage model shared by every agent.
//!
//! Agents measure usage in different ways: Claude reports rolling **percent-utilization**
//! windows (a 5-hour session and a 7-day weekly window), while credit-based agents report a
//! **consumable pool** (a balance that burns down and may run out before it refills). Rather
//! than special-case each agent downstream, every provider normalizes its data into a flat
//! list of [`Window`]s carrying one of two [`Metric`] shapes. Downstream code (the CLI, the
//! menu bar, the COSMIC applet) only ever speaks this vocabulary.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// What an agent is and where its numbers came from. Filled in even on error so callers can
/// label a failing agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentInfo {
    /// Stable lowercase id, also the CLI subcommand (e.g. `claude`, `codex`).
    pub id: String,
    /// Human display name (e.g. "Claude Code").
    pub label: String,
    /// Human description of the source mechanism (e.g. "Anthropic OAuth usage API").
    pub source: String,
}

/// A normalized usage snapshot for a single agent: who it is plus every window it exposes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub agent: AgentInfo,
    pub windows: Vec<Window>,
}

/// The role a window plays. Drives how pace color is computed and how it is labeled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowKind {
    /// A short rolling window (Claude's 5-hour session, Codex's "5h limit").
    Session,
    /// A multi-day budget window (Claude's 7-day weekly window).
    Weekly,
    /// A consumable credit balance (burns down, may auto-refill or run dry).
    Credits,
    /// Anything that does not fit the above; the label carries the meaning.
    Other,
}

impl WindowKind {
    /// Lowercase tag used in the JSON contract.
    pub fn tag(self) -> &'static str {
        match self {
            WindowKind::Session => "session",
            WindowKind::Weekly => "weekly",
            WindowKind::Credits => "credits",
            WindowKind::Other => "other",
        }
    }
}

/// How a window measures consumption.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Metric {
    /// Percent of the window consumed (0.0–100.0+, can exceed 100 when over budget).
    Utilization { used_pct: f64 },
    /// A consumable pool: how much was granted, how much is left, and (if known) the
    /// observed burn rate per day used to project depletion.
    Pool {
        total: f64,
        remaining: f64,
        burn_per_day: Option<f64>,
        /// The recurring allowance consumption is measured against (e.g. a daily recharge).
        /// When set, `used_pct` divides the consumed amount by this rather than `total`, so the
        /// part of `total` beyond `budget` is surplus that only counts as used once the
        /// allowance is spent ("extra usage", which pushes `used_pct` past 100%). `None`
        /// measures against the full pool.
        budget: Option<f64>,
    },
}

/// A single usage window normalized across agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub kind: WindowKind,
    /// Display label as the agent names it ("session", "5h limit", "weekly", "credits").
    pub label: String,
    pub metric: Metric,
    /// When this window resets/refills, if the agent exposes it.
    pub resets_at: Option<DateTime<Utc>>,
}

impl Metric {
    /// Percent of the window consumed, regardless of how the agent measures it. For a pool,
    /// this is derived from the remaining balance — against the recurring `budget` if one is
    /// set (so consuming surplus beyond it reads as over 100%), otherwise against `total`.
    pub fn used_pct(&self) -> f64 {
        match *self {
            Metric::Utilization { used_pct } => used_pct,
            Metric::Pool {
                total,
                remaining,
                budget,
                ..
            } => {
                let basis = budget.unwrap_or(total);
                if basis <= 0.0 {
                    0.0
                } else {
                    ((total - remaining) / basis * 100.0).max(0.0)
                }
            }
        }
    }

    /// Percent of the window still available (`100 - used`, never negative).
    pub fn remaining_pct(&self) -> f64 {
        (100.0 - self.used_pct()).max(0.0)
    }
}

impl Window {
    pub fn used_pct(&self) -> f64 {
        self.metric.used_pct()
    }

    pub fn remaining_pct(&self) -> f64 {
        self.metric.remaining_pct()
    }

    /// Convenience constructor for a percent-utilization window.
    pub fn utilization(
        kind: WindowKind,
        label: impl Into<String>,
        used_pct: f64,
        resets_at: Option<DateTime<Utc>>,
    ) -> Self {
        Window {
            kind,
            label: label.into(),
            metric: Metric::Utilization { used_pct },
            resets_at,
        }
    }

    /// Convenience constructor for a consumable credit-pool window.
    pub fn pool(
        label: impl Into<String>,
        total: f64,
        remaining: f64,
        burn_per_day: Option<f64>,
        resets_at: Option<DateTime<Utc>>,
    ) -> Self {
        Window {
            kind: WindowKind::Credits,
            label: label.into(),
            metric: Metric::Pool {
                total,
                remaining,
                burn_per_day,
                budget: None,
            },
            resets_at,
        }
    }

    /// Measure this pool's percentage against a recurring allowance (e.g. a daily recharge)
    /// instead of the full `total`; the rest of `total` becomes surplus that only counts as
    /// used once the allowance is spent. No-op on non-pool windows.
    pub fn with_budget(mut self, budget: f64) -> Self {
        if let Metric::Pool { budget: b, .. } = &mut self.metric {
            *b = Some(budget);
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utilization_used_and_remaining() {
        let w = Window::utilization(WindowKind::Weekly, "weekly", 42.5, None);
        assert_eq!(w.used_pct(), 42.5);
        assert_eq!(w.remaining_pct(), 57.5);
    }

    #[test]
    fn pool_derives_used_pct_from_balance() {
        // 610 of 5000 left -> 87.8% used.
        let w = Window::pool("hypercredits", 5000.0, 610.0, Some(310.0), None);
        assert!((w.used_pct() - 87.8).abs() < 0.001);
        assert!((w.remaining_pct() - 12.2).abs() < 0.001);
    }

    #[test]
    fn pool_with_zero_total_is_safe() {
        let w = Window::pool("empty", 0.0, 0.0, None, None);
        assert_eq!(w.used_pct(), 0.0);
        assert_eq!(w.remaining_pct(), 100.0);
    }

    #[test]
    fn pool_budget_measures_against_allowance_not_total() {
        // 1656 total (250 daily grant + 1406 surplus), 1620 left -> 36 of the 250 daily spent.
        let w = Window::pool("credits", 1656.0, 1620.0, None, None).with_budget(250.0);
        assert!((w.used_pct() - 14.4).abs() < 0.001);
        assert!((w.remaining_pct() - 85.6).abs() < 0.001);
    }

    #[test]
    fn pool_budget_beyond_allowance_is_extra_usage() {
        // Spent 300 against a 250 daily grant -> 50 into the surplus: over 100% used, 0% left.
        let w = Window::pool("credits", 1656.0, 1356.0, None, None).with_budget(250.0);
        assert!(w.used_pct() > 100.0);
        assert_eq!(w.remaining_pct(), 0.0);
    }

    #[test]
    fn utilization_over_hundred_clamps_remaining_at_zero() {
        let w = Window::utilization(WindowKind::Session, "session", 120.0, None);
        assert_eq!(w.used_pct(), 120.0);
        assert_eq!(w.remaining_pct(), 0.0);
    }
}
