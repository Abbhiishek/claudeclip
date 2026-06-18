//! Background update checker and self-installer.
//!
//! A thread spawned from `app::spawn_update_checker` calls `check_for_update`
//! on startup (after a short delay) and every 24 h. When a newer release is
//! found it stashes the info in `PENDING_UPDATE` and posts `WMAPP_UPDATE_FOUND`
//! to the tray window so the UI thread can show a balloon and offer an install
//! item in the context menu.
//!
//! When the user clicks "Install update", `download_and_replace` is called on
//! a worker thread: it downloads the new exe, renames the running exe aside
//! (Windows allows renaming a live binary), puts the new one in its place, and
//! returns `Ok(())`. The caller then spawns the new instance and posts
//! `WM_DESTROY` to the hidden window so the old process exits cleanly.

use anyhow::{bail, Context, Result};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const API_URL: &str =
    "https://api.github.com/repos/Abbhiishek/claudeclip/releases/latest";
const USER_AGENT: &str = concat!("ClaudeClip/", env!("CARGO_PKG_VERSION"));

pub struct Release {
    pub version: String,      // "0.2.0" — no "v" prefix
    pub download_url: String,
}

/// Set by the update-checker thread when a newer release is found.
/// Read by the UI thread to populate the tray menu and show a balloon.
pub static PENDING_UPDATE: Mutex<Option<Release>> = Mutex::new(None);

/// True while a download is in progress, to prevent a double-click install.
pub static DOWNLOADING: AtomicBool = AtomicBool::new(false);

// ── GitHub API types ─────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct ApiRelease {
    tag_name: String,
    assets: Vec<ApiAsset>,
}

#[derive(serde::Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Check GitHub for a release newer than the running binary.
/// Returns `None` silently on any network / parse error.
pub fn check_for_update() -> Option<Release> {
    let resp = ureq::get(API_URL)
        .set("User-Agent", USER_AGENT)
        .call()
        .ok()?;
    let rel: ApiRelease = resp.into_json().ok()?;
    let version = rel.tag_name.trim_start_matches('v').to_string();
    if !is_newer(&version, CURRENT_VERSION) {
        return None;
    }
    let url = rel
        .assets
        .into_iter()
        .find(|a| a.name.ends_with(".exe"))?
        .browser_download_url;
    Some(Release { version, download_url: url })
}

/// Download `release` to a temp path, then atomically swap it in for the
/// running binary. Returns `Ok(())` on success; the caller is responsible for
/// spawning the new binary and shutting down this instance.
pub fn download_and_replace(release: &Release) -> Result<()> {
    let current_exe = std::env::current_exe().context("current_exe")?;
    let dir = current_exe.parent().context("no parent dir for exe")?;
    let temp_path = dir.join("claude-clip-update.exe");

    // Download to a temp file in the same directory so the final rename is
    // guaranteed to be on the same volume (no cross-device rename).
    {
        let resp = ureq::get(&release.download_url)
            .set("User-Agent", USER_AGENT)
            .call()
            .map_err(|e| anyhow::anyhow!("download failed: {e}"))?;
        let mut reader = resp.into_reader();
        let mut file =
            std::fs::File::create(&temp_path).context("create temp download file")?;
        std::io::copy(&mut reader, &mut file).context("write download")?;
    }

    let old_path = dir.join("claude-clip.old.exe");
    // Remove a stale .old from a previous update, if any.
    let _ = std::fs::remove_file(&old_path);
    // Rename the live binary aside — Windows allows this even while it runs.
    std::fs::rename(&current_exe, &old_path).context("rename current exe to .old")?;
    // Move the downloaded exe into the canonical path.
    if let Err(e) = std::fs::rename(&temp_path, &current_exe) {
        // Try to restore the original so we don't leave the install broken.
        let _ = std::fs::rename(&old_path, &current_exe);
        let _ = std::fs::remove_file(&temp_path);
        bail!("could not place new exe: {e}");
    }

    Ok(())
}

/// Called at startup to remove the `.old.exe` left by a completed self-update.
pub fn cleanup_old_exe() {
    if let Ok(cur) = std::env::current_exe() {
        if let Some(dir) = cur.parent() {
            let _ = std::fs::remove_file(dir.join("claude-clip.old.exe"));
        }
    }
}

// ── Version comparison ────────────────────────────────────────────────────────

fn is_newer(new: &str, current: &str) -> bool {
    match (parse_ver(new), parse_ver(current)) {
        (Some(n), Some(c)) => n > c,
        _ => false,
    }
}

fn parse_ver(s: &str) -> Option<[u32; 3]> {
    let s = s.trim_start_matches('v');
    let mut it = s.splitn(3, '.').map(|p| p.parse::<u32>().ok());
    Some([it.next()??, it.next()??, it.next()??])
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_version_detected() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
    }

    #[test]
    fn v_prefix_stripped() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(!is_newer("v0.1.0", "v0.1.0"));
    }
}
