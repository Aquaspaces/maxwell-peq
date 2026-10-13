//! Maxwell PEQ: the editor in a native window.
//!
//! The same executable is the device worker (`--worker device`, started by
//! the app for each headset operation) and, with `--serve`, a loopback server
//! for using the editor in a normal browser.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use maxwell_core::audeze::AudezeWatch;
use maxwell_core::service::Service;
use maxwell_core::{http, paths, worker};
use serde_json::Value;
use std::fs::{File, TryLockError};
use std::path::Path;
use std::sync::Arc;
use tauri_plugin_dialog::DialogExt;

const USAGE: &str = "Usage: MaxwellPEQ [--offline] [--serve [--port N]]

Parametric EQ editor for the Audeze Maxwell.

  --offline   edit and preview only; never open the headset
  --serve     serve the editor at http://127.0.0.1:PORT/ for a browser instead of opening a window
  --port N    port for --serve (default 8769; 0 picks a free one)

Profiles live in MAXWELL_PEQ_DATA if set, else the per-user data folder.";

struct Options {
    offline: bool,
    serve: bool,
    port: u16,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut options = Options { offline: false, serve: false, port: 8769 };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--offline" => options.offline = true,
            "--serve" => options.serve = true,
            "--port" => {
                options.port =
                    args.next().and_then(|port| port.parse().ok()).ok_or("--port needs a number from 0 to 65535.")?;
            }
            _ => return Err(format!("Unknown option: {arg}")),
        }
    }
    Ok(options)
}

/// Two copies would read the same HID interface and corrupt each other's
/// replies. Locks the same file as the Python release, so they exclude each other too.
fn single_instance(data: &Path) -> std::io::Result<Option<File>> {
    std::fs::create_dir_all(data)?;
    let file = File::options().create(true).append(true).read(true).open(data.join("app.lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

/// Show `text` where the user will see it: a message box in the windowed app.
fn alert(text: &str) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONWARNING, MessageBoxW};
        let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
        unsafe { MessageBoxW(std::ptr::null_mut(), wide(text).as_ptr(), wide("Maxwell PEQ").as_ptr(), MB_ICONWARNING) };
    }
    #[cfg(not(windows))]
    eprintln!("{text}");
}

/// Release builds have no console of their own; print into the one we were started from.
fn attach_console() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work).await.map_err(|error| error.to_string())?
}

#[tauri::command]
async fn get(service: tauri::State<'_, Arc<Service>>, path: String) -> Result<Value, String> {
    let service = service.inner().clone();
    blocking(move || match path.as_str() {
        "/api/status" => Ok(service.status()),
        "/api/profiles" => Ok(service.profiles()),
        _ => Err("Not found.".into()),
    })
    .await
}

#[tauri::command]
async fn post(service: tauri::State<'_, Arc<Service>>, path: String, body: Value) -> Result<Value, String> {
    let service = service.inner().clone();
    blocking(move || service.post(&path, &body).map_err(|failure| failure.message)).await
}

/// Ask where to save `text`; returns the chosen path, or `None` if cancelled.
/// WebView2 has no download UI, so exports come through here.
#[tauri::command]
async fn save_text(window: tauri::Window, filename: String, text: String) -> Result<Option<String>, String> {
    blocking(move || {
        let chosen = window
            .dialog()
            .file()
            .set_parent(&window)
            .set_file_name(&filename)
            .add_filter("Text files", &["txt"])
            .add_filter("All files", &["*"])
            .blocking_save_file();
        let Some(chosen) = chosen else { return Ok(None) };
        let path = chosen.into_path().map_err(|error| error.to_string())?;
        let text = if cfg!(windows) { text.replace("\r\n", "\n").replace('\n', "\r\n") } else { text };
        std::fs::write(&path, text).map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(Some(path.display().to_string()))
    })
    .await
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 2 && args[0] == "--worker" {
        std::process::exit(match args[1].as_str() {
            "device" => worker::device_main(),
            other => {
                eprintln!("Unknown worker: {other}");
                2
            }
        });
    }
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        attach_console();
        println!("{USAGE}");
        return;
    }
    let options = match parse(&args) {
        Ok(options) => options,
        Err(error) => {
            attach_console();
            eprintln!("{error}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    if options.serve {
        attach_console();
    }
    std::process::exit(run(&options));
}

fn run(options: &Options) -> i32 {
    let data = paths::data_root();
    let _lock = match single_instance(&data) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            alert("Maxwell PEQ is already running.");
            return 1;
        }
        Err(error) => {
            alert(&format!("Maxwell PEQ could not open its data folder: {error}\n\nData folder: {}", data.display()));
            return 1;
        }
    };
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            alert(&format!("Maxwell PEQ could not find its own executable: {error}"));
            return 1;
        }
    };
    let service = match Service::new(
        !options.offline,
        Some(data.clone()),
        Arc::new(AudezeWatch::default()),
        worker::default_runner(exe),
    ) {
        Ok(service) => Arc::new(service),
        // e.g. a damaged profiles file: keep the files and say where they are.
        Err(error) => {
            alert(&format!("Maxwell PEQ could not start: {error}\n\nData folder: {}", data.display()));
            return 1;
        }
    };

    if options.serve {
        let (server, port) = match http::bind(options.port) {
            Ok(bound) => bound,
            Err(error) => {
                eprintln!("Could not listen on 127.0.0.1:{}: {error}", options.port);
                return 1;
            }
        };
        println!("Maxwell PEQ: http://127.0.0.1:{port}/{}", if options.offline { " (offline)" } else { "" });
        http::serve(server, port, service);
        return 0;
    }

    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(service)
        .invoke_handler(tauri::generate_handler![get, post, save_text])
        .run(tauri::generate_context!());
    match result {
        Ok(()) => 0,
        Err(error) => {
            alert(&format!("Maxwell PEQ could not open its window: {error}"));
            1
        }
    }
}
