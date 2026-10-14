//! Where profiles, write logs and the instance lock live.

use std::path::PathBuf;

/// `MAXWELL_PEQ_DATA`, else the platform's per-user data folder.
///
/// Windows keeps the Python release's `%LOCALAPPDATA%\MaxwellPEQ`, so
/// existing profiles carry over.
pub fn data_root() -> PathBuf {
    if let Some(path) = std::env::var_os("MAXWELL_PEQ_DATA").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    let home =
        || std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default();
    let nonempty = |name| std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from);
    if cfg!(windows) {
        nonempty("LOCALAPPDATA").unwrap_or_else(|| home().join("AppData").join("Local")).join("MaxwellPEQ")
    } else if cfg!(target_os = "macos") {
        home().join("Library").join("Application Support").join("MaxwellPEQ")
    } else {
        nonempty("XDG_DATA_HOME").unwrap_or_else(|| home().join(".local").join("share")).join("maxwell-peq")
    }
}

pub fn profiles_path(data: &std::path::Path) -> PathBuf {
    data.join("profiles.json")
}

/// Records of balance and source-profile writes.
pub fn logs_path(data: &std::path::Path) -> PathBuf {
    data.join("logs")
}

/// Which profile each custom preset on the headset was saved from.
pub fn links_path(data: &std::path::Path) -> PathBuf {
    data.join("links.json")
}

/// App settings, such as the shortcut keys.
pub fn settings_path(data: &std::path::Path) -> PathBuf {
    data.join("settings.json")
}

/// How to reach the running app, for `MaxwellPEQ --preset`.
pub fn remote_path(data: &std::path::Path) -> PathBuf {
    data.join("remote.json")
}
