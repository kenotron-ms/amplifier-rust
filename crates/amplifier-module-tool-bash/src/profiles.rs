//! Safety profile hierarchy for the bash tool.
//!
//! # Profile Hierarchy
//!
//! Profiles provide layered safety controls for command execution:
//!
//! - **Android** (Phase 3 addition): Allowlist-based — only commands in the
//!   Android toybox allowlist are permitted. Designed for sandboxed Android
//!   environments using toybox utilities.
//! - **Strict**: Denylist-based — blocks `rm -rf`, `sudo`, `mount`, `umount`,
//!   `dd ` (with trailing space to avoid matching `add`), and `mkfs`.
//! - **Standard**: Denylist-based — blocks `sudo`, `mount`, `umount`.
//! - **Permissive**: Denylist-based — blocks `mount`, `umount`.
//! - **Unrestricted**: No restrictions — all commands permitted.

/// Commands available in the Android toybox utility set.
///
/// This allowlist covers the standard toybox commands present in AOSP.
/// Used by [`SafetyProfile::Android`] to gate command execution.
pub const ANDROID_TOYBOX_ALLOWLIST: &[&str] = &[
    "ls", "cat", "echo", "mkdir", "rm", "cp", "mv", "find", "grep", "sed", "awk", "sort", "head",
    "tail", "tar", "gzip", "curl", "date", "sleep", "env", "id", "pwd", "wc", "diff", "unzip",
    "zip", "chmod", "touch", "which", "dirname", "basename",
];

/// Patterns denied under the Strict profile.
const STRICT_DENY: &[&str] = &["rm -rf", "sudo", "mount", "umount", "dd ", "mkfs"];

/// Patterns denied under the Standard profile.
const STANDARD_DENY: &[&str] = &["sudo", "mount", "umount"];

/// Patterns denied under the Permissive profile.
const PERMISSIVE_DENY: &[&str] = &["mount", "umount"];

/// Safety profile controlling which shell commands are permitted.
///
/// Profiles are ordered from most restrictive to least:
/// Android (allowlist) > Strict > Standard > Permissive > Unrestricted.
#[derive(Debug, Clone, PartialEq)]
pub enum SafetyProfile {
    /// Allowlist-based profile for Android toybox environments (Phase 3 addition).
    /// Only commands in [`ANDROID_TOYBOX_ALLOWLIST`] are permitted.
    Android,
    /// Denylist: blocks `rm -rf`, `sudo`, `mount`, `umount`, `dd `, `mkfs`.
    Strict,
    /// Denylist: blocks `sudo`, `mount`, `umount`.
    Standard,
    /// Denylist: blocks `mount`, `umount`.
    Permissive,
    /// No restrictions — all commands are permitted.
    Unrestricted,
}

/// Check whether `command` is permitted under the given `profile`.
///
/// Returns `Ok(())` if the command is allowed, or `Err(reason)` if blocked.
///
/// # Profile behaviour
///
/// - [`SafetyProfile::Unrestricted`]: always `Ok`.
/// - [`SafetyProfile::Android`]: extracts the first whitespace-delimited token
///   and checks it against [`ANDROID_TOYBOX_ALLOWLIST`].
/// - All other profiles: lowercases the full command string and checks for
///   substring matches against the respective deny list.
pub fn check_command(profile: &SafetyProfile, command: &str) -> Result<(), String> {
    match profile {
        SafetyProfile::Unrestricted => Ok(()),

        SafetyProfile::Android => {
            let cmd_name = command.split_whitespace().next().unwrap_or("");
            if ANDROID_TOYBOX_ALLOWLIST.contains(&cmd_name) {
                Ok(())
            } else {
                Err(format!(
                    "Command '{}' is not in the Android toybox allowlist",
                    cmd_name
                ))
            }
        }

        SafetyProfile::Strict => check_deny_list(STRICT_DENY, command),
        SafetyProfile::Standard => check_deny_list(STANDARD_DENY, command),
        SafetyProfile::Permissive => check_deny_list(PERMISSIVE_DENY, command),
    }
}

/// Check `command` against a deny list.
///
/// Lowercases `command` and returns an error if any pattern from `deny_list`
/// appears as a substring.
fn check_deny_list(deny_list: &[&str], command: &str) -> Result<(), String> {
    let lower = command.to_lowercase();
    for pattern in deny_list {
        if lower.contains(pattern) {
            return Err(format!(
                "Command blocked by safety profile: contains '{}'",
                pattern
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_allows_known_commands() {
        for cmd in ANDROID_TOYBOX_ALLOWLIST {
            assert!(
                check_command(&SafetyProfile::Android, cmd).is_ok(),
                "Expected '{}' to be allowed",
                cmd
            );
        }
    }

    #[test]
    fn android_rejects_unknown_command() {
        let err = check_command(&SafetyProfile::Android, "python3 -c 'print(1)'").unwrap_err();
        assert!(err.contains("toybox allowlist"));
    }

    #[test]
    fn strict_blocks_rm_rf() {
        let err = check_command(&SafetyProfile::Strict, "rm -rf /tmp").unwrap_err();
        assert!(err.contains("rm -rf"));
    }

    #[test]
    fn strict_allows_echo() {
        assert!(check_command(&SafetyProfile::Strict, "echo hello").is_ok());
    }

    #[test]
    fn unrestricted_allows_everything() {
        assert!(check_command(&SafetyProfile::Unrestricted, "rm -rf /").is_ok());
    }
}
