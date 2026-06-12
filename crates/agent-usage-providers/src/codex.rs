//! Codex provider.
//!
//! Codex (the OpenAI Codex CLI) authenticates via ChatGPT and gets its rate-limit status back
//! from the API on every turn. It does not expose a usage endpoint or a cached rate-limit table;
//! instead it records each status into its **session rollout logs** as a `token_count` event:
//!
//! ```jsonc
//! { "timestamp": "...", "type": "event_msg",
//!   "payload": { "type": "token_count",
//!     "rate_limits": {
//!       "primary":   { "used_percent": 58.0, "window_minutes": 300,   "resets_at": 1781216909 },
//!       "secondary": { "used_percent":  9.0, "window_minutes": 10080, "resets_at": 1781803709 },
//!       "credits": null, "plan_type": "team" } } }
//! ```
//!
//! So "via local config" means: read the freshest `rate_limits` event from the newest rollout
//! under `~/.codex/sessions/`. `primary` is the rolling 5-hour window, `secondary` the weekly
//! one — which map straight onto our Session/Weekly windows. (Confirmed against the live files,
//! matching what the open-source CLI writes; there is no separate cache to read.)

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use agent_usage_core::{
    AgentInfo, FetchOptions, Provider, Usage, UsageError, Window, WindowKind,
};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

use crate::creds;

const DEFAULT_CODEX_HOME: &str = "~/.codex";
/// How many of the most-recently-modified rollout files to scan for a fresh rate-limit event.
const MAX_FILES_SCANNED: usize = 8;

// --- Rollout JSON (only the fields we read; unknown fields are ignored) ---

#[derive(Deserialize)]
struct RolloutLine {
    timestamp: Option<String>,
    payload: Option<Payload>,
}

#[derive(Deserialize)]
struct Payload {
    #[serde(rename = "type")]
    kind: Option<String>,
    rate_limits: Option<RateLimits>,
}

#[derive(Deserialize)]
struct RateLimits {
    primary: Option<RlWindow>,
    secondary: Option<RlWindow>,
}

#[derive(Deserialize)]
struct RlWindow {
    used_percent: f64,
    window_minutes: Option<i64>,
    resets_at: Option<i64>,
}

/// The freshest rate-limit event we found, with the timestamp used to rank it across files.
struct Snapshot {
    timestamp: String,
    rate_limits: RateLimits,
}

pub struct Codex;

impl Codex {
    pub const fn new() -> Self {
        Codex
    }
}

impl Default for Codex {
    fn default() -> Self {
        Codex::new()
    }
}

impl Provider for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn label(&self) -> &'static str {
        "Codex"
    }

    fn source(&self) -> &'static str {
        "Codex CLI session logs (~/.codex)"
    }

    fn fetch(&self, opts: &FetchOptions) -> Result<Usage, UsageError> {
        let sessions_dir = self.sessions_dir(opts);
        if !sessions_dir.is_dir() {
            return Err(UsageError::CredentialsRead(format!(
                "Codex sessions directory not found: {}",
                sessions_dir.display()
            )));
        }

        let snapshot = newest_rate_limits(&sessions_dir)?.ok_or_else(|| {
            UsageError::NoData(
                "no Codex rate-limit data recorded yet (run Codex at least once)".to_string(),
            )
        })?;

        // The rollout is a point-in-time snapshot from Codex's last run, so correct for windows
        // that have rolled over since (see `window_from`).
        let now = Utc::now();
        let mut windows = Vec::new();
        // primary = rolling short window (5h); secondary = weekly window.
        if let Some(w) = snapshot.rate_limits.primary {
            windows.push(window_from(WindowKind::Session, primary_label(&w), &w, now));
        }
        if let Some(w) = snapshot.rate_limits.secondary {
            windows.push(window_from(WindowKind::Weekly, "weekly".to_string(), &w, now));
        }

        if windows.is_empty() {
            return Err(UsageError::NoData(
                "Codex rate-limit event had no primary/secondary windows".to_string(),
            ));
        }

        Ok(Usage {
            agent: AgentInfo {
                id: self.id().to_string(),
                label: self.label().to_string(),
                source: self.source().to_string(),
            },
            windows,
        })
    }
}

impl Codex {
    /// The Codex sessions directory. An explicit `--creds-path` overrides the Codex home (it may
    /// point at the `~/.codex` directory itself or directly at a `sessions` directory).
    fn sessions_dir(&self, opts: &FetchOptions) -> PathBuf {
        let base = opts
            .creds_path
            .clone()
            .unwrap_or_else(|| creds::expand_tilde(DEFAULT_CODEX_HOME));
        if base.ends_with("sessions") {
            base
        } else {
            base.join("sessions")
        }
    }
}

fn window_from(kind: WindowKind, label: String, w: &RlWindow, now: DateTime<Utc>) -> Window {
    let mut used = w.used_percent;
    let mut resets_at = w
        .resets_at
        .and_then(|secs| DateTime::from_timestamp(secs, 0));

    // If the recorded window has already reset, the snapshot is stale: the window rolled over to
    // 0%, and since this is the newest rollout (Codex's last activity) nothing has been spent
    // since. Report 0% used and advance the reset to the next cycle so the countdown stays sane.
    if let Some(reset) = resets_at {
        if reset <= now {
            used = 0.0;
            if let Some(mins) = w.window_minutes.filter(|m| *m > 0) {
                let win = mins * 60;
                let elapsed = (now - reset).num_seconds();
                let cycles = elapsed / win + 1; // strictly past `now`
                resets_at = Some(reset + Duration::seconds(cycles * win));
            }
        }
    }

    Window::utilization(kind, label, used, resets_at)
}

/// Human label for the short window from its length: 300 min -> "5h limit".
fn primary_label(w: &RlWindow) -> String {
    match w.window_minutes {
        Some(m) if m > 0 && m % 60 == 0 => format!("{}h limit", m / 60),
        Some(m) if m > 0 => format!("{}m limit", m),
        _ => "session".to_string(),
    }
}

/// Scan the most-recently-modified rollout files and return the rate-limit event with the
/// latest timestamp (ISO-8601 sorts chronologically), or `None` if none record rate limits.
fn newest_rate_limits(sessions_dir: &Path) -> Result<Option<Snapshot>, UsageError> {
    let mut files = rollout_files(sessions_dir)?;
    // Newest first by modification time.
    files.sort_by_key(|(_, mtime)| std::cmp::Reverse(*mtime));

    let mut best: Option<Snapshot> = None;
    for (path, _) in files.into_iter().take(MAX_FILES_SCANNED) {
        if let Some(snap) = last_rate_limits_in_file(&path) {
            let better = match &best {
                Some(b) => snap.timestamp > b.timestamp,
                None => true,
            };
            if better {
                best = Some(snap);
            }
        }
    }
    Ok(best)
}

/// The last `token_count` rate-limit event in one rollout file (files are append-only and
/// chronological, so the last match is the newest in that file).
fn last_rate_limits_in_file(path: &Path) -> Option<Snapshot> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);

    let mut found: Option<Snapshot> = None;
    for line in reader.lines().map_while(Result::ok) {
        if line.is_empty() || !line.contains("rate_limits") {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<RolloutLine>(&line) else {
            continue;
        };
        let Some(payload) = parsed.payload else { continue };
        if payload.kind.as_deref() != Some("token_count") {
            continue;
        }
        let Some(rl) = payload.rate_limits else { continue };
        if rl.primary.is_none() && rl.secondary.is_none() {
            continue;
        }
        found = Some(Snapshot {
            timestamp: parsed.timestamp.unwrap_or_default(),
            rate_limits: rl,
        });
    }
    found
}

/// Recursively collect `rollout-*.jsonl` files under `dir` with their modification times.
fn rollout_files(dir: &Path) -> Result<Vec<(PathBuf, SystemTime)>, UsageError> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];

    while let Some(d) = stack.pop() {
        let entries = match fs::read_dir(&d) {
            Ok(e) => e,
            Err(_) => continue, // skip unreadable subdirs rather than failing the whole scan
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file()
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
            {
                let mtime = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                out.push((path, mtime));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn identity() {
        let c = Codex::new();
        assert_eq!(c.id(), "codex");
        assert_eq!(c.label(), "Codex");
    }

    #[test]
    fn primary_label_formats_window() {
        let w = RlWindow {
            used_percent: 1.0,
            window_minutes: Some(300),
            resets_at: None,
        };
        assert_eq!(primary_label(&w), "5h limit");
    }

    #[test]
    fn parses_rate_limits_from_rollout() {
        let dir = tempdir();
        // `creds_path` points at the Codex home; rollouts live under `<home>/sessions/...`.
        let day = dir.join("sessions/2026/06/11");
        fs::create_dir_all(&day).unwrap();
        let mut f = File::create(day.join("rollout-2026-06-11T12-00-00-abc.jsonl")).unwrap();
        // A noise line, then the real token_count event.
        writeln!(f, r#"{{"timestamp":"2026-06-11T12:00:00Z","type":"event_msg","payload":{{"type":"agent_message","message":"hi"}}}}"#).unwrap();
        // Far-future reset times so the windows are never treated as stale by this test.
        writeln!(f, r#"{{"timestamp":"2026-06-11T12:30:00Z","type":"event_msg","payload":{{"type":"token_count","info":{{}},"rate_limits":{{"primary":{{"used_percent":58.0,"window_minutes":300,"resets_at":4102444800}},"secondary":{{"used_percent":9.0,"window_minutes":10080,"resets_at":4102444800}},"credits":null,"plan_type":"team"}}}}}}"#).unwrap();

        let opts = FetchOptions {
            creds_path: Some(dir.clone()),
            ..Default::default()
        };
        let usage = Codex::new().fetch(&opts).expect("should parse usage");
        assert_eq!(usage.windows.len(), 2);

        let session = &usage.windows[0];
        assert_eq!(session.kind, WindowKind::Session);
        assert_eq!(session.label, "5h limit");
        assert_eq!(session.used_pct(), 58.0);
        assert!(session.resets_at.is_some());

        let weekly = &usage.windows[1];
        assert_eq!(weekly.kind, WindowKind::Weekly);
        assert_eq!(weekly.used_pct(), 9.0);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stale_window_reads_as_reset() {
        // A 5h window whose reset is long past: the snapshot has rolled over.
        let now = DateTime::from_timestamp(1_781_300_000, 0).unwrap(); // 2026
        let w = RlWindow {
            used_percent: 58.0,
            window_minutes: Some(300),
            resets_at: Some(1_000_000_000), // 2001, way in the past
        };
        let win = window_from(WindowKind::Session, "5h limit".to_string(), &w, now);
        assert_eq!(win.used_pct(), 0.0, "past-reset window reads as reset");
        let next = win.resets_at.expect("reset advanced");
        assert!(next > now, "reset advanced into the future");
        // And it lands on a 5-hour cycle boundary from the original reset.
        let orig = DateTime::from_timestamp(1_000_000_000, 0).unwrap();
        assert_eq!((next - orig).num_seconds() % (300 * 60), 0);
    }

    #[test]
    fn fresh_window_is_kept_as_is() {
        let now = DateTime::from_timestamp(1_781_300_000, 0).unwrap();
        let w = RlWindow {
            used_percent: 42.0,
            window_minutes: Some(300),
            resets_at: Some(1_781_400_000), // future
        };
        let win = window_from(WindowKind::Session, "5h limit".to_string(), &w, now);
        assert_eq!(win.used_pct(), 42.0, "future-reset window kept as recorded");
    }

    #[test]
    fn no_sessions_dir_is_credentials_error() {
        let opts = FetchOptions {
            creds_path: Some(PathBuf::from("/nonexistent/codex/home")),
            ..Default::default()
        };
        assert!(matches!(
            Codex::new().fetch(&opts),
            Err(UsageError::CredentialsRead(_))
        ));
    }

    #[test]
    fn empty_sessions_dir_is_no_data() {
        let dir = tempdir();
        fs::create_dir_all(dir.join("sessions")).unwrap();
        let opts = FetchOptions {
            creds_path: Some(dir.join("sessions")),
            ..Default::default()
        };
        assert!(matches!(
            Codex::new().fetch(&opts),
            Err(UsageError::NoData(_))
        ));
        fs::remove_dir_all(&dir).ok();
    }

    /// A unique temp dir without pulling in the `tempfile` crate (keeps deps minimal).
    fn tempdir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("agent-usage-codex-test-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
