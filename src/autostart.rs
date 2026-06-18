//! Launch-on-login via the HKCU `Run` registry key.

use anyhow::Result;
use std::path::Path;
use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "ClaudeClip";

/// True if the `Run` value exists and points at the current executable.
pub fn is_enabled() -> bool {
    let exe = match std::env::current_exe() {
        Ok(p) => p.to_string_lossy().to_lowercase(),
        Err(_) => return false,
    };
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = hkcu.open_subkey(RUN_KEY) {
        if let Ok(val) = key.get_value::<String, _>(VALUE_NAME) {
            return val.to_lowercase().contains(&exe);
        }
    }
    false
}

/// Add or remove the `Run` entry pointing at the current executable.
pub fn set_enabled(enable: bool) -> Result<()> {
    let exe = std::env::current_exe()?;
    set_enabled_for(&exe, enable)
}

/// Add or remove the `Run` entry pointing at a specific executable path.
pub fn set_enabled_for(exe: &Path, enable: bool) -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(RUN_KEY)?;
    if enable {
        // Quote the path so spaces survive.
        let value = format!("\"{}\"", exe.display());
        key.set_value(VALUE_NAME, &value)?;
    } else {
        let _ = key.delete_value(VALUE_NAME);
    }
    Ok(())
}
