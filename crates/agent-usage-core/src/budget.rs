//! Shared pace-budget settings.
//!
//! These two numbers parameterize the weekly pace algorithm and are intentionally the same
//! defaults every frontend uses (the CLI, the macOS app, the COSMIC applet) so coloring is
//! consistent no matter where you look. The prototype's "Work days" day-picker is just a
//! friendlier way to choose `work_days` (and thus `daily_budget = 100 / work_days`).

pub const DEFAULT_DAILY_BUDGET: f64 = 20.0;
pub const DEFAULT_WORK_DAYS: u8 = 5;

/// Pace budget: expected percent consumed per work day, across N work days per cycle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Budget {
    pub daily_budget: f64,
    pub work_days: u8,
}

impl Default for Budget {
    fn default() -> Self {
        Budget {
            daily_budget: DEFAULT_DAILY_BUDGET,
            work_days: DEFAULT_WORK_DAYS,
        }
    }
}

impl Budget {
    /// Clamp `work_days` into the valid `1..=7` range the pace algorithm expects.
    pub fn validated(mut self) -> Self {
        self.work_days = self.work_days.clamp(1, 7);
        self
    }
}
