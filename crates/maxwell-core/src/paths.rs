//! Where profiles, backups and the instance lock live.

use std::path::PathBuf;

/// `MAXWELL_PEQ_DATA`, else the platform's per-user data folder.
///
/// Windows keeps the Python release's `%LOCALAPPDATA%\MaxwellPEQ`, so
/// existing profiles and recovery sessions carry over.
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

pub fn session_path(data: &std::path::Path) -> PathBuf {
    data.join("backups").join("session.json")
}
