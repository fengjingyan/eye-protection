#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

//! 纯 Win32 版护眼程序（方案2）。完全去除 Tauri/WebView：
//! - 托盘图标 + 菜单：Shell_NotifyIcon + TrackPopupMenu
//! - 休息提示：原生 Win32 分层窗口（overlay.rs）
//! - 设置/关于：原生窗口（settings_window.rs / about_window.rs）
//! - 计时逻辑在主窗口 WM_TIMER 中执行；全局输入监听用 rdev 线程。

mod about_window;
mod i18n;
mod overlay;
mod settings;
mod settings_window;

use serde_json::Value;
use settings::Settings;
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
    DestroyMenu, DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, KillTimer,
    LoadCursorW, LoadIconW, LookupIconIdFromDirectoryEx, PostQuitMessage, RegisterClassW,
    SetForegroundWindow, SetTimer, TrackPopupMenu, TranslateMessage, HICON, IDC_ARROW,
    IDI_APPLICATION, LR_DEFAULTCOLOR, MF_SEPARATOR, MF_STRING, MSG, SM_CXSMICON, SM_CYSMICON,
    TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_COMMAND, WM_LBUTTONUP, WM_RBUTTONUP,
    WM_TIMER, WNDCLASSW,
};

// ---- 常量 ----
const WM_TRAYICON: u32 = WM_APP + 0x100;
const TRAY_UID: u32 = 1;
const TIMER_ID: usize = 1;
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

// 单实例：主窗口过程通过它访问状态
static APP: OnceLock<Arc<AppState>> = OnceLock::new();
pub(crate) fn app() -> &'static Arc<AppState> {
    APP.get().expect("APP not initialized")
}

// 休息结束（倒计时归零/用户结束）时的状态重置，供 overlay 回调复用
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

pub fn make_show_params(state: &AppState, rest_secs: u64) -> overlay::ShowParams {
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

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// 计算窗口在主屏居中的左上角坐标
pub(crate) fn center_xy(w: i32, h: i32) -> (i32, i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    unsafe {
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);
        (((sw - w) / 2).max(0), ((sh - h) / 2).max(0))
    }
}

// ---- 托盘图标 ----
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

// 弹出托盘菜单，返回被选中的命令 id（0 表示未选）
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

// 语言切换后刷新缓存 locale（托盘/浮层下次取用即生效）
pub fn reload_locale() {
    let lang = app().settings.lock().unwrap().language.clone();
    let v = i18n::load_locale(&lang);
    *app().locale.lock().unwrap() = v;
}

// ---- 主窗口过程 ----
extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TRAYICON => {
                let evt = (lparam & 0xFFFF) as u32;
                if evt == WM_LBUTTONUP || evt == WM_RBUTTONUP {
                    let cmd = show_tray_menu(hwnd);
                    handle_command(hwnd, cmd);
                }
                0
            }
            WM_COMMAND => {
                handle_command(hwnd, (wparam & 0xFFFF) as usize);
                0
            }
            WM_TIMER => {
                if wparam == TIMER_ID {
                    tick(hwnd);
                }
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

unsafe fn handle_command(hwnd: HWND, cmd: usize) {
    match cmd {
        IDM_SETTINGS => {
            settings_window::open(hwnd);
        }
        IDM_REST => {
            let rest_secs = app().settings.lock().unwrap().rest_time * 60;
            *app().is_resting.lock().unwrap() = true;
            overlay::show(make_show_params(app(), rest_secs));
        }
        IDM_ABOUT => {
            about_window::open(hwnd);
        }
        IDM_QUIT => {
            tray_delete(hwnd);
            KillTimer(hwnd, TIMER_ID);
            PostQuitMessage(0);
        }
        _ => {}
    }
}

// 每秒计时逻辑（对齐原实现）
unsafe fn tick(hwnd: HWND) {
    let state = app();
    let now = Instant::now();
    let settings = state.settings.lock().unwrap().clone();
    let last = *state.last_activity.lock().unwrap();

    let mut is_resting = state.is_resting.lock().unwrap();
    let mut is_long = state.is_long_resting.lock().unwrap();
    let mut acc = state.accumulated.lock().unwrap();
    let mut long_acc = state.long_accumulated.lock().unwrap();

    // saturating: 避免 rdev 线程在 now 与 last 之间更新 last_activity 导致 last>now 而 panic
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
        tray_set_tip(hwnd, &t);
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

    // 原生休息浮层线程
    let overlay_state = state.clone();
    overlay::spawn(move || reset_rest_state(&overlay_state));

    // 全局输入监听
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

        // 隐藏的顶层窗口，用于承载托盘图标与消息
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

        let icon = load_tray_icon();
        tray_add(hwnd, icon);
        SetTimer(hwnd, TIMER_ID, 1000, None);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, 0, 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
