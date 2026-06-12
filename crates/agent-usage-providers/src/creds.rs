//! Shared credential-reading helpers.
//!
//! Reading a token from a file, expanding `~`, and (on macOS) falling back to the login
//! Keychain are the same chore for every file-based agent, so they live here. Agent-specific
//! JSON shapes (which field holds the token) stay in each provider module.

use std::path::{Path, PathBuf};

use agent_usage_core::UsageError;

/// Expand a leading `~/` (or bare `~`) to `$HOME`; other paths pass through unchanged.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        if path == "~" {
            return PathBuf::from(home);
        }
        if let Some(suffix) = path.strip_prefix("~/") {
            return PathBuf::from(home).join(suffix);
        }
    }
    PathBuf::from(path)
}

/// Read a file's contents as a string, mapping IO failure onto [`UsageError::CredentialsRead`].
pub fn read_file(path: &Path) -> Result<String, UsageError> {
    std::fs::read_to_string(path)
        .map_err(|e| UsageError::CredentialsRead(format!("{}: {}", path.display(), e)))
}

/// Read a credentials value from the macOS login Keychain via the `security` tool.
///
/// Returns the raw stored string (which may be JSON or a bare token); callers parse it.
#[cfg(target_os = "macos")]
pub fn read_keychain(service: &str) -> Result<String, UsageError> {
    use std::process::Command;

    let output = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", service, "-w"])
        .output()
        .map_err(|e| UsageError::CredentialsRead(format!("could not run security: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(UsageError::CredentialsRead(format!(
            "Keychain item '{service}' not found: {}",
            stderr.trim()
        )));
    }

    let blob = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if blob.is_empty() {
        return Err(UsageError::CredentialsMissingToken);
    }
    Ok(blob)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_absolute_passthrough() {
        assert_eq!(expand_tilde("/abs/x"), PathBuf::from("/abs/x"));
    }

    #[test]
    fn tilde_expands_home() {
        std::env::set_var("HOME", "/home/test");
        assert_eq!(expand_tilde("~/x"), PathBuf::from("/home/test/x"));
        assert_eq!(expand_tilde("~"), PathBuf::from("/home/test"));
    }
}
