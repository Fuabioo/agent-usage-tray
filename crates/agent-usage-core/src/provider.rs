//! The contract every agent implements.
//!
//! A [`Provider`] knows three things about itself (id, label, source) and how to `fetch` its
//! current usage normalized into the shared [`Usage`] schema. Implementations live in the
//! `agent-usage-providers` crate; this crate only defines the interface so that the pure-logic
//! core carries no network or credential dependencies.

use std::path::PathBuf;
use std::time::Duration;

use crate::error::UsageError;
use crate::schema::Usage;

/// Knobs that influence how a provider resolves credentials and fetches usage. Providers use
/// only the fields relevant to their source and ignore the rest, so one option set works for
/// every agent.
#[derive(Debug, Clone)]
pub struct FetchOptions {
    /// Explicit path to a credentials file, overriding the provider's default location.
    pub creds_path: Option<PathBuf>,
    /// A config directory to resolve the *default* credentials location under, for agents that
    /// support multiple accounts (e.g. a second Claude Code login under `~/.claude-personal`).
    /// Unlike `creds_path` this is not authoritative: the agent's Keychain fallback still
    /// applies when the file is absent. Ignored when `creds_path` is set.
    pub creds_dir: Option<PathBuf>,
    /// HTTP request timeout.
    pub timeout: Duration,
    /// macOS Keychain generic-password service to fall back to (providers that support it).
    pub keychain_service: Option<String>,
    /// macOS Keychain generic-password *account* to disambiguate when one service holds an
    /// entry per login (the `security -a` attribute). `None` matches the service's sole/first
    /// entry.
    pub keychain_account: Option<String>,
    /// When true, never consult the macOS Keychain.
    pub no_keychain: bool,
}

impl Default for FetchOptions {
    fn default() -> Self {
        FetchOptions {
            creds_path: None,
            creds_dir: None,
            timeout: Duration::from_secs(30),
            keychain_service: None,
            keychain_account: None,
            no_keychain: false,
        }
    }
}

/// A usage source for one agent. Object-safe so providers can be stored as `Box<dyn Provider>`
/// in a registry and dispatched by id.
pub trait Provider: Send + Sync {
    /// Stable lowercase id, also the CLI subcommand (e.g. `claude`).
    fn id(&self) -> &'static str;

    /// Human display name (e.g. "Claude Code").
    fn label(&self) -> &'static str;

    /// Human description of where the numbers come from (e.g. "Anthropic OAuth usage API").
    fn source(&self) -> &'static str;

    /// Fetch and normalize current usage for this agent.
    fn fetch(&self, opts: &FetchOptions) -> Result<Usage, UsageError>;

    /// Whether this agent joins the default `all` sweep. Expected agents (Claude, Codex) return
    /// `true` even when unconfigured, so they surface a "log in" error that nudges the user.
    /// Opt-in agents override this to return `false` until their credentials are present, so a
    /// fresh install isn't cluttered with an agent nobody set up. Direct lookup by id (the
    /// per-agent subcommand) ignores this, so `agent-usage <id>` always works and errors clearly.
    fn in_default_set(&self) -> bool {
        true
    }
}
