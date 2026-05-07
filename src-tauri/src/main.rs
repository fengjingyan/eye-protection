#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use rdev::{listen, Event};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{
    CustomMenuItem, Manager, SystemTray, SystemTrayEvent, SystemTrayMenu, SystemTrayMenuItem,
    WindowBuilder, WindowUrl,
};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
struct Settings {
    work_time: u64,                // minutes — 连续工作时间
    rest_time: u64,                // minutes — 短暂休息时长
    long_work_threshold_mins: u64, // minutes — 累计工作阈值（触发长时休息）
    long_rest_mins: u64,           // minutes — 长时休息时长
    opacity: f64,
    auto_start: bool,
    language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            work_time: 10,
            rest_time: 1,
            long_work_threshold_mins: 60, // 1 hour
            long_rest_mins: 5,
            opacity: 0.8,
            auto_start: false,
            language: "zh-CN".to_string(),
        }
    }
}

fn settings_file_path(app_handle: &tauri::AppHandle) -> Option<PathBuf> {
    let resolver = app_handle.path_resolver();
    resolver
        .app_config_dir()
        .or_else(|| resolver.app_data_dir())
        .map(|dir| dir.join("settings.json"))
}

fn read_settings_file(path: &PathBuf) -> Option<Settings> {
    if !path.is_file() {
        return None;
    }

    std::fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str::<Settings>(&content).ok())
}

fn save_settings_to_disk(app_handle: &tauri::AppHandle, settings: &Settings) {
    let Some(path) = settings_file_path(app_handle) else {
        return;
    };

    if let Some(parent) = path.parent() {
        if !parent.exists() && std::fs::create_dir_all(parent).is_err() {
            return;
        }

        if !parent.is_dir() {
            return;
        }
    }

    if let Ok(content) = serde_json::to_string(settings) {
        let _ = std::fs::write(path, content);
    }
}

fn load_settings_from_disk(app_handle: &tauri::AppHandle) -> Settings {
    if let Some(path) = settings_file_path(app_handle) {
        if let Some(settings) = read_settings_file(&path) {
            return settings;
        }
    }

    let legacy_path = PathBuf::from("settings.json");
    if let Some(settings) = read_settings_file(&legacy_path) {
        save_settings_to_disk(app_handle, &settings);
        return settings;
    }

    Settings::default()
}

struct AppState {
    settings: Mutex<Settings>,
    last_activity: Mutex<Instant>,
    accumulated_work_time: Mutex<Duration>,
    long_accumulated_work_time: Mutex<Duration>,
    is_resting: Mutex<bool>,
    is_long_resting: Mutex<bool>,
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
        if c.is_empty() {
            continue;
        }

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
                        // great-grandparent = project root when binary is at
                        // src-tauri/target/{debug|release}/
                        if let Some(great_grandparent) = grandparent.parent() {
                            search_dirs.push(great_grandparent.to_path_buf());
                        }
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
    let tray = app_handle.tray_handle();
    let _ = tray.get_item("settings").set_title(get_l10n_string(locale, "tray.settings"));
    let _ = tray.get_item("rest_now").set_title(get_l10n_string(locale, "tray.rest_now"));
    let _ = tray.get_item("about").set_title(get_l10n_string(locale, "tray.about"));
    let _ = tray.get_item("quit").set_title(get_l10n_string(locale, "tray.quit"));
}

#[tauri::command]
fn get_settings(state: tauri::State<Arc<AppState>>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn save_settings(
    state: tauri::State<Arc<AppState>>,
    settings: Settings,
    app_handle: tauri::AppHandle,
) {
    let mut s = state.settings.lock().unwrap();
    *s = settings.clone();

    // Apply opacity to reminder window if it exists
    for window in app_handle.windows().values() {
        if window.label().starts_with("reminder") {
            let _ = window.emit("update-settings", settings.clone());
        }
    }

    save_settings_to_disk(&app_handle, &s);

    // Apply autostart setting (Windows)
    set_windows_autostart(s.auto_start, &app_handle);

    // Always reload locale and update tray so the menu stays in sync with the
    // saved language, even when the language field itself did not change (e.g.
    // the very first save after startup where the default "zh-CN" was already
    // stored but the tray had fallen back to English due to a failed initial load).
    let new_lang = s.language.clone();
    drop(s); // release settings lock before acquiring locale lock
    {
        // Use freshly loaded locale if available, otherwise fall back to the
        // already-cached state locale so the tray is always updated.
        let fresh = load_locale(Some(&app_handle), &new_lang);
        let mut state_locale = state.locale.lock().unwrap();
        if let Some(locale) = fresh {
            *state_locale = locale;
        }
        update_tray_menu(&app_handle, &*state_locale);
    }
}

#[tauri::command]
fn close_reminder(state: tauri::State<Arc<AppState>>, app_handle: tauri::AppHandle) {
    let mut is_resting = state.is_resting.lock().unwrap();
    let mut is_long_resting = state.is_long_resting.lock().unwrap();
    let mut accumulated = state.accumulated_work_time.lock().unwrap();
    let mut long_accumulated = state.long_accumulated_work_time.lock().unwrap();

    let was_long_rest = *is_long_resting;
    *is_resting = false;
    *is_long_resting = false;
    *accumulated = Duration::from_secs(0);
    // 长休息结束后重置累计总工时，短休息不重置
    if was_long_rest {
        *long_accumulated = Duration::from_secs(0);
    }

    drop(is_resting);
    drop(is_long_resting);
    drop(accumulated);
    drop(long_accumulated);

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
        match hkcu.open_subkey_with_flags(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
            KEY_WRITE,
        ) {
            Ok(run_key) => {
                if enable {
                    let _ = run_key.set_value("EyeProtection", &format!("\"{}\"", exe_str));
                } else {
                    let _ = run_key.delete_value("EyeProtection");
                }
            }
            Err(_) => {
                if let Ok((run_key, _disp)) =
                    hkcu.create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
                {
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

fn show_reminder_windows(app_handle: &tauri::AppHandle, rest_secs: u64) {
    let monitors = if let Some(win) = app_handle.windows().values().next() {
        win.available_monitors().unwrap_or_default()
    } else {
        return;
    };

    for (i, monitor) in monitors.iter().enumerate() {
        let label = if i == 0 {
            "reminder".to_string()
        } else {
            format!("reminder_{}", i)
        };

        if let Some(win) = app_handle.get_window(&label) {
            let pos = monitor.position();
            let _ = win.set_fullscreen(false);
            let _ = win.set_position(tauri::Position::Physical(*pos));
            let _ = win.set_fullscreen(true);
            let _ = win.show();
            let _ = win.set_focus();
            let _ = win.emit("start-rest", rest_secs);
        } else {
            let res =
                WindowBuilder::new(app_handle, &label, WindowUrl::App("reminder.html".into()))
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
                let _ = win.emit("start-rest", rest_secs);
            }
        }
    }
}

fn main() {
    let settings = Settings::default();

    // Pre-load locale
    let initial_locale = load_locale(None, &settings.language).unwrap_or_else(|| {
        serde_json::from_str(r#"{"tray":{"work_timer":"工作时长","settings":"设置","rest_now":"立即休息","about":"关于","quit":"退出"}}"#).unwrap()
    });

    let state = Arc::new(AppState {
        settings: Mutex::new(settings),
        last_activity: Mutex::new(Instant::now()),
        accumulated_work_time: Mutex::new(Duration::from_secs(0)),
        long_accumulated_work_time: Mutex::new(Duration::from_secs(0)),
        is_resting: Mutex::new(false),
        is_long_resting: Mutex::new(false),
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
        .add_item(CustomMenuItem::new("settings".to_string(), "设置"))
        .add_item(CustomMenuItem::new("rest_now".to_string(), "立即休息"))
        .add_item(CustomMenuItem::new("about".to_string(), "关于"))
        .add_native_item(SystemTrayMenuItem::Separator)
        .add_item(CustomMenuItem::new("quit".to_string(), "退出"));

    let system_tray = SystemTray::new().with_menu(tray_menu);

    tauri::Builder::default()
        .manage(state.clone())
        .system_tray(system_tray)
        .on_system_tray_event(|app, event| match event {
            SystemTrayEvent::MenuItemClick { id, .. } => match id.as_str() {
                "quit" => {
                    app.exit(0);
                }
                "settings" => {
                    if let Some(window) = app.get_window("settings") {
                        let _ = window.show();
                        let _ = window.set_focus();
                        // emit after show so the JS resize fires while the window
                        // is already visible (set_size on hidden windows is unreliable)
                        let _ = window.emit("show-settings", ());
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
                    let rest_secs = {
                        let s = state.settings.lock().unwrap();
                        s.rest_time * 60
                    };
                    let mut is_resting = state.is_resting.lock().unwrap();
                    *is_resting = true;
                    drop(is_resting);

                    show_reminder_windows(&app.app_handle(), rest_secs);
                }
                _ => {}
            },
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
                let loaded_settings = load_settings_from_disk(&app_handle);
                let mut s = state.settings.lock().unwrap();
                *s = loaded_settings;
                let mut state_locale = state.locale.lock().unwrap();

                // Try to reload with app_handle for resource resolution
                if let Some(locale) = load_locale(Some(&app_handle), &s.language) {
                    *state_locale = locale;
                }
                update_tray_menu(&app_handle, &*state_locale);
            }

            // Timer loop
            thread::spawn(move || {
                loop {
                    thread::sleep(Duration::from_secs(1));

                    let now = Instant::now();
                    let settings = state.settings.lock().unwrap().clone();
                    let last_activity = *state.last_activity.lock().unwrap();

                    // 锁顺序: is_resting -> is_long_resting -> accumulated -> long_accumulated
                    let mut is_resting = state.is_resting.lock().unwrap();
                    let mut is_long_resting = state.is_long_resting.lock().unwrap();
                    let mut accumulated = state.accumulated_work_time.lock().unwrap();
                    let mut long_accumulated = state.long_accumulated_work_time.lock().unwrap();

                    let gap = now.duration_since(last_activity);
                    let rest_threshold = Duration::from_secs(settings.rest_time * 60);

                    let mut hide_windows = false;
                    let mut show_rest: Option<u64> = None;

                    // Logic 1: 空闲超过休息时长 -> 重置短休息计时器（不影响累计总工时）
                    if gap > rest_threshold {
                        *accumulated = Duration::from_secs(0);
                        if *is_resting {
                            *is_resting = false;
                            hide_windows = true;
                        }
                    }

                    if !*is_resting && !*is_long_resting {
                        // Logic 2: 活跃时同时累计短计时器和总工时
                        if gap <= rest_threshold {
                            *accumulated += Duration::from_secs(1);
                            *long_accumulated += Duration::from_secs(1);
                        }

                        // Update tray tooltip
                        let total_secs = accumulated.as_secs();
                        let hours = total_secs / 3600;
                        let minutes = (total_secs % 3600) / 60;
                        let seconds = total_secs % 60;
                        let time_str = format!("{:02}:{:02}:{:02}", hours, minutes, seconds);

                        let locale = state.locale.lock().unwrap();
                        let prefix = get_l10n_string(&locale, "tray.work_timer");

                        let status = if gap > Duration::from_secs(10) {
                            if settings.language == "zh-CN" {
                                " (空闲)"
                            } else {
                                " (Idle)"
                            }
                        } else {
                            if settings.language == "zh-CN" {
                                " (活跃)"
                            } else {
                                " (Active)"
                            }
                        };

                        let _ = app_handle
                            .tray_handle()
                            .set_tooltip(&format!("{}: {}{}", prefix, time_str, status));

                        // Logic 3a: 累计工作满阈值 -> 触发长休息（优先级高）
                        let long_threshold_secs = settings.long_work_threshold_mins * 60;
                        let long_rest_secs = settings.long_rest_mins * 60;
                        if *long_accumulated >= Duration::from_secs(long_threshold_secs) {
                            *is_long_resting = true;
                            show_rest = Some(long_rest_secs);
                        }
                        // Logic 3b: 本轮工作满 work_time 分钟 -> 触发短休息
                        else if *accumulated >= Duration::from_secs(settings.work_time * 60) {
                            *is_resting = true;
                            show_rest = Some(settings.rest_time * 60);
                        }
                    }

                    drop(is_resting);
                    drop(is_long_resting);
                    drop(accumulated);
                    drop(long_accumulated);

                    if hide_windows {
                        for window in app_handle.windows().values() {
                            if window.label().starts_with("reminder") {
                                let _ = window.hide();
                            }
                        }
                    }
                    if let Some(secs) = show_rest {
                        show_reminder_windows(&app_handle, secs);
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            close_reminder,
            set_window_size
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
