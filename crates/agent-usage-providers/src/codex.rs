//! Codex provider — live usage from the Codex/ChatGPT backend.
//!
//! Codex authenticates via ChatGPT and exposes a **read-only usage endpoint** (the same one the
//! open-source CLI's `BackendClient::get_rate_limits` / background memories check use):
//!
//! ```text
//! GET https://chatgpt.com/backend-api/wham/usage
//!   Authorization: Bearer <access_token from ~/.codex/auth.json>
//!   ChatGPT-Account-Id: <account_id>
//! ```
//!
//! The response carries up to two windows under `rate_limit`, each with `used_percent`,
//! `limit_window_seconds` and `reset_at`. This is live and current — unlike the session rollout
//! logs (which only update when Codex runs and go stale), so it stays accurate even while you're
//! actively using Codex. Confirmed against the live API.
//!
//! **The window slots are not stable.** `primary_window` once held a rolling 5-hour limit with the
//! weekly one in `secondary_window`; plans that bill a single weekly quota now report *that* as
//! `primary_window` and leave `secondary_window` null. So each window's role is derived from its
//! `limit_window_seconds` (see [`window_kind`]) rather than from the slot it arrived in — the slot
//! is only a fallback for the case where the API omits the length.

use std::path::PathBuf;

use agent_usage_core::{AgentInfo, FetchOptions, Provider, Usage, UsageError, Window, WindowKind};
use serde::Deserialize;

use crate::creds;
use crate::http;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const DEFAULT_AUTH_PATH: &str = "~/.codex/auth.json";

const DAY_SECS: i64 = 86_400;
const WEEK_SECS: i64 = 7 * DAY_SECS;

// --- auth.json (only the fields we read) ---
#[derive(Deserialize)]
struct AuthFile {
    tokens: Option<Tokens>,
}
#[derive(Deserialize)]
struct Tokens {
    access_token: Option<String>,
    account_id: Option<String>,
}

// --- usage response (only the fields we read) ---
#[derive(Deserialize)]
struct UsageResponse {
    rate_limit: Option<RateLimit>,
}
#[derive(Deserialize)]
struct RateLimit {
    primary_window: Option<RlWindow>,
    secondary_window: Option<RlWindow>,
}
#[derive(Deserialize)]
struct RlWindow {
    used_percent: f64,
    limit_window_seconds: Option<i64>,
    reset_at: Option<i64>,
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
        "Codex/ChatGPT usage API"
    }

    fn fetch(&self, opts: &FetchOptions) -> Result<Usage, UsageError> {
        let (token, account_id) = read_codex_auth(opts)?;

        let bearer = format!("Bearer {token}");
        let mut headers: Vec<http::Header<'_>> = vec![
            ("Authorization", bearer.as_str()),
            ("User-Agent", "codex-cli"),
            ("Accept", "application/json"),
        ];
        if let Some(acc) = account_id.as_deref() {
            headers.push(("ChatGPT-Account-Id", acc));
        }

        let body = http::get(USAGE_URL, &headers, opts.timeout)?;
        let parsed: UsageResponse =
            serde_json::from_str(&body).map_err(|e| UsageError::Parse(e.to_string()))?;

        let windows = windows_from(parsed.rate_limit.ok_or_else(|| {
            UsageError::NoData("Codex usage response had no rate_limit".to_string())
        })?);
        if windows.is_empty() {
            return Err(UsageError::NoData(
                "Codex usage response had no rate-limit windows".to_string(),
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

fn windows_from(rl: RateLimit) -> Vec<Window> {
    let mut windows = Vec::new();
    // The slot each window arrives in is only the fallback role; its length decides.
    if let Some(w) = rl.primary_window {
        windows.push(window_from(&w, WindowKind::Session));
    }
    if let Some(w) = rl.secondary_window {
        windows.push(window_from(&w, WindowKind::Weekly));
    }
    windows
}

fn window_from(w: &RlWindow, slot_role: WindowKind) -> Window {
    let resets_at = w
        .reset_at
        .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0));
    let kind = window_kind(w.limit_window_seconds, slot_role);
    Window::utilization(
        kind,
        window_label(w.limit_window_seconds, kind),
        w.used_percent,
        resets_at,
    )
}

/// The role a rate-limit window plays, taken from its **length** rather than the slot it arrived
/// in. Anything a day or longer is a multi-day budget that has to be paced across the cycle;
/// anything shorter is a short rolling window with fixed thresholds. `slot_role` applies only
/// when the API omits `limit_window_seconds`.
///
/// Getting this wrong is not cosmetic: a weekly quota misread as a session window is colored by
/// fixed thresholds instead of pace, so a week's budget spent on its first day still reads green.
fn window_kind(secs: Option<i64>, slot_role: WindowKind) -> WindowKind {
    match secs {
        Some(s) if s >= DAY_SECS => WindowKind::Weekly,
        Some(s) if s > 0 => WindowKind::Session,
        _ => slot_role,
    }
}

/// Human label from a window's length: 18000 s -> "5h limit", 604800 s -> "weekly". Falls back to
/// the window's role when the API omits the length.
fn window_label(secs: Option<i64>, kind: WindowKind) -> String {
    match secs {
        Some(s) if s == WEEK_SECS => "weekly".to_string(),
        Some(s) if s > 0 && s % DAY_SECS == 0 => format!("{}d limit", s / DAY_SECS),
        Some(s) if s > 0 && s % 3600 == 0 => format!("{}h limit", s / 3600),
        Some(s) if s > 0 && s % 60 == 0 => format!("{}m limit", s / 60),
        _ if kind == WindowKind::Weekly => "weekly".to_string(),
        _ => "session".to_string(),
    }
}

/// Read the ChatGPT access token and account id from Codex's `auth.json`.
fn read_codex_auth(opts: &FetchOptions) -> Result<(String, Option<String>), UsageError> {
    let path: PathBuf = opts
        .creds_path
        .clone()
        .unwrap_or_else(|| creds::expand_tilde(DEFAULT_AUTH_PATH));
    let content = creds::read_file(&path)?;
    let auth: AuthFile =
        serde_json::from_str(&content).map_err(|e| UsageError::CredentialsParse(e.to_string()))?;
    let tokens = auth.tokens.ok_or(UsageError::CredentialsMissingToken)?;
    let token = tokens
        .access_token
        .filter(|t| !t.is_empty())
        .ok_or(UsageError::CredentialsMissingToken)?;
    Ok((token, tokens.account_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity() {
        let c = Codex::new();
        assert_eq!(c.id(), "codex");
        assert_eq!(c.label(), "Codex");
    }

    #[test]
    fn window_label_formats() {
        assert_eq!(window_label(Some(18000), WindowKind::Session), "5h limit");
        assert_eq!(window_label(Some(900), WindowKind::Session), "15m limit");
        assert_eq!(window_label(Some(604800), WindowKind::Weekly), "weekly");
        assert_eq!(window_label(Some(3 * 86400), WindowKind::Weekly), "3d limit");
        // No length reported: the slot's role names it.
        assert_eq!(window_label(None, WindowKind::Session), "session");
        assert_eq!(window_label(None, WindowKind::Weekly), "weekly");
    }

    #[test]
    fn window_kind_comes_from_length_not_slot() {
        // A weekly quota in the primary slot is still weekly.
        assert_eq!(window_kind(Some(604800), WindowKind::Session), WindowKind::Weekly);
        assert_eq!(window_kind(Some(86400), WindowKind::Session), WindowKind::Weekly);
        // Under a day is a short rolling window wherever it arrives.
        assert_eq!(window_kind(Some(18000), WindowKind::Weekly), WindowKind::Session);
        // Only a missing length falls back to the slot.
        assert_eq!(window_kind(None, WindowKind::Weekly), WindowKind::Weekly);
        assert_eq!(window_kind(None, WindowKind::Session), WindowKind::Session);
    }

    #[test]
    fn parses_usage_response() {
        let body = r#"{
            "plan_type": "team",
            "rate_limit": {
                "primary_window":   {"used_percent": 78, "limit_window_seconds": 18000, "reset_at": 1781327156},
                "secondary_window": {"used_percent": 22, "limit_window_seconds": 604800, "reset_at": 1781803709}
            }
        }"#;
        let parsed: UsageResponse = serde_json::from_str(body).unwrap();
        let windows = windows_from(parsed.rate_limit.unwrap());
        assert_eq!(windows.len(), 2);

        assert_eq!(windows[0].kind, WindowKind::Session);
        assert_eq!(windows[0].label, "5h limit");
        assert_eq!(windows[0].used_pct(), 78.0);
        assert!(windows[0].resets_at.is_some());

        assert_eq!(windows[1].kind, WindowKind::Weekly);
        assert_eq!(windows[1].label, "weekly");
        assert_eq!(windows[1].used_pct(), 22.0);
    }

    /// The single-weekly-quota shape: the 7-day window moved into `primary_window` and
    /// `secondary_window` went null. It must still be paced as a weekly budget.
    #[test]
    fn parses_single_weekly_quota_response() {
        let body = r#"{
            "plan_type": "team",
            "rate_limit": {
                "allowed": true,
                "primary_window": {"used_percent": 31, "limit_window_seconds": 604800, "reset_at": 1785768607},
                "secondary_window": null
            }
        }"#;
        let parsed: UsageResponse = serde_json::from_str(body).unwrap();
        let windows = windows_from(parsed.rate_limit.unwrap());
        assert_eq!(windows.len(), 1);
        assert_eq!(
            windows[0].kind,
            WindowKind::Weekly,
            "a 7-day quota in the primary slot must be paced, not read as a session"
        );
        assert_eq!(windows[0].label, "weekly");
        assert_eq!(windows[0].used_pct(), 31.0);
        assert!(windows[0].resets_at.is_some());
    }

    #[test]
    fn auth_missing_token_errors() {
        // A temp auth.json without an access token.
        let dir = std::env::temp_dir().join(format!(
            "agent-usage-codex-auth-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("auth.json");
        std::fs::write(&path, r#"{"tokens":{"account_id":"abc"}}"#).unwrap();
        let opts = FetchOptions {
            creds_path: Some(path),
            ..Default::default()
        };
        assert!(matches!(
            Codex::new().fetch(&opts),
            Err(UsageError::CredentialsMissingToken)
        ));
        std::fs::remove_dir_all(&dir).ok();
    }
}
