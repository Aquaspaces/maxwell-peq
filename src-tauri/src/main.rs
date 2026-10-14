//! Maxwell PEQ: the editor in a native window, and a tray icon with the
//! headset's battery, mic and EQ.
//!
//! The same executable is the device worker (`--worker device`, started by
//! the app for each headset operation), a command line for switching EQ
//! (`--preset`, `--status`), and with `--serve`, a loopback server for using
//! the editor in a normal browser.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use maxwell_core::audeze::AudezeWatch;
use maxwell_core::service::Service;
use maxwell_core::{http, paths, remote, worker};
use serde_json::{Value, json};
use std::fs::{File, TryLockError};
use std::path::Path;
use std::sync::Arc;
use tauri::{Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_dialog::DialogExt;

mod glance;
mod preferences;
mod shortcuts;
mod system;
mod tray;
mod updates;

const USAGE: &str = "Usage: MaxwellPEQ [--offline] [--tray] [--serve [--port N]]
       MaxwellPEQ --preset <1-4|next|previous>
       MaxwellPEQ --status

Parametric EQ editor for the Audeze Maxwell.

  --offline   edit and preview only; never open the headset
  --tray      start in the tray without opening the editor
  --serve     serve the editor at http://127.0.0.1:PORT/ for a browser instead of opening a window
  --port N    port for --serve (default 8769; 0 picks a free one)
  --preset P  switch the headset to custom preset P (EQ 1-4), or the next or previous one
  --status    print the battery, mic and preset as JSON

--preset and --status go through the running app when there is one.

Profiles live in MAXWELL_PEQ_DATA if set, else the per-user data folder.";

struct Options {
    offline: bool,
    serve: bool,
    tray: bool,
    port: u16,
    /// `--preset` or `--status`: run one command and exit.
    command: Option<Value>,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut options = Options { offline: false, serve: false, tray: false, port: 8769, command: None };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--offline" => options.offline = true,
            "--serve" => options.serve = true,
            "--tray" => options.tray = true,
            "--port" => {
                options.port =
                    args.next().and_then(|port| port.parse().ok()).ok_or("--port needs a number from 0 to 65535.")?;
            }
            "--preset" => {
                let value = args.next().ok_or("--preset needs 1, 2, 3, 4, next or previous.")?;
                options.command = Some(json!({"command": "preset", "value": value}));
            }
            "--status" => options.command = Some(json!({"command": "status"})),
            _ => return Err(format!("Unknown option: {arg}")),
        }
    }
    if options.command.is_some() && (options.serve || options.tray) {
        return Err("--preset and --status can't be combined with --serve or --tray.".into());
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

fn service(app: &tauri::AppHandle) -> Arc<Service> {
    app.state::<Arc<Service>>().inner().clone()
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
async fn post(app: tauri::AppHandle, path: String, body: Value) -> Result<Value, String> {
    let service = service(&app);
    let result = blocking(move || service.post(&path, &body).map_err(|failure| failure.message)).await;
    tray::update(&app);
    result
}

/// The flyout's state; with `refresh`, read the headset first.
#[tauri::command]
async fn tray_status(app: tauri::AppHandle, refresh: bool) -> Result<Value, String> {
    blocking(move || {
        if refresh {
            // A failed read shows in the state.
            let _ = service(&app).post_wait("/api/glance", &json!({}));
        }
        tray::update_now(&app);
        Ok(tray::flyout_state(&app))
    })
    .await
}

/// Switch EQ from the flyout; returns the flyout's new state.
#[tauri::command]
async fn tray_preset(app: tauri::AppHandle, value: Value) -> Result<Value, String> {
    blocking(move || {
        let result = service(&app).command(&json!({"command": "preset", "value": value}));
        tray::update_now(&app);
        result.map(|_| tray::flyout_state(&app)).map_err(|failure| failure.message)
    })
    .await
}

#[tauri::command]
fn flyout_hide(app: tauri::AppHandle) {
    tray::hide_flyout(&app);
}

#[tauri::command]
fn flyout_open_editor(app: tauri::AppHandle) {
    tray::hide_flyout(&app);
    open_editor(&app, None);
}

#[tauri::command]
fn shortcuts_get(app: tauri::AppHandle) -> Value {
    shortcuts::listing(&app)
}

#[tauri::command]
fn shortcuts_pause(app: tauri::AppHandle, paused: bool) {
    shortcuts::pause(&app, paused);
}

#[tauri::command]
async fn shortcuts_set(app: tauri::AppHandle, shortcuts: Value) -> Result<Value, String> {
    blocking(move || shortcuts::save(&app, &shortcuts)).await
}

#[tauri::command]
fn settings_get(app: tauri::AppHandle) -> Value {
    preferences::listing(&app)
}

#[tauri::command]
async fn settings_set(app: tauri::AppHandle, changes: Value) -> Result<Value, String> {
    blocking(move || preferences::set(&app, &changes)).await
}

/// The last update check, or with `now`, a new one.
#[tauri::command]
async fn updates_get(app: tauri::AppHandle, now: bool) -> Result<Value, String> {
    blocking(move || Ok(if now { updates::check(&app) } else { updates::last(&app) })).await
}

#[tauri::command]
fn open_release(app: tauri::AppHandle, url: String) -> Result<(), String> {
    updates::open(&app, &url)
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

/// Show the editor, opening its window if it's closed; with `dialog`
/// (`shortcuts` or `settings`), open that dialog too.
pub fn open_editor(app: &tauri::AppHandle, dialog: Option<&str>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        if let Some(dialog) = dialog {
            let _ = window.emit("open-dialog", dialog);
        }
        return;
    }
    let Some(mut config) = app.config().app.windows.iter().find(|window| window.label == "main").cloned() else {
        return;
    };
    if let Some(dialog) = dialog {
        config.url = WebviewUrl::App(format!("index.html#{dialog}").into());
    }
    match WebviewWindowBuilder::from_config(app, &config).and_then(|builder| builder.build()) {
        Ok(window) => {
            let app = app.clone();
            let editor = window.clone();
            window.on_window_event(move |event| match event {
                WindowEvent::Resized(_) if preferences::minimize_to_tray(&app) => {
                    if editor.is_minimized().unwrap_or(false) {
                        let _ = editor.hide();
                    }
                }
                WindowEvent::Destroyed => tray::editor_closed(&app),
                _ => {}
            });
        }
        Err(error) => alert(&format!("Maxwell PEQ could not open its window: {error}")),
    }
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
    if options.serve || options.command.is_some() {
        attach_console();
    }
    if let Some(command) = &options.command {
        std::process::exit(run_command(&options, command));
    }
    std::process::exit(run(&options));
}

fn start_service(offline: bool, data: &Path) -> Result<Service, String> {
    let exe =
        std::env::current_exe().map_err(|error| format!("Maxwell PEQ could not find its own executable: {error}"))?;
    // e.g. a damaged profiles file: keep the files and say where they are.
    Service::new(!offline, Some(data.to_owned()), Arc::new(AudezeWatch::default()), worker::default_runner(exe))
        .map_err(|error| format!("Maxwell PEQ could not start: {error}\n\nData folder: {}", data.display()))
}

/// `--preset` or `--status`: through the running app if there is one, else here.
fn run_command(options: &Options, command: &Value) -> i32 {
    let data = paths::data_root();
    let result = match single_instance(&data) {
        Ok(Some(_lock)) => start_service(options.offline, &data)
            .and_then(|service| service.command(command).map_err(|failure| failure.message)),
        Ok(None) => remote::send(&data, command).map_err(|error| error.to_string()),
        Err(error) => Err(format!("Maxwell PEQ could not open its data folder: {error}")),
    };
    match result {
        Ok(reply) => {
            match reply.get("message").and_then(Value::as_str) {
                Some(message) if command["command"] == "preset" => println!("{message}"),
                _ => println!("{}", serde_json::to_string_pretty(&reply).unwrap_or_default()),
            }
            0
        }
        Err(message) => {
            eprintln!("{message}");
            1
        }
    }
}

fn run(options: &Options) -> i32 {
    let data = paths::data_root();
    let _lock = match single_instance(&data) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            // Bring the running copy's editor forward instead.
            if options.serve || remote::send(&data, &json!({"command": "show"})).is_err() {
                alert("Maxwell PEQ is already running.");
                return 1;
            }
            return 0;
        }
        Err(error) => {
            alert(&format!("Maxwell PEQ could not open its data folder: {error}\n\nData folder: {}", data.display()));
            return 1;
        }
    };
    let service = match start_service(options.offline, &data) {
        Ok(service) => Arc::new(service),
        Err(message) => {
            alert(&message);
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

    let open = !options.tray;
    let built = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::Builder::new().args(["--tray"]).build())
        .plugin(tauri_plugin_opener::init())
        .manage(service.clone())
        .manage(shortcuts::Problems::default())
        .invoke_handler(tauri::generate_handler![
            get,
            post,
            save_text,
            tray_status,
            tray_preset,
            flyout_hide,
            flyout_open_editor,
            shortcuts_get,
            shortcuts_pause,
            shortcuts_set,
            settings_get,
            settings_set,
            updates_get,
            open_release
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            preferences::setup(&handle);
            tray::setup(&handle)?;
            updates::setup(&handle);
            shortcuts::register(&handle);
            let commands = handle.clone();
            let listening = remote::listen(
                &data,
                Arc::new(move |request: &Value| match request.get("command").and_then(Value::as_str) {
                    Some("show") => {
                        open_editor(&commands, None);
                        json!({})
                    }
                    _ => {
                        let result = service.command(request);
                        tray::update(&commands);
                        result.unwrap_or_else(|failure| failure.body())
                    }
                }),
            );
            if let Err(error) = listening {
                eprintln!("Commands from other processes are off: {error}");
            }
            // Without a tray icon there would be nothing on screen.
            if open || !preferences::get(&handle, "tray_icon") {
                open_editor(&handle, None);
            }
            Ok(())
        })
        .build(tauri::generate_context!());
    let app = match built {
        Ok(app) => app,
        Err(error) => {
            alert(&format!("Maxwell PEQ could not start: {error}"));
            return 1;
        }
    };
    app.run(|_, event| {
        // Closing the editor leaves the tray icon running (see `tray::editor_closed`);
        // Quit exits with a code.
        if let RunEvent::ExitRequested { code: None, api, .. } = event {
            api.prevent_exit();
        }
    });
    0
}
