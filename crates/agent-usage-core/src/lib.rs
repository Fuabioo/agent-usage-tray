//! Agent-agnostic usage core.
//!
//! This crate is the shared brain behind every frontend in the workspace (the `agent-usage`
//! CLI, the macOS menu bar app, and the COSMIC applet). It is pure logic: no GUI, no network,
//! no credential reading. It defines
//!
//! - the [`Provider`] trait every agent implements ([`provider`]),
//! - the normalized [`Usage`] / [`Window`] / [`Metric`] schema agents report into ([`schema`]),
//! - [`pace`]-based color coding and [`projection`] of credit pools, and
//! - the shared [`Budget`] settings and small [`time`] helpers.
//!
//! Concrete providers (Claude, Codex, …) live in the `agent-usage-providers` crate so that this
//! core stays dependency-light and trivially testable.

pub mod budget;
pub mod error;
pub mod pace;
pub mod projection;
pub mod provider;
pub mod schema;
pub mod time;

pub use budget::Budget;
pub use error::UsageError;
pub use pace::{
    compute_pool_color, compute_session_color, compute_weekly_pace_color, days_into_cycle,
    default_window_color, reset_day_name, PaceColor,
};
pub use projection::{project, Projection};
pub use provider::{FetchOptions, Provider};
pub use schema::{AgentInfo, Metric, Usage, Window, WindowKind};
pub use time::format_duration;
