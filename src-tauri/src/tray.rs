//! The tray icon: a glance at battery, mic and EQ drawn as the icon (see
//! [`glance`]), a right-click menu, and a left-click flyout with the same.
//!
//! A background thread reads the headset once a minute, and right after
//! anything changes it. It gives way to every other device request.

use crate::glance::{self, Glance};
use crate::{preferences, shortcuts, system, updates};
use maxwell_core::service::{Service, summary};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Rect, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_autostart::ManagerExt;

const TRAY: &str = "tray";
pub const FLYOUT: &str = "flyout";
const POLL: Duration = Duration::from_secs(60);
const FLYOUT_SIZE: (f64, f64) = (320.0, 316.0);

pub struct Tray {
    wake: Mutex<mpsc::Sender<()>>,
    /// What the icon and menu show now; skips rebuilding them when nothing changed.
    shown: Mutex<String>,
    /// When the flyout last hid on losing focus. Clicking the tray icon takes
    /// focus first, and that click shouldn't open it again.
    hidden_at: Mutex<Option<Instant>>,
    font: Option<fontdue::Font>,
    quitting: AtomicBool,
}

fn service(app: &AppHandle) -> Arc<Service> {
    app.state::<Arc<Service>>().inner().clone()
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let (wake, woken) = mpsc::channel();
    let font = system::font_paths()
        .iter()
        .filter_map(|path| std::fs::read(path).ok())
        .find_map(|bytes| fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok());
    app.manage(Tray {
        wake: Mutex::new(wake),
        shown: Mutex::new(String::new()),
        hidden_at: Mutex::new(None),
        font,
        quitting: AtomicBool::new(false),
    });

    let mut tray = TrayIconBuilder::with_id(TRAY)
        .tooltip("Maxwell PEQ")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| menu_action(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left, button_state: MouseButtonState::Up, rect, ..
            } = event
            {
                toggle_flyout(tray.app_handle(), rect);
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?.set_visible(preferences::get(app, "tray_icon"))?;
    flyout(app)?;

    let app = app.clone();
    std::thread::spawn(move || poll(&app, &woken));
    Ok(())
}

/// Show or hide the tray icon.
pub fn show(app: &AppHandle, visible: bool) {
    if let Some(tray) = app.tray_by_id(TRAY) {
        let _ = tray.set_visible(visible);
    }
    if !visible {
        hide_flyout(app);
    }
}

/// Read the headset now instead of at the next minute.
pub fn wake(app: &AppHandle) {
    if let Some(tray) = app.try_state::<Tray>() {
        let _ = tray.wake.lock().unwrap_or_else(|e| e.into_inner()).send(());
    }
}

fn poll(app: &AppHandle, woken: &mpsc::Receiver<()>) {
    loop {
        // Skipped when another request holds the headset; that one refreshes the tray itself.
        let _ = service(app).post_background("/api/glance", &json!({}));
        update_now(app);
        if let Err(mpsc::RecvTimeoutError::Disconnected) = woken.recv_timeout(POLL) {
            return;
        }
        while woken.try_recv().is_ok() {}
    }
}

/// Run a `Service::command` off the UI thread, then refresh the tray.
pub fn run(app: &AppHandle, request: Value) {
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(failure) = service(&app).command(&request) {
            eprintln!("{}", failure.message);
            let _ = app.emit_to(FLYOUT, "command-failed", failure.message);
        }
        update_now(&app);
    });
}

/// Refresh the icon, menu and flyout from the app's state, off the UI thread.
pub fn update(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || update_now(&app));
}

/// The summary the flyout shows, with the colours it needs.
pub fn flyout_state(app: &AppHandle) -> Value {
    let mut state = summary(&service(app).status());
    let light = system::apps_light();
    state["light"] = json!(light);
    state["accent"] = json!(system::accent(light));
    state["shortcuts"] =
        Value::Object(shortcuts::current(app).into_iter().map(|(id, keys)| (id, json!(keys))).collect());
    state
}

pub fn update_now(app: &AppHandle) {
    let state = flyout_state(app);
    let _ = app.emit_to(FLYOUT, "status-changed", state.clone());
    let Some(tray) = app.tray_by_id(TRAY) else { return };
    let Some(owner) = app.try_state::<Tray>() else { return };
    let light = system::taskbar_light();
    let autostart = app.autolaunch().is_enabled().unwrap_or(false);
    let keys = shortcuts::current(app);
    let update = updates::available(app);
    let key = format!("{state}{light}{autostart}{keys:?}{update:?}");
    {
        let mut shown = owner.shown.lock().unwrap_or_else(|e| e.into_inner());
        if *shown == key {
            return;
        }
        *shown = key;
    }
    let _ = tray.set_tooltip(Some(tooltip(&state)));
    let icon = owner.font.as_ref().map(|font| {
        let glance = Glance {
            battery: state["battery"].as_u64().map(|battery| battery.min(100) as u8),
            mic_muted: state["mic_muted"].as_bool(),
            slot: state["slot"].as_u64().and_then(|slot| u8::try_from(slot).ok()),
            light,
        };
        let size = system::icon_size();
        Image::new_owned(glance::draw(font, glance, size), size, size)
    });
    let _ = tray.set_icon(icon.or_else(|| app.default_window_icon().cloned().map(Image::to_owned)));
    let version = update.as_ref().map(|(version, _)| version.as_str());
    if let Err(error) = menu(app, &tray, &state, autostart, &keys, version) {
        eprintln!("Could not build the tray menu: {error}");
    }
}

/// One line about the headset: battery and mic, or why they're unknown.
fn headline(state: &Value) -> String {
    if state["offline"] == json!(true) {
        "Offline: headset controls are off".into()
    } else if state["audeze_running"] == json!(true) {
        "Close Audeze HQ to use the headset".into()
    } else if let Some(battery) = state["battery"].as_u64() {
        let mic = match state["mic_muted"].as_bool() {
            Some(true) => " · Mic muted",
            Some(false) => " · Mic on",
            None => "",
        };
        format!("Battery {battery}%{mic}")
    } else if state["error"].is_string() {
        "Headset not found".into()
    } else {
        "Reading the headset…".into()
    }
}

/// The active preset: "EQ 2 · Harman", or a built-in one such as "Bass Boost".
fn preset_label(state: &Value) -> Option<String> {
    match (state["slot"].as_u64(), state["profile"].as_str()) {
        (Some(slot), Some(profile)) => Some(format!("EQ {slot} · {profile}")),
        (Some(slot), None) => Some(format!("EQ {slot}")),
        _ => state["preset_name"].as_str().map(str::to_owned),
    }
}

fn tooltip(state: &Value) -> String {
    let mut text = format!("Maxwell PEQ\n{}", headline(state));
    if let Some(preset) = preset_label(state) {
        text.push('\n');
        text.push_str(&preset);
    }
    text
}

fn menu(
    app: &AppHandle,
    tray: &TrayIcon,
    state: &Value,
    autostart: bool,
    keys: &[(String, String)],
    update: Option<&str>,
) -> tauri::Result<()> {
    let accelerator =
        |id: &str| keys.iter().find(|(action, _)| action == id).map(|(_, keys)| keys.clone()).filter(|k| !k.is_empty());
    let ready = state["battery"].is_u64() || state["slot"].is_u64() || state["preset"].is_u64();
    let usable = state["offline"] != json!(true) && state["audeze_running"] != json!(true);
    let status = MenuItem::with_id(app, "status", headline(state), false, None::<&str>)?;
    let mut presets = Vec::new();
    for slot in 1..=4u64 {
        let profile = state["profiles"][slot as usize - 1].as_str();
        let label = profile.map_or(format!("EQ {slot}"), |name| format!("EQ {slot} · {}", name.replace('&', "&&")));
        let checked = state["slot"].as_u64() == Some(slot);
        presets.push(CheckMenuItem::with_id(
            app,
            format!("eq{slot}"),
            label,
            usable,
            checked,
            accelerator(&format!("preset{slot}")),
        )?);
    }
    let next = MenuItem::with_id(app, "next", "Next EQ", usable && ready, accelerator("next"))?;
    let previous = MenuItem::with_id(app, "previous", "Previous EQ", usable && ready, accelerator("previous"))?;
    let refresh = MenuItem::with_id(app, "refresh", "Read headset", usable, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open Maxwell PEQ", true, None::<&str>)?;
    let keys_item = MenuItem::with_id(app, "shortcuts", "Keyboard shortcuts…", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let download = update
        .map(|version| MenuItem::with_id(app, "update", format!("Download Maxwell PEQ {version}"), true, None::<&str>))
        .transpose()?;
    let login = if cfg!(windows) { "Start with Windows" } else { "Open at login" };
    let start = CheckMenuItem::with_id(app, "autostart", login, true, autostart, None::<&str>)?;
    let close_audeze = MenuItem::with_id(app, "close_audeze", "Close Audeze HQ", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let separators =
        [PredefinedMenuItem::separator(app)?, PredefinedMenuItem::separator(app)?, PredefinedMenuItem::separator(app)?];

    let mut items: Vec<&dyn IsMenuItem<tauri::Wry>> = vec![&status, &separators[0]];
    items.extend(presets.iter().map(|item| item as &dyn IsMenuItem<tauri::Wry>));
    items.extend([
        &next as &dyn IsMenuItem<tauri::Wry>,
        &previous,
        &separators[1],
        &open,
        &refresh,
        &keys_item,
        &settings,
        &start,
    ]);
    if state["audeze_running"] == json!(true) {
        items.push(&close_audeze);
    }
    items.push(&separators[2]);
    if let Some(download) = &download {
        items.push(download);
    }
    items.push(&quit);
    tray.set_menu(Some(Menu::with_items(app, &items)?))
}

fn menu_action(app: &AppHandle, id: &str) {
    match id {
        "next" | "previous" => run(app, json!({"command": "preset", "value": id})),
        "refresh" => wake(app),
        "open" => crate::open_editor(app, None),
        "shortcuts" => crate::open_editor(app, Some("shortcuts")),
        "settings" => crate::open_editor(app, Some("settings")),
        "update" => {
            if let Some((_, url)) = updates::available(app)
                && let Err(error) = updates::open(app, &url)
            {
                eprintln!("{error}");
            }
        }
        "autostart" => {
            let autolaunch = app.autolaunch();
            let result =
                if autolaunch.is_enabled().unwrap_or(false) { autolaunch.disable() } else { autolaunch.enable() };
            if let Err(error) = result {
                eprintln!("Could not change the login item: {error}");
            }
            update(app);
        }
        "close_audeze" => {
            let app = app.clone();
            std::thread::spawn(move || {
                let _ = service(&app).post("/api/close_audeze", &json!({}));
                wake(&app);
            });
        }
        "quit" => quit(app),
        _ => {
            if let Some(slot) = id.strip_prefix("eq") {
                run(app, json!({"command": "preset", "value": slot}));
            }
        }
    }
}

/// Close the editor first so it saves pending edits, then exit.
fn quit(app: &AppHandle) {
    let Some(editor) = app.get_webview_window("main") else {
        app.exit(0);
        return;
    };
    if let Some(tray) = app.try_state::<Tray>() {
        tray.quitting.store(true, Ordering::SeqCst);
    }
    let _ = editor.close();
    // In case the page never finishes closing.
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(5));
        app.exit(0);
    });
}

/// The editor window closed; exit too if Quit asked for it, or if the app
/// isn't set to keep running in the tray.
pub fn editor_closed(app: &AppHandle) {
    let quitting = app.try_state::<Tray>().is_some_and(|tray| tray.quitting.load(Ordering::SeqCst));
    if quitting || !preferences::keep_running(app) {
        app.exit(0);
    }
}

fn flyout(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let builder = WebviewWindowBuilder::new(app, FLYOUT, WebviewUrl::App("flyout.html".into()))
        .title("Maxwell PEQ")
        .inner_size(FLYOUT_SIZE.0, FLYOUT_SIZE.1)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .shadow(true);
    #[cfg(windows)]
    let builder = builder
        .transparent(true)
        .effects(tauri::window::EffectsBuilder::new().effect(tauri::window::Effect::Acrylic).build());
    let window = builder.build()?;
    system::round_corners(&window);
    let handle = app.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Focused(false) = event {
            hide_flyout(&handle);
        }
    });
    Ok(window)
}

pub fn hide_flyout(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(FLYOUT)
        && window.is_visible().unwrap_or(false)
    {
        let _ = window.hide();
        if let Some(tray) = app.try_state::<Tray>() {
            *tray.hidden_at.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        }
    }
}

fn toggle_flyout(app: &AppHandle, icon: Rect) {
    let Some(window) = app.get_webview_window(FLYOUT).or_else(|| flyout(app).ok()) else { return };
    if window.is_visible().unwrap_or(false) {
        hide_flyout(app);
        return;
    }
    let just_hidden = app.try_state::<Tray>().is_some_and(|tray| {
        tray.hidden_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some_and(|at| at.elapsed() < Duration::from_millis(300))
    });
    if just_hidden {
        return;
    }
    place(app, &window, icon);
    let _ = window.emit("flyout-shown", flyout_state(app));
    let _ = window.show();
    // The page reads the headset when it hears this.
    let _ = window.set_focus();
}

/// Above the tray icon, like the system flyouts: inside the work area with a
/// 12 px margin, against whichever edge the taskbar is on.
fn place(app: &AppHandle, window: &WebviewWindow, icon: Rect) {
    let position = icon.position.to_physical::<f64>(1.0);
    let size = icon.size.to_physical::<f64>(1.0);
    let (cx, cy) = (position.x + size.width / 2.0, position.y + size.height / 2.0);
    let monitor = app.monitor_from_point(cx, cy).ok().flatten().or_else(|| app.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else { return };
    let scale = monitor.scale_factor();
    let (width, height) = (FLYOUT_SIZE.0 * scale, FLYOUT_SIZE.1 * scale);
    let margin = 12.0 * scale;
    let work = monitor.work_area();
    let (left, top) = (f64::from(work.position.x), f64::from(work.position.y));
    let (right, bottom) = (left + f64::from(work.size.width), top + f64::from(work.size.height));
    let screen = monitor.position();
    let screen_bottom = f64::from(screen.y) + f64::from(monitor.size().height);
    let x = if cx >= right {
        right - width - margin
    } else if cx <= left {
        left + margin
    } else {
        (cx - width / 2.0).clamp(left + margin, (right - width - margin).max(left + margin))
    };
    // A taskbar on top, or on a side, leaves the icon above the work area's middle.
    let taskbar_top = top > f64::from(screen.y) && bottom >= screen_bottom;
    let y = if taskbar_top || cy < (top + bottom) / 2.0 { top + margin } else { bottom - height - margin };
    let _ = window.set_size(tauri::PhysicalSize::new(width.round() as u32, height.round() as u32));
    let _ = window.set_position(tauri::PhysicalPosition::new(x.round() as i32, y.round() as i32));
}
