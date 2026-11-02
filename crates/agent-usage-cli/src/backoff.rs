//! Per-agent backoff after a usage source answers 429.
//!
//! Every CLI run is a fresh process, so polling and manual refreshes would otherwise keep hitting
//! a source that is already limiting us, which can keep the limit in place. The state lives next
//! to the snapshot cache at `$XDG_CACHE_HOME/agent-usage/<id>.backoff.json`, so each account backs
//! off independently. While it is active, a fetch fails with `RateLimited` without any request,
//! and callers handle that exactly like a real 429.
//!
//! Persistence is best-effort. A missing or corrupt file, or a wait longer than any this module
//! can schedule (a clock that moved), counts as no backoff: the cost is one extra request, whereas
//! honoring a bad deadline could silence an agent indefinitely.

use std::fs;
use std::path::{Path, PathBuf};

use agent_usage_core::UsageError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The wait after the first 429. Doubles per consecutive 429, up to [`MAX_DELAY_SECS`].
const BASE_DELAY_SECS: u64 = 300;
const MAX_DELAY_SECS: u64 = 3600;

#[derive(Debug, Serialize, Deserialize)]
struct State {
    consecutive: u32,
    not_before: DateTime<Utc>,
}

/// The wait after the `consecutive`-th 429 in a row. The source's own `Retry-After` wins when it
/// asks for longer; the schedule is the floor because a source may send none, or a useless 0.
fn delay_secs(consecutive: u32, retry_after: u64) -> u64 {
    let doublings = consecutive.saturating_sub(1).min(16);
    (BASE_DELAY_SECS << doublings)
        .min(MAX_DELAY_SECS)
        .max(retry_after)
        .min(MAX_DELAY_SECS)
}

/// Seconds left to wait, or `None` when a request may go out.
fn remaining_secs(state: &State, now: DateTime<Utc>) -> Option<u64> {
    let secs = (state.not_before - now).num_seconds();
    u64::try_from(secs)
        .ok()
        .filter(|s| *s > 0 && *s <= MAX_DELAY_SECS)
}

/// Where `id`'s backoff state lives under the cache directory `dir`.
pub fn state_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.backoff.json"))
}

fn load(path: &Path) -> Option<State> {
    let contents = fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

fn store(path: &Path, state: &State) {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(s) = serde_json::to_string(state) {
        let _ = fs::write(path, s);
    }
}

/// Run `fetch` unless the state at `path` says to wait. A 429 extends the backoff and a success
/// clears it; any other error leaves it alone, since it says nothing about the limit.
///
/// A returned `RateLimited` always carries the real wait, not whatever the source's header said.
pub fn guarded<T>(
    path: &Path,
    now: DateTime<Utc>,
    fetch: impl FnOnce() -> Result<T, UsageError>,
) -> Result<T, UsageError> {
    let state = load(path);
    if let Some(wait) = state.as_ref().and_then(|s| remaining_secs(s, now)) {
        return Err(UsageError::RateLimited(wait));
    }

    match fetch() {
        Ok(value) => {
            if state.is_some() {
                let _ = fs::remove_file(path);
            }
            Ok(value)
        }
        Err(UsageError::RateLimited(retry_after)) => {
            let consecutive = state.map_or(0, |s| s.consecutive).saturating_add(1);
            let delay = delay_secs(consecutive, retry_after);
            store(
                path,
                &State {
                    consecutive,
                    not_before: now + chrono::Duration::seconds(delay as i64),
                },
            );
            Err(UsageError::RateLimited(delay))
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A state path in a directory unique to this test, emptied first.
    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agent-usage-backoff-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        state_path(&dir, "claude")
    }

    fn limited() -> Result<u8, UsageError> {
        Err(UsageError::RateLimited(0))
    }

    fn wait_of(result: Result<u8, UsageError>) -> u64 {
        match result {
            Err(UsageError::RateLimited(wait)) => wait,
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[test]
    fn the_delay_doubles_up_to_the_cap() {
        let delays: Vec<u64> = (1..=6).map(|n| delay_secs(n, 0)).collect();
        assert_eq!(delays, [300, 600, 1200, 2400, 3600, 3600]);
        assert_eq!(delay_secs(u32::MAX, 0), MAX_DELAY_SECS);
    }

    #[test]
    fn a_longer_retry_after_wins_but_stays_capped() {
        assert_eq!(delay_secs(1, 900), 900);
        assert_eq!(delay_secs(3, 900), 1200);
        assert_eq!(delay_secs(1, u64::MAX), MAX_DELAY_SECS);
    }

    #[test]
    fn no_request_goes_out_during_the_wait() {
        let path = temp_path("wait");
        let now = Utc::now();
        assert_eq!(wait_of(guarded(&path, now, limited)), 300);

        let calls = Cell::new(0);
        let later = now + chrono::Duration::seconds(100);
        let result = guarded(&path, later, || {
            calls.set(calls.get() + 1);
            Ok(1u8)
        });
        assert_eq!(calls.get(), 0);
        assert_eq!(wait_of(result), 200);
    }

    #[test]
    fn another_429_after_the_wait_doubles_it() {
        let path = temp_path("double");
        let now = Utc::now();
        guarded(&path, now, limited).unwrap_err();
        let later = now + chrono::Duration::seconds(301);
        assert_eq!(wait_of(guarded(&path, later, limited)), 600);
    }

    #[test]
    fn a_success_after_the_wait_clears_the_state() {
        let path = temp_path("clear");
        let now = Utc::now();
        guarded(&path, now, limited).unwrap_err();
        let later = now + chrono::Duration::seconds(301);
        assert_eq!(guarded(&path, later, || Ok(1u8)).unwrap(), 1);
        assert!(!path.exists());
    }

    #[test]
    fn other_errors_leave_the_schedule_alone() {
        let path = temp_path("other");
        let now = Utc::now();
        guarded(&path, now, limited).unwrap_err();
        let later = now + chrono::Duration::seconds(301);
        let network = guarded::<u8>(&path, later, || Err(UsageError::Network("down".into())));
        assert!(matches!(network, Err(UsageError::Network(_))));
        assert_eq!(wait_of(guarded(&path, later, limited)), 600);
    }

    #[test]
    fn a_corrupt_file_does_not_block_fetching() {
        let path = temp_path("corrupt");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{nope").unwrap();
        assert_eq!(guarded(&path, Utc::now(), || Ok(1u8)).unwrap(), 1);
    }

    #[test]
    fn a_deadline_beyond_any_scheduled_wait_is_ignored() {
        let now = Utc::now();
        let state = State {
            consecutive: 1,
            not_before: now + chrono::Duration::seconds(MAX_DELAY_SECS as i64 + 1),
        };
        assert_eq!(remaining_secs(&state, now), None);
    }
}
