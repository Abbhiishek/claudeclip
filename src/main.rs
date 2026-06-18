//! ClaudeClip — a Windows tray daemon that watches the clipboard for screenshots
//! and images, saves them to a folder, and augments the clipboard with the file
//! path so it can be pasted straight into Claude Code.
//!
//! Subcommands:
//!   (none)        run the tray app
//!   --install     copy to %LOCALAPPDATA%\ClaudeClip, enable autostart, launch
//!   --uninstall   disable autostart and remove the installed copy
//!   --help        show usage
//!
//! Release builds use the `windows` subsystem (no console window). Debug builds
//! keep the console so logs are visible during development.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod autostart;
mod capture;
mod config;
mod installer;
mod util;

use anyhow::{anyhow, Result};
use config::Config;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use util::wide;
use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK,
};

const HELP_TEXT: &str = "ClaudeClip — clipboard screenshots → pasteable paths\n\n\
Usage:\n\
  claude-clip              Run in the system tray\n\
  claude-clip --install    Install to %LOCALAPPDATA%, start on login, and run\n\
  claude-clip --uninstall  Remove autostart and the installed copy\n\
  claude-clip --help       Show this message\n\n\
Flags:\n\
  --silent                 Suppress dialogs (for --install / --uninstall)\n\n\
While running, right-click the tray icon for options.";

fn has_flag(name: &str) -> bool {
    std::env::args().skip(1).any(|a| a == name)
}

enum Mode {
    Run,
    Install,
    Uninstall,
    Help,
}

fn main() {
    if let Err(e) = run() {
        log::error!("fatal: {e:#}");
        message_box("ClaudeClip — error", &format!("{e:#}"), MB_ICONERROR);
        std::process::exit(1);
    }
}

fn parse_mode() -> Mode {
    match std::env::args().nth(1).as_deref() {
        Some("--install") | Some("/install") | Some("install") => Mode::Install,
        Some("--uninstall") | Some("/uninstall") | Some("uninstall") => Mode::Uninstall,
        Some("--help") | Some("-h") | Some("/?") | Some("/help") => Mode::Help,
        _ => Mode::Run,
    }
}

fn run() -> Result<()> {
    let cfg_dir = dirs::config_dir()
        .ok_or_else(|| anyhow!("could not resolve %APPDATA%"))?
        .join("ClaudeClip");
    std::fs::create_dir_all(&cfg_dir)?;

    let config_path = cfg_dir.join("config.toml");
    let log_path = cfg_dir.join("claude-clip.log");
    init_logging(&log_path)?;

    let silent = has_flag("--silent") || has_flag("/silent");

    match parse_mode() {
        Mode::Help => {
            message_box("ClaudeClip", HELP_TEXT, MB_ICONINFORMATION);
            return Ok(());
        }
        Mode::Install => {
            log::info!("installing…");
            let target = installer::install()?;
            // Launch the installed copy now (it will grab the single-instance lock).
            let _ = std::process::Command::new(&target).spawn();
            if !silent {
                message_box(
                    "ClaudeClip installed",
                    &format!(
                        "Installed to:\n{}\n\nIt's running in your system tray now and \
                         will start automatically when you log in.\n\nTake a screenshot \
                         (Win+Shift+S), then paste into Claude Code.",
                        target.display()
                    ),
                    MB_ICONINFORMATION,
                );
            }
            return Ok(());
        }
        Mode::Uninstall => {
            log::info!("uninstalling…");
            installer::uninstall()?;
            if !silent {
                message_box(
                    "ClaudeClip uninstalled",
                    "Launch-on-login removed and the installed copy deleted.\n\nYour \
                     config and saved screenshots were left in place.",
                    MB_ICONINFORMATION,
                );
            }
            return Ok(());
        }
        Mode::Run => {}
    }

    log::info!("=== ClaudeClip starting (v{}) ===", env!("CARGO_PKG_VERSION"));

    // Single instance: a named mutex. If it already exists, another copy is running.
    unsafe {
        let name = wide("ClaudeClip_SingleInstance_Mutex_v1");
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if !handle.is_null() && GetLastError() == ERROR_ALREADY_EXISTS {
            log::warn!("another instance is already running; exiting");
            return Ok(());
        }
        // Intentionally never closed: the lock lives for the whole process.
    }

    let config = Config::load_or_create(&config_path)?;
    std::fs::create_dir_all(&config.save_dir)?;
    log::info!("config: {config:?}");

    spawn_cleanup(config.save_dir.clone(), config.retention_days);

    app::run_app(config, config_path, log_path)?;
    log::info!("=== ClaudeClip stopped ===");
    Ok(())
}

fn init_logging(log_path: &Path) -> Result<()> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    let _ = simplelog::WriteLogger::init(
        log::LevelFilter::Info,
        simplelog::Config::default(),
        file,
    );
    Ok(())
}

/// Background thread: delete our own screenshots older than `retention_days`.
fn spawn_cleanup(save_dir: PathBuf, retention_days: u64) {
    if retention_days == 0 {
        return;
    }
    std::thread::spawn(move || loop {
        if let Err(e) = prune(&save_dir, retention_days) {
            log::warn!("retention sweep failed: {e:#}");
        }
        std::thread::sleep(Duration::from_secs(3600));
    });
}

fn prune(dir: &Path, retention_days: u64) -> Result<()> {
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(retention_days * 86_400))
        .ok_or_else(|| anyhow!("retention window overflow"))?;

    for entry in std::fs::read_dir(dir)? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        // Only ever touch files we created.
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if !(name.starts_with("screenshot_") && name.ends_with(".png")) {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            if let Ok(modified) = meta.modified() {
                if modified < cutoff && std::fs::remove_file(&path).is_ok() {
                    log::info!("pruned old capture: {name}");
                }
            }
        }
    }
    Ok(())
}

fn message_box(title: &str, msg: &str, icon: u32) {
    let text = wide(msg);
    let title = wide(title);
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | icon,
        );
    }
}
