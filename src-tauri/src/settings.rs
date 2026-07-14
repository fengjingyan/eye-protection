// 设置项与持久化（纯 Rust，无 Tauri）。
// 存储路径沿用原应用：%APPDATA%\com.eyeprotection.app\settings.json，
// 因此不同分支/版本之间用户设置可互通。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    pub work_time: u64,                // minutes — 连续工作时间
    pub rest_time: u64,                // minutes — 短暂休息时长
    pub long_work_threshold_mins: u64, // minutes — 累计工作阈值（触发长时休息）
    pub long_rest_mins: u64,           // minutes — 长时休息时长
    pub opacity: f64,
    pub auto_start: bool,
    pub language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            work_time: 10,
            rest_time: 1,
            long_work_threshold_mins: 60,
            long_rest_mins: 5,
            opacity: 0.8,
            auto_start: false,
            language: "zh-CN".to_string(),
        }
    }
}

pub fn settings_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(|a| PathBuf::from(a).join("com.eyeprotection.app").join("settings.json"))
}

pub fn load() -> Settings {
    if let Some(p) = settings_path() {
        if let Ok(s) = std::fs::read_to_string(&p) {
            if let Ok(v) = serde_json::from_str::<Settings>(&s) {
                return v;
            }
        }
    }
    Settings::default()
}

pub fn save(s: &Settings) {
    if let Some(p) = settings_path() {
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(txt) = serde_json::to_string(s) {
            let _ = std::fs::write(p, txt);
        }
    }
}

// Windows 开机自启：写入/删除 HKCU Run 项
#[cfg(target_os = "windows")]
pub fn set_autostart(enable: bool) {
    use winreg::enums::*;
    use winreg::RegKey;

    if let Ok(exe_path) = std::env::current_exe() {
        let exe_str = exe_path.display().to_string();
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok((run_key, _)) =
            hkcu.create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
        {
            if enable {
                let _ = run_key.set_value("EyeProtection", &format!("\"{}\"", exe_str));
            } else {
                let _ = run_key.delete_value("EyeProtection");
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn set_autostart(_enable: bool) {}
