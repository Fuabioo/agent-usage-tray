//! Where on-disk caches live.
//!
//! Several crates keep small best-effort cache files under one shared directory
//! (`$XDG_CACHE_HOME/agent-usage`, or `~/.cache/agent-usage`). This resolves that
//! base directory; callers append their own file name. Pure path computation — no I/O.

use std::path::PathBuf;

/// The base cache directory: `$XDG_CACHE_HOME/agent-usage`, falling back to
/// `~/.cache/agent-usage`. Returns `None` when neither variable is set.
pub fn cache_dir() -> Option<PathBuf> {
    resolve_cache_dir(
        std::env::var("XDG_CACHE_HOME").ok(),
        std::env::var("HOME").ok(),
    )
}

/// Pure resolution from the two env values, so it can be tested without mutating
/// process-global environment state (which races under parallel test execution).
fn resolve_cache_dir(xdg_cache_home: Option<String>, home: Option<String>) -> Option<PathBuf> {
    if let Some(x) = xdg_cache_home {
        if !x.is_empty() {
            return Some(PathBuf::from(x).join("agent-usage"));
        }
    }
    home.map(|h| PathBuf::from(h).join(".cache/agent-usage"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_takes_precedence() {
        assert_eq!(
            resolve_cache_dir(Some("/tmp/xdg".into()), Some("/home/test".into())),
            Some(PathBuf::from("/tmp/xdg/agent-usage"))
        );
    }

    #[test]
    fn falls_back_to_home_when_xdg_unset() {
        assert_eq!(
            resolve_cache_dir(None, Some("/home/test".into())),
            Some(PathBuf::from("/home/test/.cache/agent-usage"))
        );
    }

    #[test]
    fn empty_xdg_falls_back_to_home() {
        assert_eq!(
            resolve_cache_dir(Some(String::new()), Some("/home/test".into())),
            Some(PathBuf::from("/home/test/.cache/agent-usage"))
        );
    }

    #[test]
    fn none_when_neither_is_set() {
        assert_eq!(resolve_cache_dir(None, None), None);
    }
}
