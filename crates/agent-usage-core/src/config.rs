//! The CLI's own configuration file.
//!
//! Most of what a provider needs it can discover: credentials sit in known paths, and windows
//! come back from the API. A few things it cannot — chiefly *when* an agent's cycle resets for an
//! agent whose API doesn't say (Hyper). Those had nowhere to live but an environment variable,
//! which means a GUI frontend holding the same setting in its own store is the only thing that
//! knows it, and the CLI run by hand silently falls back to a default that is simply wrong.
//!
//! This file is that missing baseline. It is the **lowest-precedence** source above the built-in
//! defaults: an explicit flag wins, then the environment, then this. So a frontend that passes its
//! settings as arguments keeps overriding it, exactly as before, and nothing here changes what an
//! existing invocation does.
//!
//! Location is `$XDG_CONFIG_HOME/agent-usage/config.json`, else `~/.config/agent-usage/config.json`
//! — deliberately *config*, not the cache directory, since this is authored state rather than
//! anything the tool may discard. JSON rather than TOML to keep the dependency set unchanged; it
//! is the format every other file in this project already uses.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::UsageError;

/// The base config directory: `$XDG_CONFIG_HOME/agent-usage`, falling back to
/// `~/.config/agent-usage`. `None` when neither variable is set. Pure path computation — no I/O.
pub fn config_dir() -> Option<PathBuf> {
    resolve_config_dir(
        std::env::var("XDG_CONFIG_HOME").ok(),
        std::env::var("HOME").ok(),
    )
}

/// Pure resolution from the two env values, so it can be tested without mutating process-global
/// environment state (which races under parallel test execution).
fn resolve_config_dir(xdg_config_home: Option<String>, home: Option<String>) -> Option<PathBuf> {
    if let Some(x) = xdg_config_home {
        if !x.is_empty() {
            return Some(PathBuf::from(x).join("agent-usage"));
        }
    }
    home.map(|h| PathBuf::from(h).join(".config/agent-usage"))
}

/// The config file's path, wherever it would live (whether or not it exists).
pub fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config.json"))
}

/// Every field is optional: an absent one means "no opinion, fall through to the default".
/// Field names mirror the CLI's long flags so the mapping needs no explanation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Number of budget work days per cycle, 1-7.
    pub work_days: Option<u8>,
    /// Expected usage percentage per work day.
    pub daily_budget: Option<f64>,
    /// When the daily cycle resets for agents whose API doesn't report it (`HH:MM` with an
    /// optional zone — see the Hyper provider).
    pub reset_time: Option<String>,
}

impl Config {
    /// Read the config file. A **missing** file is not an error — it yields an empty config, which
    /// is the out-of-the-box state. A file that exists but cannot be read or parsed **is** an
    /// error: silently falling back to defaults would turn a typo into wrong numbers with no
    /// symptom, which is the exact failure this file exists to prevent.
    pub fn load() -> Result<Self, UsageError> {
        match config_path() {
            Some(path) => Self::load_from(&path),
            None => Ok(Config::default()),
        }
    }

    pub fn load_from(path: &std::path::Path) -> Result<Self, UsageError> {
        let contents = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => {
                return Err(UsageError::Unsupported(format!(
                    "could not read {}: {e}",
                    path.display()
                )))
            }
        };
        Self::parse(&contents).map_err(|e| {
            UsageError::Unsupported(format!("could not parse {}: {e}", path.display()))
        })
    }

    /// Parse config JSON. Unknown keys are rejected so a misspelled field is caught here rather
    /// than being ignored and leaving the user wondering why their setting does nothing.
    fn parse(contents: &str) -> Result<Self, serde_json::Error> {
        if contents.trim().is_empty() {
            return Ok(Config::default());
        }
        serde_json::from_str(contents)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_takes_precedence() {
        assert_eq!(
            resolve_config_dir(Some("/tmp/xdg".into()), Some("/home/test".into())),
            Some(PathBuf::from("/tmp/xdg/agent-usage"))
        );
    }

    #[test]
    fn falls_back_to_home_when_xdg_unset() {
        assert_eq!(
            resolve_config_dir(None, Some("/home/test".into())),
            Some(PathBuf::from("/home/test/.config/agent-usage"))
        );
    }

    #[test]
    fn empty_xdg_falls_back_to_home() {
        assert_eq!(
            resolve_config_dir(Some(String::new()), Some("/home/test".into())),
            Some(PathBuf::from("/home/test/.config/agent-usage"))
        );
    }

    #[test]
    fn none_when_neither_is_set() {
        assert_eq!(resolve_config_dir(None, None), None);
    }

    #[test]
    fn parses_a_full_config() {
        let c = Config::parse(
            r#"{"work_days": 4, "daily_budget": 25.0, "reset_time": "08:00 local"}"#,
        )
        .unwrap();
        assert_eq!(c.work_days, Some(4));
        assert_eq!(c.daily_budget, Some(25.0));
        assert_eq!(c.reset_time.as_deref(), Some("08:00 local"));
    }

    #[test]
    fn absent_fields_mean_no_opinion() {
        let c = Config::parse(r#"{"reset_time": "08:00 local"}"#).unwrap();
        assert_eq!(c.reset_time.as_deref(), Some("08:00 local"));
        assert_eq!(c.work_days, None, "an unset field must not shadow the default");
        assert_eq!(c.daily_budget, None);
    }

    #[test]
    fn an_empty_or_bare_file_is_the_default() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
        assert_eq!(Config::parse("   \n").unwrap(), Config::default());
        assert_eq!(Config::parse("{}").unwrap(), Config::default());
    }

    /// A misspelled key must not be silently ignored — that is the failure this file prevents.
    #[test]
    fn an_unknown_key_is_rejected() {
        assert!(Config::parse(r#"{"rest_time": "08:00 local"}"#).is_err());
    }

    #[test]
    fn malformed_json_is_rejected() {
        assert!(Config::parse(r#"{"work_days": }"#).is_err());
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let path = std::env::temp_dir().join("agent-usage-no-such-config-9c1f.json");
        std::fs::remove_file(&path).ok();
        assert_eq!(Config::load_from(&path).unwrap(), Config::default());
    }

    #[test]
    fn a_present_but_broken_file_is_an_error() {
        let path = std::env::temp_dir().join(format!(
            "agent-usage-broken-config-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, "{ not json").unwrap();
        let err = Config::load_from(&path).unwrap_err();
        assert!(
            err.to_string().contains("could not parse"),
            "a broken config must surface, not silently become defaults: {err}"
        );
        std::fs::remove_file(&path).ok();
    }
}
