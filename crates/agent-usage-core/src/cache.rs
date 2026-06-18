//! Where on-disk caches live.
//!
//! Several crates keep small best-effort cache files under one shared directory
//! (`$XDG_CACHE_HOME/agent-usage`, or `~/.cache/agent-usage`). This resolves that
//! base directory; callers append their own file name. Pure path computation — no I/O.

use std::path::PathBuf;

/// The base cache directory: `$XDG_CACHE_HOME/agent-usage`, falling back to
/// `~/.cache/agent-usage`. Returns `None` when neither variable is set.
pub fn cache_dir() -> Option<PathBuf> {
    if let Ok(x) = std::env::var("XDG_CACHE_HOME") {
        if !x.is_empty() {
            return Some(PathBuf::from(x).join("agent-usage"));
        }
    }
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".cache/agent-usage"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Snapshot the cache-dir env vars, run `f` with them set as given, then restore —
    /// so these tests never leak state into each other or the rest of the suite.
    fn with_env(xdg: Option<&str>, home: Option<&str>, f: impl FnOnce()) {
        let saved = (
            std::env::var_os("XDG_CACHE_HOME"),
            std::env::var_os("HOME"),
        );
        let set = |key, val: Option<&str>| match val {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        };
        set("XDG_CACHE_HOME", xdg);
        set("HOME", home);
        f();
        match saved.0 {
            Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
            None => std::env::remove_var("XDG_CACHE_HOME"),
        }
        match saved.1 {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn xdg_takes_precedence() {
        with_env(Some("/tmp/xdg"), Some("/home/test"), || {
            assert_eq!(cache_dir(), Some(PathBuf::from("/tmp/xdg/agent-usage")));
        });
    }

    #[test]
    fn falls_back_to_home() {
        with_env(None, Some("/home/test"), || {
            assert_eq!(
                cache_dir(),
                Some(PathBuf::from("/home/test/.cache/agent-usage"))
            );
        });
    }

    #[test]
    fn empty_xdg_falls_back_to_home() {
        with_env(Some(""), Some("/home/test"), || {
            assert_eq!(
                cache_dir(),
                Some(PathBuf::from("/home/test/.cache/agent-usage"))
            );
        });
    }
}
