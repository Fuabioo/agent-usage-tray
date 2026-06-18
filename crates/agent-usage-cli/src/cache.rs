//! A tiny on-disk cache of the last successful JSON snapshot per agent.
//!
//! Two jobs: (1) dedupe rapid repeated calls within a short TTL so we stop hammering an agent's
//! usage API (e.g. the app re-polling on every relaunch), and (2) serve the last good snapshot
//! when a fetch fails transiently (rate limit, network blip) instead of an error — the
//! stale-is-better-than-nothing behavior the original tool's design called for.
//!
//! Cache files live at `$XDG_CACHE_HOME/agent-usage/<id>.json` (or `~/.cache/...`), keyed by
//! agent id; the file's mtime is its age. All operations are best-effort and never fail the run.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use agent_usage_core::cache_dir;

fn cache_path(id: &str) -> Option<PathBuf> {
    cache_dir().map(|d| d.join(format!("{id}.json")))
}

/// The cached snapshot JSON for `id` and its age, if a cache file exists and is readable.
pub fn read(id: &str) -> Option<(Duration, String)> {
    let path = cache_path(id)?;
    let modified = fs::metadata(&path).ok()?.modified().ok()?;
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::ZERO);
    let contents = fs::read_to_string(&path).ok()?;
    Some((age, contents))
}

/// Write `contents` as the cached snapshot for `id` (best-effort; errors are ignored).
pub fn write(id: &str, contents: &str) {
    let Some(path) = cache_path(id) else { return };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(path, contents);
}
