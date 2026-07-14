// 原生 Win32 设置窗口：数字输入 + 透明度滑块 + 语言下拉 + 自启复选 + 确定/应用/取消。
#![cfg(target_os = "windows")]

use std::cell::RefCell;

use crate::{app, i18n, settings, wide};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateFontW, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH,
    FF_DONTCARE, FW_NORMAL, OUT_DEFAULT_PRECIS,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{
    InitCommonControlsEx, ICC_BAR_CLASSES, INITCOMMONCONTROLSEX,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowTextW, IsWindow, LoadCursorW,
    MessageBoxW, RegisterClassW, SendMessageW, SetForegroundWindow, SetWindowTextW, ShowWindow,
    BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL,
    CBS_DROPDOWNLIST, ES_NUMBER, IDC_ARROW, MB_OK, SW_SHOW, WM_COMMAND, WM_DESTROY, WM_SETFONT,
    WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

// 部分控件消息/常量在 windows-sys 中类型不一或未导出，这里直接按 Win32 定义
const TBM_SETRANGE: u32 = 0x0406; // WM_USER + 6
const TBM_SETPOS: u32 = 0x0405; // WM_USER + 5
const TBM_GETPOS: u32 = 0x0400; // WM_USER + 0
const BST_CHECKED: usize = 1;

const ID_WORK: usize = 101;
const ID_REST: usize = 102;
const ID_LONGWORK: usize = 103;
const ID_LONGREST: usize = 104;
const ID_OPACITY: usize = 105;
const ID_LANG: usize = 106;
const ID_AUTO: usize = 107;
const ID_OK: usize = 108;
const ID_APPLY: usize = 109;
const ID_CANCEL: usize = 110;

struct Ctx {
    hwnd: HWND,
    work: HWND,
    rest: HWND,
    longwork: HWND,
    longrest: HWND,
    opacity: HWND,
    lang: HWND,
    auto: HWND,
}

thread_local! {
    static SCTX: RefCell<Option<Ctx>> = RefCell::new(None);
    static SFONT: RefCell<isize> = RefCell::new(0);
    static REGISTERED: RefCell<bool> = RefCell::new(false);
}

const CLASS: &str = "EPSettingsWindow";

fn font() -> isize {
    SFONT.with(|f| {
        let mut v = f.borrow_mut();
        if *v == 0 {
            let face = wide("Microsoft YaHei");
            *v = unsafe {
                CreateFontW(
                    -16, 0, 0, 0, FW_NORMAL as i32, 0, 0, 0,
                    DEFAULT_CHARSET as u32, OUT_DEFAULT_PRECIS as u32, CLIP_DEFAULT_PRECIS as u32,
                    CLEARTYPE_QUALITY as u32, (DEFAULT_PITCH | FF_DONTCARE) as u32, face.as_ptr(),
                )
            };
        }
        *v
    })
}

unsafe fn mk(parent: HWND, class: &str, text: &str, style: u32, x: i32, y: i32, w: i32, h: i32, id: usize) -> HWND {
    let hinst = GetModuleHandleW(std::ptr::null());
    let hwnd = CreateWindowExW(
        0,
        wide(class).as_ptr(),
        wide(text).as_ptr(),
        WS_CHILD | WS_VISIBLE | style,
        x, y, w, h,
        parent,
        id as isize,
        hinst,
        std::ptr::null(),
    );
    SendMessageW(hwnd, WM_SETFONT, font() as WPARAM, 1);
    hwnd
}

fn set_text(h: HWND, s: &str) {
    unsafe { SetWindowTextW(h, wide(s).as_ptr()); }
}

fn get_u64(h: HWND) -> u64 {
    let mut buf = [0u16; 32];
    let n = unsafe { GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32) };
    let s = String::from_utf16_lossy(&buf[..n as usize]);
    s.trim().parse::<u64>().unwrap_or(0)
}

pub fn open(main_hwnd: HWND) {
    // 已打开则前置
    let existing = SCTX.with(|c| c.borrow().as_ref().map(|x| x.hwnd));
    if let Some(h) = existing {
        if unsafe { IsWindow(h) } != 0 {
            unsafe { SetForegroundWindow(h); }
            return;
        }
    }

    unsafe {
        let mut icc: INITCOMMONCONTROLSEX = std::mem::zeroed();
        icc.dwSize = std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32;
        icc.dwICC = ICC_BAR_CLASSES;
        InitCommonControlsEx(&icc);

        let hinst = GetModuleHandleW(std::ptr::null());
        REGISTERED.with(|r| {
            let mut reg = r.borrow_mut();
            if !*reg {
                let cls = wide(CLASS); // 必须在 RegisterClassW 调用前保持存活
                let mut wc: WNDCLASSW = std::mem::zeroed();
                wc.lpfnWndProc = Some(wnd_proc);
                wc.hInstance = hinst;
                wc.lpszClassName = cls.as_ptr();
                wc.hCursor = LoadCursorW(0, IDC_ARROW);
                // 背景用系统对话框面色 (COLOR_BTNFACE=15) + 1
                wc.hbrBackground = 16 as isize;
                RegisterClassW(&wc);
                *reg = true;
            }
        });

        let locale = app().locale.lock().unwrap();
        let title = i18n::l(&locale, "settings.title");
        let l_work = i18n::l(&locale, "settings.workTime");
        let l_rest = i18n::l(&locale, "settings.restTime");
        let l_lwork = i18n::l(&locale, "settings.longWorkThreshold");
        let l_lrest = i18n::l(&locale, "settings.longRestTime");
        let l_op = i18n::l(&locale, "settings.opacity");
        let l_lang = i18n::l(&locale, "settings.language");
        let l_auto = i18n::l(&locale, "settings.autoStart");
        let l_ok = i18n::l(&locale, "settings.ok");
        let l_apply = i18n::l(&locale, "settings.apply");
        let l_cancel = i18n::l(&locale, "settings.cancel");
        drop(locale);

        let (wx, wy) = crate::center_xy(420, 520);
        let hwnd = CreateWindowExW(
            0,
            wide(CLASS).as_ptr(),
            wide(&title).as_ptr(),
            WS_CAPTION | WS_SYSMENU,
            wx,
            wy,
            420,
            520,
            main_hwnd,
            0,
            hinst,
            std::ptr::null(),
        );

        // 布局
        let lx = 16;
        let cx = 210;
        let lw = 185;
        let cw = 180;
        let mut y = 16;
        let rowh = 30;
        let gap = 12;
        let mklabel = |text: &str, yy: i32| mk(hwnd, "STATIC", text, 0, lx, yy + 4, lw, 22, 0);
        let edit_style = WS_BORDER | WS_TABSTOP | ES_NUMBER as u32;
        let combo_style = CBS_DROPDOWNLIST as u32 | WS_VSCROLL | WS_TABSTOP;
        let check_style = BS_AUTOCHECKBOX as u32 | WS_TABSTOP;

        mklabel(&l_work, y);
        let work = mk(hwnd, "EDIT", "", edit_style, cx, y, cw, 24, ID_WORK);
        y += rowh + gap;
        mklabel(&l_rest, y);
        let rest = mk(hwnd, "EDIT", "", edit_style, cx, y, cw, 24, ID_REST);
        y += rowh + gap;
        mklabel(&l_lwork, y);
        let longwork = mk(hwnd, "EDIT", "", edit_style, cx, y, cw, 24, ID_LONGWORK);
        y += rowh + gap;
        mklabel(&l_lrest, y);
        let longrest = mk(hwnd, "EDIT", "", edit_style, cx, y, cw, 24, ID_LONGREST);
        y += rowh + gap;
        mklabel(&l_op, y);
        let opacity = mk(hwnd, "msctls_trackbar32", "", WS_TABSTOP, cx, y, cw, 28, ID_OPACITY);
        SendMessageW(opacity, TBM_SETRANGE, 1, (0i32 | (100i32 << 16)) as LPARAM);
        y += rowh + gap;
        mklabel(&l_lang, y);
        let lang = mk(hwnd, "COMBOBOX", "", combo_style, cx, y, cw, 200, ID_LANG);
        SendMessageW(lang, CB_ADDSTRING, 0, wide("zh-CN").as_ptr() as LPARAM);
        SendMessageW(lang, CB_ADDSTRING, 0, wide("en").as_ptr() as LPARAM);
        y += rowh + gap;
        let auto = mk(hwnd, "BUTTON", &l_auto, check_style, lx, y, 380, 24, ID_AUTO);
        y += rowh + gap + 8;

        // 按钮
        mk(hwnd, "BUTTON", &l_ok, WS_TABSTOP, 60, y, 90, 30, ID_OK);
        mk(hwnd, "BUTTON", &l_apply, WS_TABSTOP, 165, y, 90, 30, ID_APPLY);
        mk(hwnd, "BUTTON", &l_cancel, WS_TABSTOP, 270, y, 90, 30, ID_CANCEL);

        let ctx = Ctx { hwnd, work, rest, longwork, longrest, opacity, lang, auto };
        populate(&ctx);
        SCTX.with(|c| *c.borrow_mut() = Some(ctx));

        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
    }
}

fn populate(ctx: &Ctx) {
    let s = app().settings.lock().unwrap().clone();
    set_text(ctx.work, &s.work_time.to_string());
    set_text(ctx.rest, &s.rest_time.to_string());
    set_text(ctx.longwork, &s.long_work_threshold_mins.to_string());
    set_text(ctx.longrest, &s.long_rest_mins.to_string());
    unsafe {
        SendMessageW(ctx.opacity, TBM_SETPOS, 1, (s.opacity * 100.0) as LPARAM);
        let idx = if s.language.starts_with("en") { 1 } else { 0 };
        SendMessageW(ctx.lang, CB_SETCURSEL, idx, 0);
        SendMessageW(ctx.auto, BM_SETCHECK, if s.auto_start { BST_CHECKED as WPARAM } else { 0 }, 0);
    }
}

// 收集控件值并校验；成功则返回 Settings
fn collect(ctx: &Ctx) -> Option<settings::Settings> {
    let work = get_u64(ctx.work).max(0);
    let rest = get_u64(ctx.rest);
    let longwork = get_u64(ctx.longwork);
    let longrest = get_u64(ctx.longrest);
    let opacity = (unsafe { SendMessageW(ctx.opacity, TBM_GETPOS, 0, 0) } as f64) / 100.0;
    let lang_idx = unsafe { SendMessageW(ctx.lang, CB_GETCURSEL, 0, 0) };
    let language = if lang_idx == 1 { "en" } else { "zh-CN" }.to_string();
    let auto_start = unsafe { SendMessageW(ctx.auto, BM_GETCHECK, 0, 0) } == BST_CHECKED as isize;

    if work < 1 || rest < 1 || longrest < 1 {
        warn(ctx.hwnd, "工作/休息时长必须 ≥ 1 分钟");
        return None;
    }
    if longwork < work * 2 {
        warn(ctx.hwnd, "累计工作阈值必须 ≥ 连续工作时间的 2 倍");
        return None;
    }

    Some(settings::Settings {
        work_time: work,
        rest_time: rest,
        long_work_threshold_mins: longwork,
        long_rest_mins: longrest,
        opacity: opacity.clamp(0.0, 1.0),
        auto_start,
        language,
    })
}

fn warn(hwnd: HWND, msg: &str) {
    unsafe { MessageBoxW(hwnd, wide(msg).as_ptr(), wide("EyeProtection").as_ptr(), MB_OK); }
}

// 应用设置：写内存/磁盘、自启、刷新 locale
fn apply(new: settings::Settings) {
    {
        let mut s = app().settings.lock().unwrap();
        *s = new.clone();
    }
    settings::save(&new);
    settings::set_autostart(new.auto_start);
    crate::reload_locale();
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_COMMAND => {
                let id = (wparam & 0xFFFF) as usize;
                let notif = ((wparam >> 16) & 0xFFFF) as u32;
                if notif == BN_CLICKED {
                    match id {
                        ID_OK => {
                            if let Some(new) = SCTX.with(|c| c.borrow().as_ref().and_then(collect)) {
                                apply(new);
                                DestroyWindow(hwnd);
                            }
                        }
                        ID_APPLY => {
                            if let Some(new) = SCTX.with(|c| c.borrow().as_ref().and_then(collect)) {
                                apply(new);
                            }
                        }
                        ID_CANCEL => {
                            DestroyWindow(hwnd);
                        }
                        _ => {}
                    }
                }
                0
            }
            WM_DESTROY => {
                SCTX.with(|c| *c.borrow_mut() = None);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
