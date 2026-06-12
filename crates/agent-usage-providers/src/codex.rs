//! Codex provider (stub).
//!
//! The surface is wired up — `agent-usage codex` is a real subcommand and Codex appears in the
//! registry and the settings list — but the usage source is not implemented yet. The prototype
//! labels Codex "via local config" and shows a "5h limit" window, so the intended shape is a
//! single [`WindowKind::Session`] read from Codex's on-disk config/usage state under
//! `~/.codex/`. Until that exists, `fetch` returns a clear [`UsageError::Unsupported`], which
//! the CLI still renders as a valid JSON error snapshot.

use agent_usage_core::{FetchOptions, Provider, Usage, UsageError};

const DEFAULT_CONFIG_PATH: &str = "~/.codex/";

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
        "local config"
    }

    fn fetch(&self, _opts: &FetchOptions) -> Result<Usage, UsageError> {
        let _ = DEFAULT_CONFIG_PATH;
        Err(UsageError::Unsupported(format!(
            "codex usage is not implemented yet (planned source: {} local config)",
            DEFAULT_CONFIG_PATH
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_and_unsupported() {
        let c = Codex::new();
        assert_eq!(c.id(), "codex");
        assert!(matches!(
            c.fetch(&FetchOptions::default()),
            Err(UsageError::Unsupported(_))
        ));
    }
}
