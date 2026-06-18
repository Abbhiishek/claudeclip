//! Persisted configuration, stored as TOML in `%APPDATA%\ClaudeClip\config.toml`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// How the file path is rendered as clipboard text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathFormat {
    /// `C:/Users/me/ClaudeClips/shot.png` — forward slashes, no quotes.
    Plain,
    /// `"C:\Users\me\ClaudeClips\shot.png"` — native backslashes, quoted.
    Quoted,
    /// `file:///C:/Users/me/ClaudeClips/shot.png`
    Url,
}

impl Default for PathFormat {
    fn default() -> Self {
        PathFormat::Plain
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Where captured images are written.
    pub save_dir: PathBuf,
    /// Delete our screenshots older than this many days (0 disables pruning).
    pub retention_days: u64,
    /// Clipboard text format.
    pub path_format: PathFormat,
    /// Cap the longer edge of saved images to this many pixels (0 = no resize).
    /// Smaller images cost fewer image tokens when sent to Claude.
    pub max_dimension: u32,
    /// Keep the original image on the clipboard alongside the path text.
    pub keep_image: bool,
    /// Show a tray balloon when a capture happens.
    pub notify_on_capture: bool,
    /// Also convert copied files (videos, existing images, etc.) to a text path.
    pub handle_files: bool,
    /// Separator used when multiple files are copied at once.
    pub multi_file_separator: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            save_dir: default_save_dir(),
            retention_days: 7,
            path_format: PathFormat::Plain,
            max_dimension: 0,
            keep_image: true,
            notify_on_capture: true,
            handle_files: true,
            multi_file_separator: "\n".to_string(),
        }
    }
}

impl Config {
    /// Load the config from `path`, creating it with defaults if it does not exist.
    pub fn load_or_create(path: &Path) -> Result<Config> {
        if path.exists() {
            let raw = std::fs::read_to_string(path)
                .with_context(|| format!("reading config {}", path.display()))?;
            let cfg: Config = toml::from_str(&raw)
                .with_context(|| format!("parsing config {}", path.display()))?;
            Ok(cfg)
        } else {
            let cfg = Config::default();
            cfg.save(path)?;
            Ok(cfg)
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(path, text)
            .with_context(|| format!("writing config {}", path.display()))?;
        Ok(())
    }
}

fn default_save_dir() -> PathBuf {
    // Local app-data (not OneDrive-synced) — these captures are transient and
    // pruned after `retention_days`, so syncing them to the cloud is wasteful.
    // Override `save_dir` in config.toml to use Pictures or anywhere else.
    dirs::data_local_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ClaudeClip")
        .join("captures")
}
