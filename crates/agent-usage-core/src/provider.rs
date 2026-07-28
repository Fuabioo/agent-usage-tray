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
    /// API key for agents authenticated by one (currently Hyper). Resolved by the caller so the
    /// key can come from a config file as well as the environment — which is what lets an agent
    /// survive a reboot, since an app started by launchd inherits no shell profile.
    pub api_key: Option<String>,
    /// When an agent's daily cycle resets, for the agents whose API doesn't report it (currently
    /// Hyper, whose `/v1/credits` returns only a balance). `HH:MM` with an optional zone — see
    /// the Hyper provider. Takes precedence over whatever environment variable the provider would
    /// otherwise read, so a GUI can own the setting instead of requiring a shell export.
    pub reset_time: Option<String>,
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
            reset_time: None,
            api_key: None,
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
    ///
    /// Takes the resolved options because "is this configured" may depend on them: a key supplied
    /// by a config file counts exactly as much as one exported into the environment.
    fn in_default_set(&self, _opts: &FetchOptions) -> bool {
        true
    }
}
