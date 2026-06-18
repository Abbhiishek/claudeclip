//! Self-contained install / uninstall: copy the executable to a stable location
//! under `%LOCALAPPDATA%\ClaudeClip`, and register/remove launch-on-login.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// `%LOCALAPPDATA%\ClaudeClip`
pub fn install_dir() -> Result<PathBuf> {
    Ok(dirs::data_local_dir()
        .context("could not resolve %LOCALAPPDATA%")?
        .join("ClaudeClip"))
}

/// The stable executable location we install to / launch from autostart.
pub fn installed_exe() -> Result<PathBuf> {
    Ok(install_dir()?.join("claude-clip.exe"))
}

/// Copy this executable into the install dir (if not already there) and enable
/// launch-on-login pointing at the installed copy. Returns the installed path.
pub fn install() -> Result<PathBuf> {
    let dir = install_dir()?;
    std::fs::create_dir_all(&dir)?;
    let target = installed_exe()?;
    let current = std::env::current_exe()?;

    if !same_file(&current, &target) {
        stop_other_instances();

        // Clean a stale .old from a previous update.
        let old = target.with_extension("old");
        let _ = std::fs::remove_file(&old);

        // A running .exe can't be overwritten or deleted, but it CAN be renamed,
        // so rename any existing copy out of the way before writing the new one.
        if target.exists() {
            let _ = std::fs::rename(&target, &old);
        }
        std::fs::copy(&current, &target)
            .with_context(|| format!("copying executable to {}", target.display()))?;
    }

    crate::autostart::set_enabled_for(&target, true)?;
    Ok(target)
}

/// Remove launch-on-login and the installed executable (best effort).
pub fn uninstall() -> Result<()> {
    crate::autostart::set_enabled(false)?;
    stop_other_instances();

    let target = installed_exe()?;
    let current = std::env::current_exe().unwrap_or_default();
    // We can't delete the exe we're currently running from.
    if !same_file(&current, &target) {
        let _ = std::fs::remove_file(&target);
        let _ = std::fs::remove_file(target.with_extension("old"));
    }
    Ok(())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// Terminate any other running ClaudeClip processes so we can replace the binary
/// and so the freshly launched copy can acquire the single-instance lock.
fn stop_other_instances() {
    let me = std::process::id();
    let cmd = format!(
        "Get-Process claude-clip -ErrorAction SilentlyContinue | \
         Where-Object {{ $_.Id -ne {me} }} | Stop-Process -Force"
    );
    let _ = std::process::Command::new("powershell")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &cmd])
        .status();
}
