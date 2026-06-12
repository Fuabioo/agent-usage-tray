//! The stable output contract — identical shape for every agent.
//!
//! A frontend (or a human) gets the same JSON document whether it asks about Claude, Codex, or
//! any future agent: an `agent` block, the pace `config`, a flat `windows` array, an optional
//! weekly `pace` summary, and an `error` that is null on success. Per-window color and
//! credit-pool projection are computed here so consumers never re-derive them.

use agent_usage_core::{
    self as core, AgentInfo, Budget, Metric, PaceColor, Usage, UsageError, Window, WindowKind,
};
use chrono::{DateTime, Local, Utc};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub agent: AgentDto,
    pub fetched_at: DateTime<Utc>,
    pub config: ConfigDto,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<WindowDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pace: Option<PaceSummaryDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorDto>,
}

#[derive(Debug, Serialize)]
pub struct AgentDto {
    pub id: String,
    pub label: String,
    pub source: String,
}

#[derive(Debug, Serialize)]
pub struct ConfigDto {
    pub daily_budget: f64,
    pub work_days: u8,
}

#[derive(Debug, Serialize)]
pub struct WindowDto {
    pub kind: &'static str,
    pub label: String,
    pub used_pct: f64,
    pub remaining_pct: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_in_secs: Option<i64>,
    pub pace: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool: Option<PoolDto>,
}

#[derive(Debug, Serialize)]
pub struct PoolDto {
    pub total: f64,
    pub remaining: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub burn_per_day: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projected_depletion: Option<DateTime<Utc>>,
    pub depletes_before_reset: bool,
}

/// Weekly pace context, surfaced once per snapshot when a weekly window is present.
#[derive(Debug, Serialize)]
pub struct PaceSummaryDto {
    pub work_day_index: u8,
    pub daily_ceiling: f64,
    /// `ceiling - weekly used`. Negative means over today's budget.
    pub remaining: f64,
    pub reset_day_local: String,
}

#[derive(Debug, Serialize)]
pub struct ErrorDto {
    pub kind: String,
    pub message: String,
}

impl AgentDto {
    pub fn from_info(info: &AgentInfo) -> Self {
        AgentDto {
            id: info.id.clone(),
            label: info.label.clone(),
            source: info.source.clone(),
        }
    }
}

impl ConfigDto {
    fn from_budget(b: &Budget) -> Self {
        ConfigDto {
            daily_budget: b.daily_budget,
            work_days: b.work_days,
        }
    }
}

/// Color one window using the rule appropriate to its kind, plus any projection.
fn window_color(window: &Window, budget: &Budget, now: DateTime<Utc>) -> PaceColor {
    match window.kind {
        WindowKind::Weekly => match window.resets_at {
            Some(resets_at) => core::compute_weekly_pace_color(
                window.used_pct(),
                budget.daily_budget,
                budget.work_days,
                resets_at,
                now,
            ),
            None => core::default_window_color(window),
        },
        WindowKind::Session => core::compute_session_color(window.used_pct()),
        WindowKind::Credits => {
            let depletes = core::project(window, now)
                .map(|p| p.depletes_before_reset)
                .unwrap_or(false);
            core::compute_pool_color(window.used_pct(), depletes)
        }
        WindowKind::Other => core::default_window_color(window),
    }
}

fn build_window(window: &Window, budget: &Budget, now: DateTime<Utc>) -> WindowDto {
    let resets_in_secs = window
        .resets_at
        .map(|r| r.signed_duration_since(now).num_seconds().max(0));

    let pool = match window.metric {
        Metric::Pool {
            total,
            remaining,
            burn_per_day,
        } => {
            let proj = core::project(window, now);
            Some(PoolDto {
                total,
                remaining,
                burn_per_day,
                projected_depletion: proj.and_then(|p| p.depletes_at),
                depletes_before_reset: proj.map(|p| p.depletes_before_reset).unwrap_or(false),
            })
        }
        Metric::Utilization { .. } => None,
    };

    WindowDto {
        kind: window.kind.tag(),
        label: window.label.clone(),
        used_pct: window.used_pct(),
        remaining_pct: window.remaining_pct(),
        resets_at: window.resets_at,
        resets_in_secs,
        pace: window_color(window, budget, now).tag(),
        pool,
    }
}

/// Build a success snapshot from normalized usage plus pace config.
pub fn build_snapshot(usage: &Usage, budget: &Budget, now: DateTime<Utc>) -> Snapshot {
    let windows: Vec<WindowDto> = usage
        .windows
        .iter()
        .map(|w| build_window(w, budget, now))
        .collect();

    // Derive the weekly pace summary from the first weekly window, if any.
    let pace = usage
        .windows
        .iter()
        .find(|w| w.kind == WindowKind::Weekly)
        .and_then(|w| w.resets_at.map(|r| (w, r)))
        .map(|(w, resets_at)| {
            let idx = core::days_into_cycle(resets_at, now, budget.work_days);
            let ceiling = idx as f64 * budget.daily_budget;
            PaceSummaryDto {
                work_day_index: idx,
                daily_ceiling: ceiling,
                remaining: ceiling - w.used_pct(),
                reset_day_local: core::reset_day_name(resets_at),
            }
        });

    Snapshot {
        agent: AgentDto::from_info(&usage.agent),
        fetched_at: now,
        config: ConfigDto::from_budget(budget),
        windows,
        pace,
        error: None,
    }
}

/// Build an error snapshot. Agent identity is still populated so callers can label the failure.
pub fn build_error_snapshot(
    agent: &AgentInfo,
    err: &UsageError,
    budget: &Budget,
    now: DateTime<Utc>,
) -> Snapshot {
    Snapshot {
        agent: AgentDto::from_info(agent),
        fetched_at: now,
        config: ConfigDto::from_budget(budget),
        windows: Vec::new(),
        pace: None,
        error: Some(ErrorDto {
            kind: err.kind().to_string(),
            message: err.to_string(),
        }),
    }
}

/// Render the human-readable `--status` view for one snapshot.
pub fn render_status(snap: &Snapshot, now: DateTime<Utc>) -> String {
    let mut out = String::new();
    out.push_str(&format!("=== {} ({}) ===\n\n", snap.agent.label, snap.agent.source));

    if let Some(err) = &snap.error {
        out.push_str(&format!("error [{}]: {}\n", err.kind, err.message));
        return out;
    }

    out.push_str(&format!("now (local)   = {}\n", now.with_timezone(&Local)));
    out.push_str("[Config]\n");
    out.push_str(&format!(
        "  daily_budget = {}% per work day\n",
        snap.config.daily_budget
    ));
    out.push_str(&format!("  work_days    = {}\n", snap.config.work_days));

    if let Some(p) = &snap.pace {
        out.push_str(&format!("  pace ceiling = {} work days x {}% = {:.1}%\n", p.work_day_index, snap.config.daily_budget, p.daily_ceiling));
        out.push_str(&format!("  resets       = {}\n", p.reset_day_local));
    }
    out.push('\n');

    for w in &snap.windows {
        out.push_str(&format!("[{} · {}]\n", w.kind, w.label));
        out.push_str(&format!("  used        = {:.1}%  ({:.1}% left)\n", w.used_pct, w.remaining_pct));
        out.push_str(&format!("  pace        = {}\n", w.pace));
        if let Some(secs) = w.resets_in_secs {
            let dur = chrono::Duration::seconds(secs);
            out.push_str(&format!("  resets_in   = {}\n", core::format_duration(dur)));
        }
        if let Some(pool) = &w.pool {
            out.push_str(&format!(
                "  pool        = {:.0} of {:.0} left\n",
                pool.remaining, pool.total
            ));
            if let Some(burn) = pool.burn_per_day {
                out.push_str(&format!("  burn        = {:.0}/day\n", burn));
            }
            if let Some(dep) = pool.projected_depletion {
                out.push_str(&format!(
                    "  projected   = out {} ({})\n",
                    core::reset_day_name(dep),
                    if pool.depletes_before_reset {
                        "before reset"
                    } else {
                        "after reset"
                    }
                ));
            }
        }
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_usage_core::WindowKind;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2024, 3, 11, 12, 0, 0).unwrap()
    }

    fn claude_usage() -> Usage {
        Usage {
            agent: AgentInfo {
                id: "claude".into(),
                label: "Claude Code".into(),
                source: "test".into(),
            },
            windows: vec![
                Window::utilization(
                    WindowKind::Weekly,
                    "weekly",
                    42.0,
                    Some(Utc.with_ymd_and_hms(2024, 3, 14, 9, 0, 0).unwrap()),
                ),
                Window::utilization(
                    WindowKind::Session,
                    "session",
                    15.0,
                    Some(Utc.with_ymd_and_hms(2024, 3, 11, 15, 0, 0).unwrap()),
                ),
            ],
        }
    }

    #[test]
    fn snapshot_has_windows_and_pace() {
        let snap = build_snapshot(&claude_usage(), &Budget::default(), now());
        assert_eq!(snap.windows.len(), 2);
        assert!(snap.pace.is_some());
        assert_eq!(snap.windows[0].kind, "weekly");
        // 42% used -> 58% left.
        assert!((snap.windows[0].remaining_pct - 58.0).abs() < 0.001);
        assert!(snap.error.is_none());
    }

    #[test]
    fn pool_window_emits_projection() {
        let usage = Usage {
            agent: AgentInfo {
                id: "hyper".into(),
                label: "Hyper".into(),
                source: "test".into(),
            },
            windows: vec![Window::pool("hypercredits", 5000.0, 610.0, Some(310.0), None)],
        };
        let snap = build_snapshot(&usage, &Budget::default(), now());
        let pool = snap.windows[0].pool.as_ref().unwrap();
        assert_eq!(pool.remaining, 610.0);
        assert!(pool.projected_depletion.is_some());
        assert!(pool.depletes_before_reset);
        // 87.8% used + depleting -> red.
        assert_eq!(snap.windows[0].pace, "red");
    }

    #[test]
    fn error_snapshot_keeps_agent_and_omits_windows() {
        let info = AgentInfo {
            id: "codex".into(),
            label: "Codex".into(),
            source: "local config".into(),
        };
        let snap = build_error_snapshot(
            &info,
            &UsageError::Unsupported("nope".into()),
            &Budget::default(),
            now(),
        );
        assert!(snap.windows.is_empty());
        assert_eq!(snap.error.as_ref().unwrap().kind, "unsupported");
        assert_eq!(snap.agent.id, "codex");
    }
}
