//! Concrete agent usage providers and a registry to look them up by id.
//!
//! Adding an agent is a two-line change here (a module + a registry entry); nothing downstream
//! — not the CLI, not the menu bar — needs to know the agent exists ahead of time. The CLI
//! resolves a subcommand like `claude` straight out of [`get`], and `agent-usage all` iterates
//! [`all`], so every new provider shows up everywhere automatically.

mod creds;
mod http;

pub mod claude;
pub mod codex;
pub mod hyper;

use agent_usage_core::Provider;

pub use claude::Claude;
pub use codex::Codex;
pub use hyper::Hyper;

/// Every known provider, in display order. The CLI's `all` command iterates this.
pub fn all() -> Vec<Box<dyn Provider>> {
    vec![Box::new(Claude::new()), Box::new(Codex::new()), Box::new(Hyper::new())]
}

/// Look up a provider by its id (the CLI subcommand). Returns `None` for unknown ids.
pub fn get(id: &str) -> Option<Box<dyn Provider>> {
    all().into_iter().find(|p| p.id() == id)
}

/// The ids of all known providers, for help text and validation.
pub fn ids() -> Vec<&'static str> {
    all().iter().map(|p| p.id()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_claude_codex_and_hyper() {
        assert!(get("claude").is_some());
        assert!(get("codex").is_some());
        assert!(get("hyper").is_some());
        assert!(get("nope").is_none());
        assert_eq!(ids(), vec!["claude", "codex", "hyper"]);
    }
}
