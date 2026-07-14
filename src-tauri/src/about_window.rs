// 原生关于窗口。占位实现：先弹消息框，随后替换为真正的窗口。
#![cfg(target_os = "windows")]

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK};

pub fn open(hwnd: HWND) {
    let locale = crate::app().locale.lock().unwrap();
    let title = crate::i18n::l(&locale, "about.title");
    drop(locale);
    let text = crate::wide(&format!("{}\nEyeProtection 0.0.1", title));
    let cap = crate::wide("EyeProtection");
    unsafe {
        MessageBoxW(hwnd, text.as_ptr(), cap.as_ptr(), MB_OK);
    }
}
