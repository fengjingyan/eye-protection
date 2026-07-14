#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

//! Slint 版护眼程序（方案3）。
//! - 设置窗口用 Slint（声明式 UI，见 ui/settings.slint）。
//! - 托盘图标/菜单、休息浮层仍用 Win32（Slint 无托盘；全屏透明浮层 Win32 更稳）。
//! - 主事件循环用 slint::run_event_loop()；Win32 托盘窗口与计时器消息由该循环一并派发。
//! - 计时用 slint::Timer（在 Slint 事件循环中稳定触发）。

mod i18n;
mod overlay;
mod settings;

use serde_json::Value;
use settings::Settings;
use std::cell::RefCell;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use rdev::{listen, Event};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconFromResourceEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyMenu, GetCursorPos, GetSystemMetrics, LoadCursorW, LoadIconW,
    LookupIconIdFromDirectoryEx, MessageBoxW, RegisterClassW, SetForegroundWindow, TrackPopupMenu,
    HICON, IDC_ARROW, IDI_APPLICATION, LR_DEFAULTCOLOR, MB_OK, MF_SEPARATOR, MF_STRING,
    SM_CXSMICON, SM_CYSMICON, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_COMMAND,
    WM_LBUTTONUP, WM_RBUTTONUP, WNDCLASSW,
};

slint::include_modules!();

// ---- 常量 ----
const WM_TRAYICON: u32 = WM_APP + 0x100;
const TRAY_UID: u32 = 1;
const IDM_SETTINGS: usize = 40001;
const IDM_REST: usize = 40002;
const IDM_ABOUT: usize = 40003;
const IDM_QUIT: usize = 40004;

const ICON_ICO: &[u8] = include_bytes!("../icons/icon.ico");

// ---- 全局应用状态 ----
pub struct AppState {
    pub settings: Mutex<Settings>,
    pub last_activity: Mutex<Instant>,
    pub accumulated: Mutex<Duration>,
    pub long_accumulated: Mutex<Duration>,
    pub is_resting: Mutex<bool>,
    pub is_long_resting: Mutex<bool>,
    pub locale: Mutex<Value>,
}

static APP: OnceLock<Arc<AppState>> = OnceLock::new();
static MAIN_HWND: OnceLock<isize> = OnceLock::new();

fn app() -> &'static Arc<AppState> {
    APP.get().expect("APP not initialized")
}
fn main_hwnd() -> HWND {
    *MAIN_HWND.get().unwrap_or(&0)
}

thread_local! {
    // 保持 Slint 设置窗口实例存活（!Send，仅主线程）
    static SETTINGS_WIN: RefCell<Option<SettingsWindow>> = RefCell::new(None);
}

pub fn reset_rest_state(state: &AppState) {
    let mut is_resting = state.is_resting.lock().unwrap();
    let mut is_long = state.is_long_resting.lock().unwrap();
    let mut acc = state.accumulated.lock().unwrap();
    let mut long_acc = state.long_accumulated.lock().unwrap();
    let was_long = *is_long;
    *is_resting = false;
    *is_long = false;
    *acc = Duration::from_secs(0);
    if was_long {
        *long_acc = Duration::from_secs(0);
    }
}

fn make_show_params(state: &AppState, rest_secs: u64) -> overlay::ShowParams {
    let opacity = state.settings.lock().unwrap().opacity;
    let locale = state.locale.lock().unwrap();
    overlay::ShowParams {
        rest_secs,
        opacity,
        title: i18n::l(&locale, "reminder.title"),
        message: i18n::l(&locale, "reminder.message"),
        rest_info: i18n::l(&locale, "reminder.restInfo"),
        close_label: i18n::l(&locale, "reminder.close"),
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn reload_locale() {
    let lang = app().settings.lock().unwrap().language.clone();
    *app().locale.lock().unwrap() = i18n::load_locale(&lang);
}

// ---- 托盘 ----
unsafe fn load_tray_icon() -> HICON {
    let cx = GetSystemMetrics(SM_CXSMICON);
    let cy = GetSystemMetrics(SM_CYSMICON);
    let offset = LookupIconIdFromDirectoryEx(ICON_ICO.as_ptr(), 1, cx, cy, LR_DEFAULTCOLOR);
    if offset > 0 {
        let icon = CreateIconFromResourceEx(
            ICON_ICO.as_ptr().add(offset as usize),
            (ICON_ICO.len() - offset as usize) as u32,
            1,
            0x00030000,
            cx,
            cy,
            LR_DEFAULTCOLOR,
        );
        if icon != 0 {
            return icon;
        }
    }
    LoadIconW(0, IDI_APPLICATION)
}

unsafe fn tray_add(hwnd: HWND, icon: HICON) {
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_UID;
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAYICON;
    nid.hIcon = icon;
    let tip = wide("EyeProtection");
    let n = tip.len().min(127);
    nid.szTip[..n].copy_from_slice(&tip[..n]);
    Shell_NotifyIconW(NIM_ADD, &nid);
}

unsafe fn tray_set_tip(hwnd: HWND, text: &str) {
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_UID;
    nid.uFlags = NIF_TIP;
    let tip = wide(text);
    let n = tip.len().min(127);
    nid.szTip[..n].copy_from_slice(&tip[..n]);
    Shell_NotifyIconW(NIM_MODIFY, &nid);
}

unsafe fn tray_delete(hwnd: HWND) {
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_UID;
    Shell_NotifyIconW(NIM_DELETE, &nid);
}

unsafe fn show_tray_menu(hwnd: HWND) -> usize {
    let locale = app().locale.lock().unwrap();
    let s_settings = wide(&i18n::l(&locale, "tray.settings"));
    let s_rest = wide(&i18n::l(&locale, "tray.rest_now"));
    let s_about = wide(&i18n::l(&locale, "tray.about"));
    let s_quit = wide(&i18n::l(&locale, "tray.quit"));
    drop(locale);

    let menu = CreatePopupMenu();
    AppendMenuW(menu, MF_STRING, IDM_SETTINGS, s_settings.as_ptr());
    AppendMenuW(menu, MF_STRING, IDM_REST, s_rest.as_ptr());
    AppendMenuW(menu, MF_STRING, IDM_ABOUT, s_about.as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
    AppendMenuW(menu, MF_STRING, IDM_QUIT, s_quit.as_ptr());

    let mut pt: POINT = std::mem::zeroed();
    GetCursorPos(&mut pt);
    SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(
        menu,
        TPM_LEFTALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD,
        pt.x,
        pt.y,
        0,
        hwnd,
        std::ptr::null(),
    );
    DestroyMenu(menu);
    cmd as usize
}

fn about_box() {
    let locale = app().locale.lock().unwrap();
    let title = i18n::l(&locale, "about.title");
    drop(locale);
    unsafe {
        MessageBoxW(
            main_hwnd(),
            wide(&format!("{}\nEyeProtection 0.0.1 (Slint)", title)).as_ptr(),
            wide("EyeProtection").as_ptr(),
            MB_OK,
        );
    }
}

// ---- Slint 设置窗口 ----
fn open_settings() {
    // 复用已存在实例
    let exists = SETTINGS_WIN.with(|w| w.borrow().is_some());
    if exists {
        SETTINGS_WIN.with(|w| {
            if let Some(sw) = w.borrow().as_ref() {
                populate_settings(sw);
                let _ = sw.show();
            }
        });
        return;
    }

    let sw = match SettingsWindow::new() {
        Ok(w) => w,
        Err(_) => return,
    };

    // 注入 i18n 标签
    {
        let locale = app().locale.lock().unwrap();
        sw.set_win_title(i18n::l(&locale, "settings.title").into());
        sw.set_l_work(i18n::l(&locale, "settings.workTime").into());
        sw.set_l_rest(i18n::l(&locale, "settings.restTime").into());
        sw.set_l_lwork(i18n::l(&locale, "settings.longWorkThreshold").into());
        sw.set_l_lrest(i18n::l(&locale, "settings.longRestTime").into());
        sw.set_l_op(i18n::l(&locale, "settings.opacity").into());
        sw.set_l_lang(i18n::l(&locale, "settings.language").into());
        sw.set_l_auto(i18n::l(&locale, "settings.autoStart").into());
        sw.set_l_ok(i18n::l(&locale, "settings.ok").into());
        sw.set_l_apply(i18n::l(&locale, "settings.apply").into());
        sw.set_l_cancel(i18n::l(&locale, "settings.cancel").into());
    }
    populate_settings(&sw);

    // 关闭按钮：隐藏而非退出事件循环
    sw.window()
        .on_close_requested(|| slint::CloseRequestResponse::HideWindow);

    let w_ok = sw.as_weak();
    sw.on_ok(move || {
        if let Some(w) = w_ok.upgrade() {
            save_from_settings(&w);
            let _ = w.hide();
        }
    });
    let w_apply = sw.as_weak();
    sw.on_apply(move || {
        if let Some(w) = w_apply.upgrade() {
            save_from_settings(&w);
        }
    });
    let w_cancel = sw.as_weak();
    sw.on_cancel(move || {
        if let Some(w) = w_cancel.upgrade() {
            let _ = w.hide();
        }
    });

    let _ = sw.show();
    SETTINGS_WIN.with(|slot| *slot.borrow_mut() = Some(sw));
}

fn populate_settings(sw: &SettingsWindow) {
    let s = app().settings.lock().unwrap().clone();
    sw.set_work_time(s.work_time as i32);
    sw.set_rest_time(s.rest_time as i32);
    sw.set_long_work(s.long_work_threshold_mins as i32);
    sw.set_long_rest(s.long_rest_mins as i32);
    sw.set_op_pct((s.opacity * 100.0) as f32);
    sw.set_lang_index(if s.language.starts_with("en") { 1 } else { 0 });
    sw.set_auto_start(s.auto_start);
}

fn save_from_settings(sw: &SettingsWindow) {
    let work = sw.get_work_time().max(1) as u64;
    let new = Settings {
        work_time: work,
        rest_time: sw.get_rest_time().max(1) as u64,
        long_work_threshold_mins: (sw.get_long_work().max(1) as u64).max(work * 2),
        long_rest_mins: sw.get_long_rest().max(1) as u64,
        opacity: (sw.get_op_pct() as f64 / 100.0).clamp(0.0, 1.0),
        auto_start: sw.get_auto_start(),
        language: if sw.get_lang_index() == 1 { "en" } else { "zh-CN" }.to_string(),
    };
    {
        *app().settings.lock().unwrap() = new.clone();
    }
    settings::save(&new);
    settings::set_autostart(new.auto_start);
    reload_locale();
}

// ---- Win32 主窗口过程（托盘/命令/计时器） ----
extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TRAYICON => {
                let evt = (lparam & 0xFFFF) as u32;
                if evt == WM_LBUTTONUP || evt == WM_RBUTTONUP {
                    let cmd = show_tray_menu(hwnd);
                    handle_command(cmd);
                }
                0
            }
            WM_COMMAND => {
                handle_command((wparam & 0xFFFF) as usize);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

unsafe fn handle_command(cmd: usize) {
    match cmd {
        IDM_SETTINGS => open_settings(),
        IDM_REST => {
            let rest_secs = app().settings.lock().unwrap().rest_time * 60;
            *app().is_resting.lock().unwrap() = true;
            overlay::show(make_show_params(app(), rest_secs));
        }
        IDM_ABOUT => about_box(),
        IDM_QUIT => {
            tray_delete(main_hwnd());
            slint::quit_event_loop().ok();
        }
        _ => {}
    }
}

unsafe fn tick() {
    let state = app();
    let now = Instant::now();
    let settings = state.settings.lock().unwrap().clone();
    let last = *state.last_activity.lock().unwrap();

    let mut is_resting = state.is_resting.lock().unwrap();
    let mut is_long = state.is_long_resting.lock().unwrap();
    let mut acc = state.accumulated.lock().unwrap();
    let mut long_acc = state.long_accumulated.lock().unwrap();

    let gap = now.saturating_duration_since(last);
    let rest_threshold = Duration::from_secs(settings.rest_time * 60);

    let mut do_hide = false;
    let mut show_rest: Option<u64> = None;
    let mut tip: Option<String> = None;

    if gap > rest_threshold {
        *acc = Duration::from_secs(0);
        if *is_resting {
            *is_resting = false;
            do_hide = true;
        }
    }

    if !*is_resting && !*is_long {
        if gap <= rest_threshold {
            *acc += Duration::from_secs(1);
            *long_acc += Duration::from_secs(1);
        }

        let total = acc.as_secs();
        let time_str = format!("{:02}:{:02}:{:02}", total / 3600, (total % 3600) / 60, total % 60);
        let locale = state.locale.lock().unwrap();
        let prefix = i18n::l(&locale, "tray.work_timer");
        drop(locale);
        let zh = settings.language == "zh-CN";
        let status = if gap > Duration::from_secs(10) {
            if zh { " (空闲)" } else { " (Idle)" }
        } else if zh {
            " (活跃)"
        } else {
            " (Active)"
        };
        tip = Some(format!("{}: {}{}", prefix, time_str, status));

        let long_threshold = Duration::from_secs(settings.long_work_threshold_mins * 60);
        if *long_acc >= long_threshold {
            *is_long = true;
            show_rest = Some(settings.long_rest_mins * 60);
        } else if *acc >= Duration::from_secs(settings.work_time * 60) {
            *is_resting = true;
            show_rest = Some(settings.rest_time * 60);
        }
    }

    drop(is_resting);
    drop(is_long);
    drop(acc);
    drop(long_acc);

    if let Some(t) = tip {
        tray_set_tip(main_hwnd(), &t);
    }
    if do_hide {
        overlay::hide();
    }
    if let Some(secs) = show_rest {
        overlay::show(make_show_params(state, secs));
    }
}

fn main() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let settings = settings::load();
    let locale = i18n::load_locale(&settings.language);
    let state = Arc::new(AppState {
        settings: Mutex::new(settings),
        last_activity: Mutex::new(Instant::now()),
        accumulated: Mutex::new(Duration::from_secs(0)),
        long_accumulated: Mutex::new(Duration::from_secs(0)),
        is_resting: Mutex::new(false),
        is_long_resting: Mutex::new(false),
        locale: Mutex::new(locale),
    });
    let _ = APP.set(state.clone());

    let overlay_state = state.clone();
    overlay::spawn(move || reset_rest_state(&overlay_state));

    let input_state = state.clone();
    thread::spawn(move || {
        let cb = move |_e: Event| {
            *input_state.last_activity.lock().unwrap() = Instant::now();
        };
        let _ = listen(cb);
    });

    unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        let class_name = wide("EPMainWindow");
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(wnd_proc);
        wc.hInstance = hinst;
        wc.lpszClassName = class_name.as_ptr();
        wc.hCursor = LoadCursorW(0, IDC_ARROW);
        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            wide("EyeProtection").as_ptr(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            hinst,
            std::ptr::null(),
        );
        let _ = MAIN_HWND.set(hwnd);

        let icon = load_tray_icon();
        tray_add(hwnd, icon);
    }

    // 计时用 slint::Timer（在 Slint 事件循环内稳定触发，不依赖 WM_TIMER 派发）
    let tick_timer = slint::Timer::default();
    tick_timer.start(slint::TimerMode::Repeated, Duration::from_secs(1), || unsafe {
        tick();
    });

    slint::run_event_loop().expect("event loop failed");
    // tick_timer 保活至事件循环结束
    drop(tick_timer);
}
