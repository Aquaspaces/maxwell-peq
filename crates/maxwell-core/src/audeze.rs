//! Detecting and closing the Audeze app, which competes for the headset's
//! control interface and overwrites custom EQ.

use std::sync::Mutex;
use std::time::{Duration, Instant};

#[cfg(windows)]
pub const PROCESSES: [&str; 3] = ["Audeze.exe", "Audeze HQ.exe", "AudezeHQ.exe"];
/// Executable names inside the app bundles. Not yet checked against a Mac install.
#[cfg(not(windows))]
pub const PROCESSES: [&str; 3] = ["Audeze", "Audeze HQ", "AudezeHQ"];

/// Running Audeze apps by canonical name, or `None` where the check is unsupported.
pub type Check = Box<dyn Fn() -> Option<Vec<String>> + Send + Sync>;

/// Canonical names of the Audeze apps among `images` (case-insensitive).
pub fn matching<'a>(images: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let images: Vec<String> = images.into_iter().map(str::to_lowercase).collect();
    PROCESSES.iter().filter(|name| images.contains(&name.to_lowercase())).map(|name| name.to_string()).collect()
}

#[cfg(windows)]
pub fn running() -> Option<Vec<String>> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut images = Vec::new();
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while more {
        let name = &entry.szExeFile;
        images.push(String::from_utf16_lossy(&name[..name.iter().position(|&u| u == 0).unwrap_or(name.len())]));
        more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    Some(matching(images.iter().map(String::as_str)))
}

#[cfg(not(windows))]
pub fn running() -> Option<Vec<String>> {
    // macOS prints the full executable path for `comm`; Linux prints the name.
    let output = std::process::Command::new("ps").args(["-A", "-o", "comm="]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Some(matching(text.lines().map(|line| line.trim().rsplit('/').next().unwrap_or_default())))
}

/// Ask Audeze apps to exit, then force any that remain (tray processes ignore
/// WM_CLOSE). Returns the names still running.
pub fn close() -> Vec<String> {
    let mut still = running().unwrap_or_default();
    if still.is_empty() {
        return still;
    }
    for force in [false, true] {
        for name in &still {
            #[cfg(windows)]
            let mut command = {
                use std::os::windows::process::CommandExt;
                let mut command = std::process::Command::new("taskkill");
                command.args(["/IM", name, "/T"]).creation_flags(0x0800_0000);
                if force {
                    command.arg("/F");
                }
                command
            };
            #[cfg(not(windows))]
            let mut command = {
                let mut command = std::process::Command::new("pkill");
                if force {
                    command.arg("-9");
                }
                command.args(["-x", name]);
                command
            };
            let _ = command.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while !still.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(250));
            still = running().unwrap_or_default();
        }
        if still.is_empty() {
            break;
        }
    }
    still
}

/// Status polls share one recent answer.
pub struct AudezeWatch {
    check: Check,
    ttl: Duration,
    cache: Mutex<(Option<Vec<String>>, Option<Instant>)>,
}

impl Default for AudezeWatch {
    fn default() -> Self {
        AudezeWatch::new(Box::new(running), Duration::from_millis(1500))
    }
}

impl AudezeWatch {
    pub fn new(check: Check, ttl: Duration) -> Self {
        AudezeWatch { check, ttl, cache: Mutex::new((None, None)) }
    }

    pub fn running(&self, fresh: bool) -> Option<Vec<String>> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if fresh || cache.1.is_none_or(|when| when.elapsed() > self.ttl) {
            *cache = ((self.check)(), Some(Instant::now()));
        }
        cache.0.clone()
    }

    pub fn reset(&self) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).1 = None;
    }
}
