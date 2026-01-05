#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::thread;
use tauri::{
    CustomMenuItem, Manager, SystemTray, SystemTrayEvent, SystemTrayMenu, SystemTrayMenuItem,
    WindowBuilder, WindowUrl,
};
use rdev::{listen, Event};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
struct Settings {
    work_time: u64, // minutes
    rest_time: u64, // minutes
    opacity: f64,
    auto_start: bool,
    language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            work_time: 40,
            rest_time: 5,
            opacity: 0.8,
            auto_start: false,
            language: "zh-CN".to_string(),
        }
    }
}

struct AppState {
    settings: Mutex<Settings>,
    last_activity: Mutex<Instant>,
    accumulated_work_time: Mutex<Duration>,
    is_resting: Mutex<bool>,
    locale: Mutex<Value>,
}

// helper: load locale json from ui/i18n
fn load_locale(app_handle: Option<&tauri::AppHandle>, lang: &str) -> Option<Value> {
    let candidates = [
        lang.to_string(),
        lang.split('-').next().unwrap_or("").to_string(),
        "zh-CN".to_string(),
        "en".to_string(),
    ];
    
    for c in &candidates {
        if c.is_empty() { continue; }
        
        // 1. Try resolve via Tauri resource (for bundled app)
        if let Some(handle) = app_handle {
            let resource_paths = [
                format!("ui/i18n/{}.json", c),
                format!("i18n/{}.json", c),
                format!("{}.json", c),
            ];
            for rp in &resource_paths {
                if let Some(p) = handle.path_resolver().resolve_resource(rp) {
                    if let Ok(s) = std::fs::read_to_string(p) {
                        if let Ok(v) = serde_json::from_str::<Value>(&s) {
                            return Some(v);
                        }
                    }
                }
            }
        }

        // 2. Try relative paths (for dev mode)
        let mut search_dirs = vec![std::env::current_dir().unwrap_or_default()];
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                search_dirs.push(exe_dir.to_path_buf());
                if let Some(parent) = exe_dir.parent() {
                    search_dirs.push(parent.to_path_buf());
                    if let Some(grandparent) = parent.parent() {
                        search_dirs.push(grandparent.to_path_buf());
                    }
                }
            }
        }

        for dir in search_dirs {
            let dev_paths = [
                dir.join(format!("ui/i18n/{}.json", c)),
                dir.join(format!("i18n/{}.json", c)),
                dir.join(format!("{}.json", c)),
                dir.join(format!("../ui/i18n/{}.json", c)),
            ];
            for p in dev_paths {
                if let Ok(s) = std::fs::read_to_string(p) {
                    if let Ok(v) = serde_json::from_str::<Value>(&s) {
                        return Some(v);
                    }
                }
            }
        }
    }
    None
}

fn get_l10n_string(v: &Value, key: &str) -> String {
    let mut cur = v;
    for part in key.split('.') {
        if let Some(next) = cur.get(part) {
            cur = next;
        } else {
            return key.to_string();
        }
    }
    if cur.is_string() {
        cur.as_str().unwrap().to_string()
    } else {
        key.to_string()
    }
}

fn update_tray_menu(app_handle: &tauri::AppHandle, locale: &Value) {
    let settings_label = get_l10n_string(locale, "tray.settings");
    let rest_label = get_l10n_string(locale, "tray.rest_now");
    let about_label = get_l10n_string(locale, "tray.about");
    let quit_label = get_l10n_string(locale, "tray.quit");
    
    let new_menu = SystemTrayMenu::new()
        .add_item(CustomMenuItem::new("settings".to_string(), settings_label))
        .add_item(CustomMenuItem::new("rest_now".to_string(), rest_label))
        .add_item(CustomMenuItem::new("about".to_string(), about_label))
        .add_native_item(SystemTrayMenuItem::Separator)
        .add_item(CustomMenuItem::new("quit".to_string(), quit_label));
    let _ = app_handle.tray_handle().set_menu(new_menu);
}

#[tauri::command]
fn get_settings(state: tauri::State<Arc<AppState>>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn save_settings(state: tauri::State<Arc<AppState>>, settings: Settings, app_handle: tauri::AppHandle) {
    let mut s = state.settings.lock().unwrap();
    let old_lang = s.language.clone();
    *s = settings.clone();
    
    // Apply opacity to reminder window if it exists
    for window in app_handle.windows().values() {
        if window.label().starts_with("reminder") {
            let _ = window.emit("update-settings", settings.clone());
        }
    }
    
    // Save to disk (simplified)
    let _ = std::fs::write("settings.json", serde_json::to_string(&*s).unwrap());

    // Apply autostart setting (Windows)
    set_windows_autostart(s.auto_start, &app_handle);

    // Update tray menu labels if language changed
    if old_lang != s.language {
        if let Some(locale) = load_locale(Some(&app_handle), &s.language) {
            let mut state_locale = state.locale.lock().unwrap();
            *state_locale = locale.clone();
            update_tray_menu(&app_handle, &locale);
        }
    }
}

#[tauri::command]
fn close_reminder(state: tauri::State<Arc<AppState>>, app_handle: tauri::AppHandle) {
    let mut is_resting = state.is_resting.lock().unwrap();
    *is_resting = false;
    
    let mut accumulated = state.accumulated_work_time.lock().unwrap();
    *accumulated = Duration::from_secs(0);
    
    for window in app_handle.windows().values() {
        if window.label().starts_with("reminder") {
            let _ = window.hide();
        }
    }
}

#[tauri::command]
fn set_window_size(app_handle: tauri::AppHandle, width: f64, height: f64) {
    if let Some(win) = app_handle.get_window("settings") {
        let _ = win.set_size(tauri::Size::Logical(tauri::LogicalSize { width, height }));
    }
}

// Configure autostart on Windows by adding/removing Run registry entry
#[cfg(target_os = "windows")]
fn set_windows_autostart(enable: bool, _app_handle: &tauri::AppHandle) {
    use winreg::enums::*;
    use winreg::RegKey;

    if let Ok(exe_path) = std::env::current_exe() {
        let exe_str = exe_path.display().to_string();
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        match hkcu.open_subkey_with_flags("Software\\Microsoft\\Windows\\CurrentVersion\\Run", KEY_WRITE) {
            Ok(run_key) => {
                if enable {
                    let _ = run_key.set_value("EyeProtection", &format!("\"{}\"", exe_str));
                } else {
                    let _ = run_key.delete_value("EyeProtection");
                }
            }
            Err(_) => {
                if let Ok((run_key, _disp)) = hkcu.create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run") {
                    if enable {
                        let _ = run_key.set_value("EyeProtection", &format!("\"{}\"", exe_str));
                    }
                }
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn set_windows_autostart(_enable: bool, _app_handle: &tauri::AppHandle) {}

fn show_reminder_windows(app_handle: &tauri::AppHandle) {
    let monitors = if let Some(win) = app_handle.windows().values().next() {
        win.available_monitors().unwrap_or_default()
    } else {
        return;
    };
    
    for (i, monitor) in monitors.iter().enumerate() {
        let label = if i == 0 { "reminder".to_string() } else { format!("reminder_{}", i) };
        
        if let Some(win) = app_handle.get_window(&label) {
            let pos = monitor.position();
            let _ = win.set_fullscreen(false);
            let _ = win.set_position(tauri::Position::Physical(*pos));
            let _ = win.set_fullscreen(true);
            let _ = win.show();
            let _ = win.set_focus();
            let _ = win.emit("start-rest", ());
        } else {
            let res = WindowBuilder::new(
                app_handle,
                &label,
                WindowUrl::App("reminder.html".into())
            )
            .transparent(true)
            .always_on_top(true)
            .decorations(false)
            .skip_taskbar(true)
            .visible(false)
            .build();

            if let Ok(win) = res {
                let pos = monitor.position();
                let _ = win.set_position(tauri::Position::Physical(*pos));
                let _ = win.set_fullscreen(true);
                let _ = win.show();
                let _ = win.set_focus();
                let _ = win.emit("start-rest", ());
            }
        }
    }
}

fn main() {
    let settings = if let Ok(content) = std::fs::read_to_string("settings.json") {
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        Settings::default()
    };

    // Pre-load locale
    let initial_locale = load_locale(None, &settings.language).unwrap_or_else(|| {
        serde_json::from_str(r#"{"tray":{"work_timer":"Work Duration","settings":"Settings","rest_now":"Rest Now","about":"About","quit":"Quit"}}"#).unwrap()
    });

    let state = Arc::new(AppState {
        settings: Mutex::new(settings),
        last_activity: Mutex::new(Instant::now()),
        accumulated_work_time: Mutex::new(Duration::from_secs(0)),
        is_resting: Mutex::new(false),
        locale: Mutex::new(initial_locale),
    });

    let state_clone = state.clone();
    
    // Input monitoring thread
    thread::spawn(move || {
        let callback = move |_event: Event| {
            let mut last = state_clone.last_activity.lock().unwrap();
            *last = Instant::now();
        };
        if let Err(error) = listen(callback) {
            println!("Error: {:?}", error);
        }
    });

    // Initial tray labels (will be updated in setup with actual locale)
    let tray_menu = SystemTrayMenu::new()
        .add_item(CustomMenuItem::new("settings".to_string(), "..."))
        .add_item(CustomMenuItem::new("rest_now".to_string(), "..."))
        .add_item(CustomMenuItem::new("about".to_string(), "..."))
        .add_native_item(SystemTrayMenuItem::Separator)
        .add_item(CustomMenuItem::new("quit".to_string(), "..."));

    let system_tray = SystemTray::new().with_menu(tray_menu);

    tauri::Builder::default()
        .manage(state.clone())
        .system_tray(system_tray)
        .on_system_tray_event(|app, event| match event {
            SystemTrayEvent::MenuItemClick { id, .. } => {
                match id.as_str() {
                    "quit" => {
                        app.exit(0);
                    }
                    "settings" => {
                        if let Some(window) = app.get_window("settings") {
                            let _ = window.emit("show-settings", ());
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "about" => {
                        if let Some(window) = app.get_window("about") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "rest_now" => {
                        let state: tauri::State<Arc<AppState>> = app.state();
                        let mut is_resting = state.is_resting.lock().unwrap();
                        *is_resting = true;
                        
                        show_reminder_windows(&app.app_handle());
                    }
                    _ => {}
                }
            }
            _ => {}
        })
        .on_window_event(|event| match event.event() {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                let label = event.window().label();
                if label == "settings" || label == "about" {
                    let _ = event.window().hide();
                    api.prevent_close();
                }
            }
            _ => {}
        })
        .setup(move |app| {
            let app_handle = app.handle();
            let state = state.clone();

            // Initial tray update
            {
                let s = state.settings.lock().unwrap();
                let mut state_locale = state.locale.lock().unwrap();
                
                // Try to reload with app_handle for resource resolution
                if let Some(locale) = load_locale(Some(&app_handle), &s.language) {
                    *state_locale = locale;
                }
                update_tray_menu(&app_handle, &*state_locale);

                // Apply autostart setting on startup
                set_windows_autostart(s.auto_start, &app_handle);
            }
            
            // Timer loop
            thread::spawn(move || {
                loop {
                    thread::sleep(Duration::from_secs(1));
                    
                    let now = Instant::now();
                    let settings = state.settings.lock().unwrap().clone();
                    let last_activity = *state.last_activity.lock().unwrap();
                    
                    // Lock order: is_resting -> accumulated
                    let mut is_resting = state.is_resting.lock().unwrap();
                    let mut accumulated = state.accumulated_work_time.lock().unwrap();
                    
                    let gap = now.duration_since(last_activity);
                    
                    // Logic 1: If operation interval > rest time, reset work time
                    if gap > Duration::from_secs(settings.rest_time * 60) {
                        *accumulated = Duration::from_secs(0);
                        
                        if *is_resting {
                            *is_resting = false;
                            for window in app_handle.windows().values() {
                                if window.label().starts_with("reminder") {
                                    let _ = window.hide();
                                }
                            }
                        }
                    }
                    
                    if !*is_resting {
                        // Logic 2: Accumulate work time
                        // We count it as work if the gap is less than rest_time
                        if gap <= Duration::from_secs(settings.rest_time * 60) {
                             *accumulated += Duration::from_secs(1);
                        }
                        
                        // Update tray tooltip
                        let total_secs = accumulated.as_secs();
                        let hours = total_secs / 3600;
                        let minutes = (total_secs % 3600) / 60;
                        let seconds = total_secs % 60;
                        let time_str = format!("{:02}:{:02}:{:02}", hours, minutes, seconds);
                        
                        let locale = state.locale.lock().unwrap();
                        let prefix = get_l10n_string(&locale, "tray.work_timer");
                        
                        // Add activity status to tooltip
                        let status = if gap > Duration::from_secs(10) {
                            if settings.language == "zh-CN" { " (空闲)" } else { " (Idle)" }
                        } else {
                            if settings.language == "zh-CN" { " (活跃)" } else { " (Active)" }
                        };
                        
                        let _ = app_handle.tray_handle().set_tooltip(&format!("{}: {}{}", prefix, time_str, status));
                        
                        // Logic 3: Trigger reminder
                        if *accumulated >= Duration::from_secs(settings.work_time * 60) {
                            *is_resting = true;
                            show_reminder_windows(&app_handle);
                        }
                    }
                }
            });
            
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_settings, save_settings, close_reminder, set_window_size])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
