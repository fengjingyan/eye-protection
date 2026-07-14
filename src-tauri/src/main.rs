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
    // 当前休息时长（秒）。休息窗口按需重建时，加载后主动拉取该值，
    // 避免 start-rest 事件早于窗口 JS 监听器注册而丢失导致时长错误。
    current_rest_secs: Mutex<u64>,
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

    // 真正销毁休息窗口，回收其 WebView renderer 进程内存（而非仅隐藏）
    for window in app_handle.windows().values() {
        if window.label().starts_with("reminder") {
            let _ = window.close();
        }
    }
    trim_working_set();
}

// 供休息窗口加载后拉取当前休息时长（秒），避免事件竞态
#[tauri::command]
fn get_rest_secs(state: tauri::State<Arc<AppState>>) -> u64 {
    *state.current_rest_secs.lock().unwrap()
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

// Windows: 把当前进程的工作集交还给系统，降低任务管理器显示的内存占用。
// 注意：只作用于本 Rust 进程，WebView2 的子进程是独立 PID，不受影响；
// 真正的内存下降来自窗口按需创建/销毁与 WebView2 进程精简参数。
#[cfg(target_os = "windows")]
fn trim_working_set() {
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn SetProcessWorkingSetSizeEx(
            h_process: isize,
            dw_minimum_working_set_size: usize,
            dw_maximum_working_set_size: usize,
            flags: u32,
        ) -> i32;
    }
    // 传入 usize::MAX (即 -1) 表示让系统清空工作集
    unsafe {
        let _ = SetProcessWorkingSetSizeEx(GetCurrentProcess(), usize::MAX, usize::MAX, 0);
    }
}

#[cfg(not(target_os = "windows"))]
fn trim_working_set() {}

// 按需创建/复用休息提醒窗口（透明、全屏、无边框、置顶）
fn build_reminder_window(app_handle: &tauri::AppHandle, label: &str) -> Option<tauri::Window> {
    WindowBuilder::new(app_handle, label, WindowUrl::App("reminder.html".into()))
        .transparent(true)
        .always_on_top(true)
        .decorations(false)
        .skip_taskbar(true)
        .visible(false)
        .fullscreen(false)
        .build()
        .ok()
}

// 按需显示设置窗口：存在则显示，不存在则新建（关闭时会被销毁以释放 WebView）
fn show_settings_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_window("settings") {
        let _ = win.show();
        let _ = win.set_focus();
        let _ = win.emit("show-settings", ());
        return;
    }

    if let Ok(win) = WindowBuilder::new(app, "settings", WindowUrl::App("settings.html".into()))
        .title("Eye Protection Settings")
        .inner_size(400.0, 720.0)
        .resizable(false)
        .center()
        .visible(false)
        .build()
    {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

// 按需显示关于窗口
fn show_about_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_window("about") {
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }

    if let Ok(win) = WindowBuilder::new(app, "about", WindowUrl::App("about.html".into()))
        .title("About EyeProtection")
        .inner_size(500.0, 350.0)
        .resizable(false)
        .center()
        .decorations(true)
        .visible(false)
        .build()
    {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

fn show_reminder_windows(app_handle: &tauri::AppHandle, rest_secs: u64) {
    // 先记录本次休息时长，供窗口加载后通过 get_rest_secs 拉取
    {
        let state: tauri::State<Arc<AppState>> = app_handle.state();
        *state.current_rest_secs.lock().unwrap() = rest_secs;
    }

    // 空闲时可能没有任何窗口存在。先建（或复用）主休息窗口，
    // 才能通过它查询可用显示器，再为其余显示器各建一个。
    let primary = match app_handle.get_window("reminder") {
        Some(win) => win,
        None => match build_reminder_window(app_handle, "reminder") {
            Some(win) => win,
            None => return,
        },
    };

    let monitors = primary.available_monitors().unwrap_or_default();

    // 主窗口放到第一个显示器
    if let Some(monitor) = monitors.first() {
        let _ = primary.set_fullscreen(false);
        let _ = primary.set_position(tauri::Position::Physical(*monitor.position()));
    }
    let _ = primary.set_fullscreen(true);
    let _ = primary.show();
    let _ = primary.set_focus();
    let _ = primary.emit("start-rest", rest_secs);

    // 其余显示器
    for (i, monitor) in monitors.iter().enumerate().skip(1) {
        let label = format!("reminder_{}", i);
        let win = match app_handle.get_window(&label) {
            Some(win) => win,
            None => match build_reminder_window(app_handle, &label) {
                Some(win) => win,
                None => continue,
            },
        };

        let _ = win.set_fullscreen(false);
        let _ = win.set_position(tauri::Position::Physical(*monitor.position()));
        let _ = win.set_fullscreen(true);
        let _ = win.show();
        let _ = win.set_focus();
        let _ = win.emit("start-rest", rest_secs);
    }
}

fn main() {
    // 精简 WebView2 子进程：多窗口/多显示器共用同一 renderer 进程、限制 renderer 数量。
    // 不关闭 GPU：休息窗口是全屏透明，走 GPU 合成比软件合成更省内存也更稳。
    std::env::set_var(
        "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
        "--process-per-site --renderer-process-limit=1",
    );

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
        current_rest_secs: Mutex::new(0),
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
                    // 用 process::exit 直接退出，绕开 ExitRequested 的 prevent_exit
                    std::process::exit(0);
                }
                "settings" => {
                    show_settings_window(&app.app_handle());
                }
                "about" => {
                    show_about_window(&app.app_handle());
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
        // 不再拦截关闭：settings/about 关闭时直接销毁窗口以回收 WebView 内存，
        // 下次从托盘打开会按需重建。托盘保活由 RunEvent::ExitRequested 处理。
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
                                let _ = window.close();
                            }
                        }
                        trim_working_set();
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
            set_window_size,
            get_rest_secs
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|_app_handle, event| {
            // 所有窗口关闭后不退出应用，保持托盘常驻
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                api.prevent_exit();
            }
        });
}
