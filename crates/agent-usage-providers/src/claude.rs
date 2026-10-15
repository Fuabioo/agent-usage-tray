//! Claude Code provider.
//!
//! Reads the OAuth token Claude Code stores (the `~/.claude/.credentials.json` file on Linux,
//! or the login Keychain on macOS), calls the Anthropic OAuth usage API, and normalizes the
//! `seven_day` / `five_hour` windows into the shared schema. Ported from the original
//! `cc-usage` tool — same endpoint, same credential handling, same numbers.

use std::path::PathBuf;

use agent_usage_core::{FetchOptions, Provider, Usage, UsageError, Window, WindowKind};
use serde::Deserialize;

use crate::creds;
use crate::http;

const API_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const DEFAULT_CREDS_PATH: &str = "~/.claude/.credentials.json";
/// The credentials file's name inside a Claude Code config dir (`$CLAUDE_CONFIG_DIR`), used to
/// locate a non-default account's token, e.g. `~/.claude-personal/.credentials.json`.
const CREDS_FILE_NAME: &str = ".credentials.json";
const DEFAULT_KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// The Anthropic OAuth usage response (only the fields we use).
#[derive(Debug, Deserialize)]
struct UsageResponse {
    seven_day: UsageWindow,
    five_hour: UsageWindow,
}

#[derive(Debug, Deserialize)]
struct UsageWindow {
    utilization: f64,
    /// `null` when the window has no scheduled reset — e.g. the five-hour window while no
    /// session is active. Kept as `None` rather than guessed; consumers skip reset-based pace.
    #[serde(default)]
    resets_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub struct Claude;

impl Claude {
    pub const fn new() -> Self {
        Claude
    }
}

impl Default for Claude {
    fn default() -> Self {
        Claude::new()
    }
}

impl Provider for Claude {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn label(&self) -> &'static str {
        "Claude Code"
    }

    fn source(&self) -> &'static str {
        "Anthropic OAuth usage API"
    }

    fn fetch(&self, opts: &FetchOptions) -> Result<Usage, UsageError> {
        let token = resolve_token(opts)?;

        let bearer = format!("Bearer {token}");
        let body = http::get(
            API_URL,
            &[
                ("Authorization", bearer.as_str()),
                ("anthropic-beta", "oauth-2025-04-20"),
                ("Accept", "application/json"),
            ],
            opts.timeout,
        )?;

        Ok(Usage {
            agent: agent_usage_core::AgentInfo {
                id: self.id().to_string(),
                label: self.label().to_string(),
                source: self.source().to_string(),
            },
            windows: parse_windows(&body)?,
        })
    }
}

/// Normalize the usage response body into the weekly and session windows.
fn parse_windows(body: &str) -> Result<Vec<Window>, UsageError> {
    let parsed: UsageResponse =
        serde_json::from_str(body).map_err(|e| UsageError::Parse(e.to_string()))?;

    Ok(vec![
        Window::utilization(
            WindowKind::Weekly,
            "weekly",
            parsed.seven_day.utilization,
            parsed.seven_day.resets_at,
        ),
        Window::utilization(
            WindowKind::Session,
            "session",
            parsed.five_hour.utilization,
            parsed.five_hour.resets_at,
        ),
    ])
}

/// Resolve the OAuth bearer token: explicit/default file first, macOS Keychain as fallback.
///
/// Account selection (for a second Claude Code login) rides on `FetchOptions`:
/// - `creds_path` — an explicit file, authoritative (no Keychain fallback);
/// - `creds_dir` — a config dir whose `.credentials.json` is the *default* file, with the Keychain
///   fallback still applying (e.g. `~/.claude-personal`);
/// - `keychain_account` — disambiguates the Keychain entry when one service holds several logins.
fn resolve_token(opts: &FetchOptions) -> Result<String, UsageError> {
    let explicit = opts.creds_path.is_some();
    let path: PathBuf = opts.creds_path.clone().unwrap_or_else(|| match &opts.creds_dir {
        Some(dir) => dir.join(CREDS_FILE_NAME),
        None => creds::expand_tilde(DEFAULT_CREDS_PATH),
    });

    let file_result = creds::read_file(&path).and_then(|c| parse_token(&c));

    // A working file, or an explicitly-chosen file, is authoritative.
    if file_result.is_ok() || explicit {
        return file_result;
    }

    // No usable default file: on macOS, Claude Code keeps the token in the Keychain.
    #[cfg(target_os = "macos")]
    {
        if !opts.no_keychain {
            let service = opts
                .keychain_service
                .as_deref()
                .unwrap_or(DEFAULT_KEYCHAIN_SERVICE);
            match creds::read_keychain(service, opts.keychain_account.as_deref()) {
                Ok(blob) => {
                    return if blob.starts_with('{') {
                        parse_token(&blob)
                    } else {
                        Ok(blob) // bare token stored directly
                    };
                }
                Err(kc_err) => {
                    let file_err = file_result.unwrap_err();
                    return Err(UsageError::CredentialsRead(format!(
                        "no credentials file ({file_err}); Keychain fallback failed ({kc_err})"
                    )));
                }
            }
        }
    }
    let _ = DEFAULT_KEYCHAIN_SERVICE; // silence unused on non-macOS

    file_result
}

/// Extract the access token from Claude's `{"claudeAiOauth":{"accessToken":"..."}}` shape.
fn parse_token(content: &str) -> Result<String, UsageError> {
    let parsed: serde_json::Value =
        serde_json::from_str(content).map_err(|e| UsageError::CredentialsParse(e.to_string()))?;

    parsed
        .get("claudeAiOauth")
        .and_then(|o| o.get("accessToken"))
        .and_then(|t| t.as_str())
        .map(String::from)
        .ok_or(UsageError::CredentialsMissingToken)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_token_ok() {
        let json = r#"{"claudeAiOauth":{"accessToken":"tok-123"}}"#;
        assert_eq!(parse_token(json).unwrap(), "tok-123");
    }

    #[test]
    fn parse_token_missing() {
        assert!(matches!(
            parse_token(r#"{"other":1}"#),
            Err(UsageError::CredentialsMissingToken)
        ));
    }

    #[test]
    fn parse_token_bad_json() {
        assert!(matches!(
            parse_token("{nope"),
            Err(UsageError::CredentialsParse(_))
        ));
    }

    #[test]
    fn parse_windows_with_reset() {
        let body = r#"{"five_hour":{"utilization":12.0,"resets_at":"2026-10-15T20:00:00Z"},
                       "seven_day":{"utilization":40.0,"resets_at":"2026-10-20T02:00:00Z"}}"#;
        let windows = parse_windows(body).unwrap();
        assert_eq!(windows[0].kind, WindowKind::Weekly);
        assert_eq!(
            windows[0].resets_at.unwrap().to_rfc3339(),
            "2026-10-20T02:00:00+00:00"
        );
        assert_eq!(
            windows[1].resets_at.unwrap().to_rfc3339(),
            "2026-10-15T20:00:00+00:00"
        );
    }

    #[test]
    fn parse_windows_null_reset() {
        // An idle five-hour window comes back with `resets_at: null`.
        let body = r#"{"five_hour":{"utilization":0.0,"resets_at":null},"seven_day":{"utilization":40.0,"resets_at":"2026-10-20T02:00:00Z"},"extra_usage":null}"#;
        let windows = parse_windows(body).unwrap();
        assert_eq!(windows[1].kind, WindowKind::Session);
        assert_eq!(windows[1].used_pct(), 0.0);
        assert_eq!(windows[1].resets_at, None);
        assert!(windows[0].resets_at.is_some());
    }

    #[test]
    fn parse_windows_missing_reset() {
        let body = r#"{"five_hour":{"utilization":0.0},"seven_day":{"utilization":5.0,"resets_at":null}}"#;
        let windows = parse_windows(body).unwrap();
        assert!(windows.iter().all(|w| w.resets_at.is_none()));
    }

    #[test]
    fn identity() {
        let c = Claude::new();
        assert_eq!(c.id(), "claude");
        assert_eq!(c.label(), "Claude Code");
    }
}
