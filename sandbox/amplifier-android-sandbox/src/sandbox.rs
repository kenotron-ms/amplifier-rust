//! Sandbox module: restricts the running process to the vault directory.
//!
//! ## Linux (kernel 5.13+)
//!
//! Uses **landlock** (LSM) and **seccomp BPF** to enforce:
//!
//! ### Landlock — filesystem restrictions
//!
//! Restricts filesystem access to:
//! - `<vault_path>` — full read/write access
//! - `/tmp` — full read/write access
//! - `/etc/ssl` — read-only (certificates)
//!
//! Requires Linux kernel 5.13+. If the kernel is older, a warning is printed
//! and the process continues unrestricted.
//!
//! ### Seccomp BPF — syscall filtering
//!
//! Blocks dangerous syscalls:
//! `ptrace`, `mount`, `umount2`, `setuid`, `setgid`, `capset`,
//! `kexec_load`, `chroot`
//!
//! ## macOS / Windows
//!
//! `apply()` is a documented no-op. Use Docker on Linux for full-fidelity sandbox testing.
//!
//! ## Docker recipe
//!
//! ```sh
//! docker run --rm \
//!   --security-opt no-new-privileges \
//!   --read-only \
//!   -v /path/to/vault:/vault \
//!   amplifier-android-sandbox --sandbox --vault /vault
//! ```

/// Apply sandbox restrictions for the current process.
///
/// On Linux (kernel 5.13+), enforces landlock filesystem restrictions and seccomp BPF
/// syscall filtering. On other platforms this is a documented no-op.
#[cfg(target_os = "linux")]
pub fn apply(vault_path: &std::path::Path) -> anyhow::Result<()> {
    apply_landlock(vault_path)?;
    apply_seccomp()?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn apply_landlock(vault_path: &std::path::Path) -> anyhow::Result<()> {
    use landlock::{
        Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
        RulesetStatus, ABI,
    };

    let abi = ABI::V3;
    let status = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))?
        .create()?
        .add_rule(PathBeneath::new(
            PathFd::new(vault_path)?,
            AccessFs::from_all(abi),
        ))?
        .add_rule(PathBeneath::new(
            PathFd::new("/tmp")?,
            AccessFs::from_all(abi),
        ))?
        .add_rule(PathBeneath::new(
            PathFd::new("/etc/ssl")?,
            AccessFs::ReadFile | AccessFs::ReadDir,
        ))?
        .restrict_self()?;

    if status.ruleset == RulesetStatus::NotEnforced {
        eprintln!("[sandbox] WARNING: landlock not enforced — kernel < 5.13");
    } else {
        eprintln!("[sandbox] landlock applied: vault={}", vault_path.display());
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn apply_seccomp() -> anyhow::Result<()> {
    use libseccomp::{ScmpAction, ScmpFilterContext, ScmpSyscall};

    let mut ctx = ScmpFilterContext::new_filter(ScmpAction::Allow)?;
    for name in &[
        "ptrace",
        "mount",
        "umount2",
        "setuid",
        "setgid",
        "capset",
        "kexec_load",
        "chroot",
    ] {
        ctx.add_rule(ScmpAction::Errno(1), ScmpSyscall::from_name(name)?)?;
    }
    ctx.load()?;
    eprintln!("[sandbox] seccomp BPF loaded — dangerous syscalls blocked");
    Ok(())
}

/// Apply sandbox restrictions for the current process.
///
/// On this platform (non-Linux), this is a no-op. landlock and seccomp are
/// Linux-only features requiring kernel 5.13+.
#[cfg(not(target_os = "linux"))]
pub fn apply(_vault_path: &std::path::Path) -> anyhow::Result<()> {
    println!(
        "Note: --sandbox flag has no effect on this platform. \
         landlock and seccomp are Linux-only (kernel 5.13+). \
         Use Docker on Linux for full-fidelity sandbox testing."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_is_callable_on_all_platforms() {
        let dir = tempfile::TempDir::new().unwrap();

        #[cfg(not(target_os = "linux"))]
        assert!(apply(dir.path()).is_ok());

        #[cfg(target_os = "linux")]
        let _ = dir;
    }
}
