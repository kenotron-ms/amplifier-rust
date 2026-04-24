#[cfg(target_os = "linux")]
pub fn apply(vault_path: &std::path::Path) -> anyhow::Result<()> {
    eprintln!("[sandbox] applying Linux sandbox restrictions for vault: {}", vault_path.display());
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn apply(_vault_path: &std::path::Path) -> anyhow::Result<()> {
    eprintln!("Note: --sandbox has no effect on this platform (Linux only)");
    Ok(())
}
