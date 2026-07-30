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
///
/// `skip_serializing_if` on every field keeps a saved file to just the keys that carry an
/// opinion, so a hand-edited config isn't cluttered with nulls.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Number of budget work days per cycle, 1-7.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_days: Option<u8>,
    /// Expected usage percentage per work day.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daily_budget: Option<f64>,
    /// When the daily cycle resets for agents whose API doesn't report it (`HH:MM` with an
    /// optional zone — see the Hyper provider).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_time: Option<String>,
    /// Size of the credit pool for agents whose API reports only a balance (currently Hyper):
    /// permanent credits plus the recurring daily grant.
    ///
    /// The provider otherwise infers this from successive balances, which is an inference from a
    /// single number and can drift — chiefly because a balance never says how much of it is the
    /// day's grant. Setting it here states the ceiling outright. Absent means "keep inferring".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_credits: Option<u32>,
    /// Charm Hyper API key.
    ///
    /// Hyper is opt-in on the *presence* of a key, and until now the only place to put one was
    /// `HYPER_API_KEY`. An app launched by launchd at login inherits no shell profile, so the
    /// agent silently vanished from the menu bar after every reboot. Storing it here makes that
    /// survive a restart — and the file is written `0600`, which is stricter than the
    /// world-readable `~/.env` an export typically lives in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hyper_api_key: Option<String>,
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

    /// Overlay `patch` onto this config: `Some` replaces, `None` leaves the current value alone.
    ///
    /// Merging rather than replacing is what lets a frontend save the one setting it owns without
    /// erasing keys a user hand-wrote. To *clear* a field, see [`Patch`]'s `clear_*` flags — an
    /// absent value has to mean "no opinion", so it cannot also mean "remove".
    pub fn merge(&mut self, patch: Patch) {
        if patch.clear_work_days {
            self.work_days = None;
        } else if patch.work_days.is_some() {
            self.work_days = patch.work_days;
        }
        if patch.clear_daily_budget {
            self.daily_budget = None;
        } else if patch.daily_budget.is_some() {
            self.daily_budget = patch.daily_budget;
        }
        if patch.clear_reset_time {
            self.reset_time = None;
        } else if patch.reset_time.is_some() {
            self.reset_time = patch.reset_time;
        }
        if patch.clear_total_credits {
            self.total_credits = None;
        } else if patch.total_credits.is_some() {
            self.total_credits = patch.total_credits;
        }
        if patch.clear_hyper_api_key {
            self.hyper_api_key = None;
        } else if patch.hyper_api_key.is_some() {
            self.hyper_api_key = patch.hyper_api_key;
        }
    }

    /// Write to the default config path, creating the directory as needed.
    pub fn save(&self) -> Result<PathBuf, UsageError> {
        let path = config_path().ok_or_else(|| {
            UsageError::Unsupported(
                "cannot locate a config directory (neither XDG_CONFIG_HOME nor HOME is set)".into(),
            )
        })?;
        self.save_to(&path)?;
        Ok(path)
    }

    /// Write to `path` with owner-only permissions.
    ///
    /// The mode is set on the file itself rather than left to the umask because this may hold an
    /// API key: a default umask yields a world-readable file, which is how the same secret ends up
    /// exposed in a shell profile. Written to a temporary file and renamed so a crash mid-write
    /// cannot leave a truncated config that the next run would reject.
    pub fn save_to(&self, path: &std::path::Path) -> Result<(), UsageError> {
        let io_err = |e: std::io::Error| {
            UsageError::Unsupported(format!("could not write {}: {e}", path.display()))
        };

        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io_err)?;
        }
        let mut body = serde_json::to_string_pretty(self)
            .map_err(|e| UsageError::Unsupported(format!("could not serialize config: {e}")))?;
        body.push('\n');

        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &body).map_err(io_err)?;
        restrict_permissions(&tmp)?;
        std::fs::rename(&tmp, path).map_err(io_err)?;
        Ok(())
    }
}

/// Owner-only (`0600`) on Unix; a no-op elsewhere.
#[cfg(unix)]
fn restrict_permissions(path: &std::path::Path) -> Result<(), UsageError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| {
        UsageError::Unsupported(format!("could not secure {}: {e}", path.display()))
    })
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &std::path::Path) -> Result<(), UsageError> {
    Ok(())
}

/// A partial update to a [`Config`]. `Some` sets a field, `None` leaves it untouched, and the
/// matching `clear_*` flag removes it — three states that a bare `Option` cannot express.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Patch {
    pub work_days: Option<u8>,
    pub clear_work_days: bool,
    pub daily_budget: Option<f64>,
    pub clear_daily_budget: bool,
    pub reset_time: Option<String>,
    pub clear_reset_time: bool,
    pub total_credits: Option<u32>,
    pub clear_total_credits: bool,
    pub hyper_api_key: Option<String>,
    pub clear_hyper_api_key: bool,
}

impl Patch {
    /// True when this patch would change nothing — lets a caller skip a pointless write.
    pub fn is_empty(&self) -> bool {
        *self == Patch::default()
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
            r#"{"work_days": 4, "daily_budget": 25.0, "reset_time": "08:00 local",
                "total_credits": 1527}"#,
        )
        .unwrap();
        assert_eq!(c.work_days, Some(4));
        assert_eq!(c.daily_budget, Some(25.0));
        assert_eq!(c.reset_time.as_deref(), Some("08:00 local"));
        assert_eq!(c.total_credits, Some(1527));
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

    fn tmp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agent-usage-{tag}-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    /// Merging, not replacing: a frontend saving the one setting it owns must not erase keys a
    /// user hand-wrote into the same file.
    #[test]
    fn a_patch_only_touches_the_fields_it_sets() {
        let mut c = Config {
            work_days: Some(5),
            daily_budget: Some(20.0),
            reset_time: Some("20:18".into()),
            total_credits: Some(1600),
            hyper_api_key: Some("secret".into()),
        };
        c.merge(Patch {
            work_days: Some(4),
            ..Default::default()
        });
        assert_eq!(c.work_days, Some(4));
        assert_eq!(c.daily_budget, Some(20.0));
        assert_eq!(c.reset_time.as_deref(), Some("20:18"));
        assert_eq!(c.total_credits, Some(1600));
        assert_eq!(c.hyper_api_key.as_deref(), Some("secret"));
    }

    /// An absent value means "leave alone", so removing needs its own signal.
    #[test]
    fn a_clear_flag_removes_a_field() {
        let mut c = Config {
            reset_time: Some("20:18".into()),
            total_credits: Some(1600),
            hyper_api_key: Some("secret".into()),
            ..Default::default()
        };
        c.merge(Patch {
            clear_reset_time: true,
            ..Default::default()
        });
        assert_eq!(c.reset_time, None);
        assert_eq!(c.hyper_api_key.as_deref(), Some("secret"), "unrelated field survives");

        c.merge(Patch {
            clear_total_credits: true,
            ..Default::default()
        });
        assert_eq!(c.total_credits, None);

        c.merge(Patch {
            clear_hyper_api_key: true,
            ..Default::default()
        });
        assert_eq!(c.hyper_api_key, None);
    }

    #[test]
    fn an_empty_patch_changes_nothing() {
        let original = Config {
            work_days: Some(4),
            reset_time: Some("08:00 local".into()),
            ..Default::default()
        };
        let mut c = original.clone();
        c.merge(Patch::default());
        assert_eq!(c, original);
        assert!(Patch::default().is_empty());
    }

    #[test]
    fn a_saved_config_round_trips() {
        let path = tmp_path("roundtrip");
        let c = Config {
            work_days: Some(4),
            daily_budget: None,
            reset_time: Some("08:00 local".into()),
            total_credits: Some(1527),
            hyper_api_key: Some("secret".into()),
        };
        c.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path).unwrap(), c);

        // Unset fields are omitted rather than written as nulls.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("daily_budget"), "got {raw}");
        std::fs::remove_file(&path).ok();
    }

    /// The file may hold an API key, so it must not be left at the umask's mercy — that is how
    /// the same secret ends up world-readable in a shell profile.
    #[cfg(unix)]
    #[test]
    fn a_saved_config_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let path = tmp_path("perms");
        Config {
            hyper_api_key: Some("secret".into()),
            ..Default::default()
        }
        .save_to(&path)
        .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "got {mode:o}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn saving_leaves_no_temporary_file_behind() {
        let path = tmp_path("tmpfile");
        Config {
            work_days: Some(3),
            ..Default::default()
        }
        .save_to(&path)
        .unwrap();
        assert!(!path.with_extension("json.tmp").exists());
        std::fs::remove_file(&path).ok();
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
