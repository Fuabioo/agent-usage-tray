//! Charm Hyper provider.
//!
//! Calls `GET https://hyper.charm.land/v1/credits` with an API key
//! (from `HYPER_API_KEY` env var) and returns a `Credits` pool window.
//!
//! Hyper's subscription model:
//!   - 250 HC refresh every 24h (non-stackable, unused expires)
//!   - Permanent purchased credits are consumed *after* daily allocation
//!     and never expire
//!
//! The API returns only a single `balance` (int). To derive `total` we need the
//! permanent-credit count: each 24h cycle (keyed by its reset instant) we re-derive
//! it as `max(0, balance - 250)`, but never below the last known baseline — permanent
//! credits persist across cycles, so a mid-cycle cold start (when `balance` no longer
//! reflects a full daily grant) must not undercount them. The baseline is cached in
//! `~/.cache/agent-usage/hyper.permanent.json`. The reset moment comes from the
//! `HYPER_RESET_TIME` env var (UTC `HH:MM`, e.g. `20:18`); an unset value defaults to
//! midnight UTC, and a malformed value is a hard error rather than a silent fallback.

use agent_usage_core::{AgentInfo, FetchOptions, Provider, Usage, UsageError, Window};

use crate::http;

const DAILY: i64 = 250;
const USAGE_URL: &str = "https://hyper.charm.land/v1/credits";

/// The API response shape.
#[derive(serde::Deserialize)]
pub struct CreditsResponse {
    pub balance: i64,
}

pub struct Hyper;

impl Hyper {
    pub const fn new() -> Self {
        Hyper
    }
}

impl Default for Hyper {
    fn default() -> Self {
        Hyper::new()
    }
}

impl Provider for Hyper {
    fn id(&self) -> &'static str {
        "hyper"
    }

    fn label(&self) -> &'static str {
        "Charm Hyper"
    }

    fn source(&self) -> &'static str {
        "Hyper /v1/credits API"
    }

    fn fetch(&self, opts: &FetchOptions) -> Result<Usage, UsageError> {
        let api_key = resolve_api_key()?;
        let now = chrono::Utc::now();

        let bearer = format!("Bearer {api_key}");
        let body = http::get(
            USAGE_URL,
            &[
                ("Authorization", bearer.as_str()),
                ("Accept", "application/json"),
            ],
            opts.timeout,
        )?;

        let parsed: CreditsResponse =
            serde_json::from_str(&body).map_err(|e| UsageError::Parse(e.to_string()))?;

        let balance = parsed.balance;
        let reset = parse_reset_time(&std::env::var("HYPER_RESET_TIME").unwrap_or_default())?;
        let resets_at = next_reset(now, reset);

        let permanent = resolve_permanent("hyper", balance, resets_at.timestamp());

        let total = permanent as f64 + DAILY as f64;
        let remaining = balance as f64;

        Ok(Usage {
            agent: AgentInfo {
                id: self.id().to_string(),
                label: self.label().to_string(),
                source: self.source().to_string(),
            },
            // `total`/`remaining` carry the raw balance (250 daily grant + permanent surplus),
            // but the percentage is measured against the daily grant via `with_budget`: spending
            // beyond the day's 250 dips into the permanent surplus and reads as "extra usage"
            // (over 100%). No `burn_per_day` — the grant is fixed, not an observed burn rate, so
            // there is nothing to project. Label is a lowercase noun ("credits") to match the
            // other windows' convention, which the UI renders as "<label> left".
            windows: vec![Window::pool("credits", total, remaining, None, Some(resets_at))
                .with_budget(DAILY as f64)],
        })
    }
}

/// Read the API key from the `HYPER_API_KEY` environment variable.
fn resolve_api_key() -> Result<String, UsageError> {
    std::env::var("HYPER_API_KEY")
        .map_err(|_| UsageError::CredentialsRead("HYPER_API_KEY not set".to_string()))
}

/// Compute the *next* reset instant from `now`, given an optional `HH:MM` UTC reset
/// time. `None` defaults to midnight UTC.
fn next_reset(
    now: chrono::DateTime<chrono::Utc>,
    reset: Option<(u32, u32)>,
) -> chrono::DateTime<chrono::Utc> {
    let (hour, min) = reset.unwrap_or((0, 0));
    let today = now
        .date_naive()
        .and_hms_opt(hour, min, 0)
        .map(|t| t.and_utc())
        .unwrap_or(now);
    if today > now {
        today
    } else {
        today + chrono::Duration::days(1)
    }
}

/// Parse a `HYPER_RESET_TIME` value (UTC `HH:MM`, e.g. `20:18`). An empty value yields
/// `None` (caller defaults to midnight UTC); a non-empty but malformed value is a hard
/// error, so a typo surfaces instead of silently shifting the reset to midnight.
fn parse_reset_time(raw: &str) -> Result<Option<(u32, u32)>, UsageError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let bad =
        || UsageError::Unsupported(format!("HYPER_RESET_TIME must be UTC HH:MM, got {raw:?}"));
    let (h, m) = raw.split_once(':').ok_or_else(bad)?;
    let hour = h.parse::<u32>().ok().filter(|h| *h < 24).ok_or_else(bad)?;
    let min = m.parse::<u32>().ok().filter(|m| *m < 60).ok_or_else(bad)?;
    Ok(Some((hour, min)))
}

/// Resolve the permanent-credit baseline for `balance` in the cycle identified by
/// `cycle` (the reset instant, unix seconds). Within a known cycle the cached value is
/// reused; on a new (or uncached) cycle we re-derive it but never drop below the last
/// known baseline (see [`derive_permanent`]), then persist it. The cache write is
/// best-effort — a failure just means we re-derive on the next fetch.
fn resolve_permanent(id: &str, balance: i64, cycle: i64) -> u32 {
    let stored = perm::read(id);
    if let Some(r) = &stored {
        if r.cycle == cycle {
            return r.value;
        }
    }
    let value = derive_permanent(stored.map_or(0, |r| r.value), balance);
    perm::write(id, &perm::Record { value, cycle });
    value
}

/// New-cycle baseline: `balance - DAILY` (exact only at reset, when the grant is full),
/// floored at the previously known baseline so a mid-cycle cold start cannot undercount
/// permanent credits, and at zero.
fn derive_permanent(previous: u32, balance: i64) -> u32 {
    previous.max((balance - DAILY).max(0) as u32)
}

/// Tiny permanent-credits baseline cache, stored alongside the main snapshot cache and
/// best-effort like it: read/write failures are swallowed, since a miss just triggers
/// re-derivation on the next fetch.
mod perm {
    use agent_usage_core::cache_dir;
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Record {
        pub value: u32,
        /// Identity of the cycle this baseline was derived for: the reset instant as
        /// unix seconds. A different reset instant means a new cycle.
        pub cycle: i64,
    }

    fn cache_path(id: &str) -> Option<PathBuf> {
        cache_dir().map(|d| d.join(format!("{id}.permanent.json")))
    }

    pub fn read(id: &str) -> Option<Record> {
        let path = cache_path(id)?;
        let contents = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&contents).ok()
    }

    pub fn write(id: &str, rec: &Record) {
        let Some(path) = cache_path(id) else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string(rec) {
            let _ = std::fs::write(path, s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    #[test]
    fn identity() {
        let h = Hyper::new();
        assert_eq!(h.id(), "hyper");
        assert_eq!(h.label(), "Charm Hyper");
    }

    #[test]
    fn parses_balance() {
        let body = r#"{"balance": 610}"#;
        let parsed: CreditsResponse = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.balance, 610);
    }

    #[test]
    fn reset_defaults_to_midnight_when_unset() {
        let now = chrono::Utc::now();
        let r = next_reset(now, parse_reset_time("").unwrap());
        assert!(r > now);
        assert_eq!(r.hour(), 0);
        assert_eq!(r.minute(), 0);
    }

    #[test]
    fn reset_time_parses_hh_mm() {
        assert_eq!(parse_reset_time("20:18").unwrap(), Some((20, 18)));
        assert_eq!(parse_reset_time("00:00").unwrap(), Some((0, 0)));
        assert_eq!(parse_reset_time("   ").unwrap(), None);
    }

    #[test]
    fn malformed_reset_time_is_an_error() {
        for bad in ["20", "24:00", "12:60", "ab:cd", "-1:00"] {
            assert!(parse_reset_time(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn permanent_derivation_floors_at_previous_and_zero() {
        // At reset the grant is full, so balance - DAILY recovers permanent exactly.
        assert_eq!(derive_permanent(0, 610), 360);
        // Mid-cycle cold start (balance below a full grant) keeps the known baseline.
        assert_eq!(derive_permanent(360, 200), 360);
        // Nothing known and balance below the grant: zero, never negative.
        assert_eq!(derive_permanent(0, 200), 0);
        // A balance proving more permanent than we knew raises the baseline.
        assert_eq!(derive_permanent(100, 610), 360);
    }
}
