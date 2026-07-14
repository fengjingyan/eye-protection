# EyeProtection （护眼程序 · 纯 Win32 分支）

本 `win32` 分支是护眼程序的**纯原生 Win32 实现**，完全去除 Tauri 与 WebView，追求最低运行时内存。UI 全部用 Win32 API 手写，非 UI 逻辑（计时状态机、设置持久化、开机自启、全局输入监听、i18n）从 Tauri 版移植而来。

> 其它分支：`master`（原始 Tauri/WebView 版）、`mix-frontend`（混合：休息窗原生、设置/关于仍用 Tauri WebView）。三者用于内存对比。

## 内存实测（本分支）

| 状态 | 纯 Win32 (本分支) | 混合(mix-frontend) | 原始 Tauri (master) |
|---|---|---|---|
| 空闲（工作中） | **~9 MB** | ~11 MB | ~300 MB |
| 休息弹窗期间 | **~14 MB** | ~14 MB | ~300–500 MB |
| 进程数 | 单进程，无 WebView2 子进程 | 单进程 | 主进程 + 多个 msedgewebview2 |

## 架构与文件

- [src-tauri/src/main.rs](src-tauri/src/main.rs) — 主逻辑：托盘图标/菜单（Shell_NotifyIcon + TrackPopupMenu）、每秒计时（WM_TIMER）、rdev 全局输入监听、消息循环。
- [src-tauri/src/overlay.rs](src-tauri/src/overlay.rs) — 原生 Win32 全屏休息浮层（独立线程 + 分层窗口 + GDI 绘制倒计时）。
- [src-tauri/src/settings_window.rs](src-tauri/src/settings_window.rs) — 原生设置窗口（EDIT/TRACKBAR/COMBOBOX/CHECKBOX 控件）。
- [src-tauri/src/about_window.rs](src-tauri/src/about_window.rs) — 关于窗口（当前为原生 MessageBox）。
- [src-tauri/src/settings.rs](src-tauri/src/settings.rs) — 设置项与持久化（`%APPDATA%\com.eyeprotection.app\settings.json`）。
- [src-tauri/src/i18n.rs](src-tauri/src/i18n.rs) — 国际化，locale JSON 编译期嵌入（`include_str!`，无需外部文件）。
- [ui/i18n](ui/i18n) — 语言源文件（zh-CN.json / en.json），编译时被嵌入二进制。

> `src-tauri/` 目录名与 `tauri.conf.json` 为历史遗留，本分支不再使用 Tauri；`ui/` 下的 HTML/JS 也不再参与构建（仅 i18n JSON 被嵌入）。

## 开发环境要求

- Rust（stable，通过 rustup 安装）
- **仅 Windows**：UI 使用 Win32 API（`windows-sys`）。
- 无需 Node.js / npm / Tauri CLI / WiX。

## 编译与运行

纯 Cargo 构建，无前端构建步骤：

```powershell
# 调试
cargo build --manifest-path src-tauri/Cargo.toml
# 运行
cargo run  --manifest-path src-tauri/Cargo.toml

# 发布（已启用 opt-level="z" + lto + strip，体积小、内存低）
cargo build --release --manifest-path src-tauri/Cargo.toml
```

**产物路径**：`src-tauri/target/release/EyeProtection.exe`（单文件，i18n 与图标均已嵌入，可直接分发运行）。

> **网络较慢/离线构建**：`windows-sys` 及其目标库首次需从 crates.io 下载；依赖已缓存时可加 `--offline`：
> `cargo build --release --offline --manifest-path src-tauri/Cargo.toml`

## 运行说明

- 程序常驻系统托盘。左键或右键点击托盘图标弹出菜单：设置 / 立即休息 / 关于 / 退出。
- 关闭设置窗口不会退出程序；只有托盘菜单「退出」才真正结束进程。
- 达到「连续工作时间」触发短休息；累计工作达到阈值触发长休息。休息浮层倒计时结束、点击「结束休息」或按 Esc 即可结束。

## 多语言（i18n）

语言文件位于 `ui/i18n/`（`zh-CN.json` / `en.json`），在编译期通过 `include_str!` 嵌入二进制。在设置窗口切换语言并保存后，托盘菜单与休息浮层文案即时生效。新增语言需在 `i18n.rs` 中登记对应的嵌入常量。
