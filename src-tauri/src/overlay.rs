// 原生 Win32 全屏休息提示浮层（替代 Tauri WebView 休息窗，消除 WebView2 内存开销）。
//
// 设计要点：
// - 单独一个线程运行 Win32 消息循环，创建每个显示器一个 WS_EX_LAYERED 分层窗口。
// - 主线程通过 PostMessage 向该线程的“控制器窗口”发送 显示/隐藏 指令。
// - 倒计时结束或用户点击“结束休息”/按 Esc 时，调用 on_end 回调（复用主程序的状态重置逻辑）。
// - 整窗统一透明度（SetLayeredWindowAttributes + LWA_ALPHA），对齐原 rgba teal 背景的观感。
//
// 使用 windows-sys（原生 FFI，句柄均为 isize，常量为整型），故大量 unsafe。

#![cfg(target_os = "windows")]

use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Mutex;

use windows_sys::Win32::Foundation::{
    BOOL, COLORREF, HWND, LPARAM, LRESULT, RECT, SYSTEMTIME, TRUE, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, EnumDisplayMonitors,
    FillRect, FrameRect, InvalidateRect, SelectObject, SetBkMode, SetTextColor, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE,
    DT_VCENTER, FF_DONTCARE, FW_BOLD, FW_NORMAL, HDC, HMONITOR, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW,
    KillTimer, LoadCursorW, PostMessageW, RegisterClassW, SetForegroundWindow,
    SetLayeredWindowAttributes, SetTimer, ShowWindow, TranslateMessage, HWND_MESSAGE, IDC_ARROW,
    LWA_ALPHA, MSG, SW_SHOW, WM_APP, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_PAINT, WM_TIMER,
    WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

// ---- 主线程 <-> 浮层线程 通信 ----

const WM_APP_SHOW: u32 = WM_APP + 1;
const WM_APP_HIDE: u32 = WM_APP + 2;
const WM_APP_END: u32 = WM_APP + 3; // 用户点击/按键触发的结束
const TIMER_ID: usize = 1;

/// 一次休息展示所需的全部参数（可跨线程传递）。
pub struct ShowParams {
    pub rest_secs: u64,
    pub opacity: f64,
    pub title: String,
    pub message: String,
    pub rest_info: String, // 可含 \n 分多行
    pub close_label: String,
}

// 控制器窗口句柄（isize，跨线程安全）。0 表示尚未就绪。
static CONTROLLER: Mutex<isize> = Mutex::new(0);
static PENDING_SHOW: Mutex<Option<ShowParams>> = Mutex::new(None);
// 浮层窗口类的 ATOM（RegisterClassW 返回），供 WM_APP_SHOW 创建窗口时使用。
static OVERLAY_ATOM: AtomicUsize = AtomicUsize::new(0);

fn controller_hwnd() -> HWND {
    *CONTROLLER.lock().unwrap()
}

/// 启动浮层线程。`on_end` 会在“休息结束”（倒计时归零或用户主动结束）时于浮层线程被调用。
pub fn spawn<F: Fn() + Send + 'static>(on_end: F) {
    let (ready_tx, ready_rx) = mpsc::channel::<isize>();
    std::thread::spawn(move || {
        RT.with(|rt| rt.borrow_mut().on_end = Some(Box::new(on_end)));
        let controller = unsafe { init_windows() };
        let _ = ready_tx.send(controller);
        // 消息循环
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, 0, 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    });
    if let Ok(h) = ready_rx.recv() {
        *CONTROLLER.lock().unwrap() = h;
    }
}

/// 显示休息浮层。
pub fn show(params: ShowParams) {
    *PENDING_SHOW.lock().unwrap() = Some(params);
    let h = controller_hwnd();
    if h != 0 {
        unsafe {
            PostMessageW(h, WM_APP_SHOW, 0, 0);
        }
    }
}

/// 隐藏休息浮层（不触发 on_end；用于主程序已自行重置状态的场景，如空闲结束）。
pub fn hide() {
    let h = controller_hwnd();
    if h != 0 {
        unsafe {
            PostMessageW(h, WM_APP_HIDE, 0, 0);
        }
    }
}

// ---- 浮层线程内部运行时状态（thread-local，仅浮层线程访问） ----

#[derive(Default)]
struct Strings {
    title: String,
    message: String,
    rest_info_lines: Vec<String>,
    close_label: String,
}

struct Rt {
    on_end: Option<Box<dyn Fn()>>,
    windows: Vec<HWND>,
    remaining: i64,
    ended: bool,
    strings: Strings,
}

impl Rt {
    fn new() -> Self {
        Rt {
            on_end: None,
            windows: Vec::new(),
            remaining: 0,
            ended: true,
            strings: Strings::default(),
        }
    }
}

thread_local! {
    static RT: RefCell<Rt> = RefCell::new(Rt::new());
}

// ---- Win32 初始化与窗口过程 ----

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn init_windows() -> HWND {
    let hinst = GetModuleHandleW(std::ptr::null());
    let cursor = LoadCursorW(0, IDC_ARROW);

    let controller_name = wide("EPOverlayController");
    let mut controller_class: WNDCLASSW = std::mem::zeroed();
    controller_class.lpfnWndProc = Some(controller_proc);
    controller_class.hInstance = hinst;
    controller_class.lpszClassName = controller_name.as_ptr();
    let controller_atom = RegisterClassW(&controller_class);

    let overlay_name = wide("EPOverlayWindow");
    let mut overlay_class: WNDCLASSW = std::mem::zeroed();
    overlay_class.lpfnWndProc = Some(overlay_proc);
    overlay_class.hInstance = hinst;
    overlay_class.lpszClassName = overlay_name.as_ptr();
    overlay_class.hCursor = cursor;
    let overlay_atom = RegisterClassW(&overlay_class);
    OVERLAY_ATOM.store(overlay_atom as usize, Ordering::SeqCst);

    // 消息专用（message-only）控制器窗口
    CreateWindowExW(
        0,
        controller_atom as usize as *const u16,
        std::ptr::null(),
        0,
        0,
        0,
        0,
        0,
        HWND_MESSAGE,
        0,
        hinst,
        std::ptr::null(),
    )
}

// 收集所有显示器的物理矩形
unsafe extern "system" fn monitor_enum_proc(
    _hmon: HMONITOR,
    _hdc: HDC,
    rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let rects = &mut *(lparam as *mut Vec<RECT>);
    rects.push(*rect);
    TRUE
}

fn enum_monitor_rects() -> Vec<RECT> {
    let mut rects: Vec<RECT> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            0,
            std::ptr::null(),
            Some(monitor_enum_proc),
            &mut rects as *mut _ as LPARAM,
        );
    }
    if rects.is_empty() {
        rects.push(RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        });
    }
    rects
}

unsafe fn destroy_overlay_windows(rt: &mut Rt) {
    for h in rt.windows.drain(..) {
        DestroyWindow(h);
    }
}

// 结束休息：销毁窗口、停计时器，可选触发回调。
// on_end 始终保留（不 take），可跨多次休息复用；ended 标志防止重复触发。
unsafe fn end_rest(controller: HWND, call_on_end: bool) {
    let should_call = RT.with(|rt| {
        let mut r = rt.borrow_mut();
        if r.ended {
            return false;
        }
        r.ended = true;
        destroy_overlay_windows(&mut r);
        call_on_end
    });
    KillTimer(controller, TIMER_ID);
    if should_call {
        // 回调只操作 AppState，不触碰 RT，无重入风险
        RT.with(|rt| {
            if let Some(cb) = rt.borrow().on_end.as_ref() {
                cb();
            }
        });
    }
}

unsafe extern "system" fn controller_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_APP_SHOW => {
            let params = PENDING_SHOW.lock().unwrap().take();
            if let Some(p) = params {
                let alpha = (p.opacity.clamp(0.0, 1.0) * 255.0) as u8;
                RT.with(|rt| {
                    let mut r = rt.borrow_mut();
                    destroy_overlay_windows(&mut r);
                    r.remaining = p.rest_secs as i64;
                    r.ended = false;
                    r.strings = Strings {
                        title: p.title,
                        message: p.message,
                        rest_info_lines: p
                            .rest_info
                            .split('\n')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect(),
                        close_label: p.close_label,
                    };
                });

                let rects = enum_monitor_rects();
                let hinst = GetModuleHandleW(std::ptr::null());
                let atom = OVERLAY_ATOM.load(Ordering::SeqCst);
                let mut first: HWND = 0;
                for rc in rects {
                    let w = rc.right - rc.left;
                    let h = rc.bottom - rc.top;
                    let win = CreateWindowExW(
                        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                        atom as *const u16,
                        std::ptr::null(),
                        WS_POPUP,
                        rc.left,
                        rc.top,
                        w,
                        h,
                        0,
                        0,
                        hinst,
                        std::ptr::null(),
                    );
                    if win == 0 {
                        continue;
                    }
                    SetLayeredWindowAttributes(win, 0, alpha, LWA_ALPHA);
                    ShowWindow(win, SW_SHOW);
                    if first == 0 {
                        first = win;
                    }
                    RT.with(|rt| rt.borrow_mut().windows.push(win));
                }
                if first != 0 {
                    SetForegroundWindow(first);
                }
                SetTimer(hwnd, TIMER_ID, 1000, None);
            }
            0
        }
        WM_APP_HIDE => {
            end_rest(hwnd, false);
            0
        }
        WM_APP_END => {
            end_rest(hwnd, true);
            0
        }
        WM_TIMER => {
            let done = RT.with(|rt| {
                let mut r = rt.borrow_mut();
                if r.ended {
                    return true;
                }
                r.remaining -= 1;
                r.remaining < 0
            });
            if done {
                end_rest(hwnd, true);
            } else {
                invalidate_all();
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn invalidate_all() {
    RT.with(|rt| {
        for h in &rt.borrow().windows {
            unsafe {
                InvalidateRect(*h, std::ptr::null(), 0);
            }
        }
    });
}

unsafe extern "system" fn overlay_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => 1, // 全部在 WM_PAINT 里画，避免闪烁
        WM_PAINT => {
            paint_overlay(hwnd);
            0
        }
        WM_LBUTTONDOWN => {
            // 命中“结束休息”按钮才结束
            let x = (lparam & 0xFFFF) as i16 as i32;
            let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            let mut client: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut client);
            let btn = button_rect(&client);
            let hit = x >= btn.left && x <= btn.right && y >= btn.top && y <= btn.bottom;
            if hit {
                let c = controller_hwnd();
                if c != 0 {
                    PostMessageW(c, WM_APP_END, 0, 0);
                }
            }
            0
        }
        WM_KEYDOWN => {
            if wparam as u16 == VK_ESCAPE {
                let c = controller_hwnd();
                if c != 0 {
                    PostMessageW(c, WM_APP_END, 0, 0);
                }
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// ---- 绘制 ----

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

// 按钮矩形（相对于窗口客户区中心的固定位置，绘制与命中测试共用）
fn button_rect(client: &RECT) -> RECT {
    let cw = client.right - client.left;
    let ch = client.bottom - client.top;
    let scale = (ch as f32 / 1080.0).max(0.5);
    let bw = (260.0 * scale) as i32;
    let bh = (64.0 * scale) as i32;
    let cx = cw / 2;
    let cy = ch / 2 + (200.0 * scale) as i32;
    RECT {
        left: cx - bw / 2,
        top: cy - bh / 2,
        right: cx + bw / 2,
        bottom: cy + bh / 2,
    }
}

unsafe fn make_font(px: i32, bold: bool) -> isize {
    let face = wide("Microsoft YaHei");
    CreateFontW(
        -px,
        0,
        0,
        0,
        if bold { FW_BOLD as i32 } else { FW_NORMAL as i32 },
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        CLEARTYPE_QUALITY as u32,
        (DEFAULT_PITCH | FF_DONTCARE) as u32,
        face.as_ptr(),
    )
}

// 在以 y 为竖直中心、跨整行宽度的区域内居中绘制一行文本
unsafe fn draw_line(hdc: HDC, cw: i32, y: i32, band: i32, text: &str, color: COLORREF, font: isize) {
    let old = SelectObject(hdc, font);
    SetTextColor(hdc, color);
    SetBkMode(hdc, TRANSPARENT as i32);
    let mut rc = RECT {
        left: 0,
        top: y - band / 2,
        right: cw,
        bottom: y + band / 2,
    };
    let mut buf: Vec<u16> = text.encode_utf16().collect();
    let len = buf.len() as i32;
    DrawTextW(
        hdc,
        buf.as_mut_ptr(),
        len,
        &mut rc,
        DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
    );
    SelectObject(hdc, old);
}

fn now_hms() -> String {
    let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut st) };
    format!("{:02}:{:02}:{:02}", st.wHour, st.wMinute, st.wSecond)
}

unsafe fn paint_overlay(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);

    let mut client: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut client);
    let cw = client.right - client.left;
    let ch = client.bottom - client.top;
    let scale = (ch as f32 / 1080.0).max(0.5);

    // 背景：teal 实心（整窗透明度由分层属性统一施加）
    let bg = CreateSolidBrush(rgb(0, 128, 128));
    FillRect(hdc, &client, bg);
    DeleteObject(bg);

    let cx = cw / 2;
    let cy = ch / 2;
    let _ = cx;

    let (remaining, s_title, s_msg, s_lines, s_close) = RT.with(|rt| {
        let r = rt.borrow();
        (
            r.remaining.max(0),
            r.strings.title.clone(),
            r.strings.message.clone(),
            r.strings.rest_info_lines.clone(),
            r.strings.close_label.clone(),
        )
    });

    let white = rgb(255, 255, 255);
    let yellow = rgb(255, 235, 59);

    let f_title = make_font((48.0 * scale) as i32, true);
    let f_msg = make_font((24.0 * scale) as i32, false);
    let f_info = make_font((18.0 * scale) as i32, false);
    let f_time = make_font((36.0 * scale) as i32, true);
    let f_count = make_font((72.0 * scale) as i32, true);
    let f_btn = make_font((20.0 * scale) as i32, false);

    let sc = |v: f32| (v * scale) as i32;

    draw_line(hdc, cw, cy - sc(260.0), sc(70.0), &s_title, white, f_title);
    draw_line(hdc, cw, cy - sc(180.0), sc(40.0), &s_msg, white, f_msg);
    let mut info_y = cy - sc(120.0);
    for line in &s_lines {
        draw_line(hdc, cw, info_y, sc(30.0), line, white, f_info);
        info_y += sc(34.0);
    }
    draw_line(hdc, cw, cy - sc(20.0), sc(50.0), &now_hms(), yellow, f_time);
    let mm = remaining / 60;
    let ss = remaining % 60;
    let count = format!("{:02}:{:02}", mm, ss);
    draw_line(hdc, cw, cy + sc(80.0), sc(100.0), &count, yellow, f_count);

    // 按钮：白框 + 文案
    let btn = button_rect(&client);
    let white_brush = CreateSolidBrush(white);
    FrameRect(hdc, &btn, white_brush);
    DeleteObject(white_brush);
    let btn_cy = (btn.top + btn.bottom) / 2;
    draw_line(hdc, cw, btn_cy, btn.bottom - btn.top, &s_close, white, f_btn);

    DeleteObject(f_title);
    DeleteObject(f_msg);
    DeleteObject(f_info);
    DeleteObject(f_time);
    DeleteObject(f_count);
    DeleteObject(f_btn);

    EndPaint(hwnd, &ps);
}
