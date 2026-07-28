//! On-disk persistence for an agent's usage sample series.
//!
//! The burn rate and burst readouts in [`agent_usage_core::history`] need successive readings over
//! time, which a single CLI invocation cannot have — every run is a fresh process. So the series
//! lives next to the snapshot cache at `$XDG_CACHE_HOME/agent-usage/<id>.history.json` (or
//! `~/.cache/...`), keyed by the same id, and each live fetch appends one sample to it.
//!
//! Like the snapshot cache, every operation is best-effort: a missing, unreadable or corrupt file
//! yields an empty series, which simply means no trend is reported until samples accumulate again.

use std::fs;
use std::path::PathBuf;

use agent_usage_core::{cache_dir, History};

fn history_path(id: &str) -> Option<PathBuf> {
    cache_dir().map(|d| d.join(format!("{id}.history.json")))
}

/// The stored series for `id`, or an empty one when there is nothing usable on disk.
pub fn load(id: &str) -> History {
    history_path(id)
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist the series for `id` (best-effort; errors are ignored).
pub fn store(id: &str, history: &History) {
    let Some(path) = history_path(id) else { return };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(s) = serde_json::to_string(history) {
        let _ = fs::write(path, s);
    }
}
