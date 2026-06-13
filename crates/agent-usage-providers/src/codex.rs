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
//! The response's `rate_limit.primary_window` is the rolling 5-hour window and `secondary_window`
//! the weekly one (`used_percent`, `limit_window_seconds`, `reset_at`). This is live and current —
//! unlike the session rollout logs (which only update when Codex runs and go stale), so it stays
//! accurate even while you're actively using Codex. Confirmed against the live API.

use std::path::PathBuf;

use agent_usage_core::{AgentInfo, FetchOptions, Provider, Usage, UsageError, Window, WindowKind};
use serde::Deserialize;

use crate::creds;
use crate::http;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const DEFAULT_AUTH_PATH: &str = "~/.codex/auth.json";

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
    if let Some(w) = rl.primary_window {
        windows.push(window_from(WindowKind::Session, &w));
    }
    if let Some(w) = rl.secondary_window {
        windows.push(window_from(WindowKind::Weekly, &w));
    }
    windows
}

fn window_from(kind: WindowKind, w: &RlWindow) -> Window {
    let resets_at = w
        .reset_at
        .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0));
    let label = match kind {
        WindowKind::Weekly => "weekly".to_string(),
        _ => window_label(w.limit_window_seconds),
    };
    Window::utilization(kind, label, w.used_percent, resets_at)
}

/// Human label for the short window from its length: 18000 s -> "5h limit".
fn window_label(secs: Option<i64>) -> String {
    match secs {
        Some(s) if s > 0 && s % 3600 == 0 => format!("{}h limit", s / 3600),
        Some(s) if s > 0 && s % 60 == 0 => format!("{}m limit", s / 60),
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
        assert_eq!(window_label(Some(18000)), "5h limit"); // 5h
        assert_eq!(window_label(Some(900)), "15m limit"); // 15m
        assert_eq!(window_label(None), "session");
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
