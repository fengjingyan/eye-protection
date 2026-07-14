# EyeProtection （护眼程序 · Slint 分支）

本 `slint` 分支用 **Slint**（声明式 Rust UI 框架，自带渲染器，不依赖 WebView）实现设置界面，去除 Tauri。托盘图标/菜单与全屏休息浮层仍用 Win32（Slint 无托盘；全屏透明浮层用 Win32 更稳）。

> 其它分支：`master`（原始 Tauri/WebView 版）、`win32`（纯 Win32 全量重写）、`mix-frontend`（混合：休息窗原生、设置/关于仍用 Tauri WebView）。四者用于内存对比。

## 内存实测

| 状态 | Slint (本分支) | 纯 Win32 | 混合(mix-frontend) | 原始 Tauri |
|---|---|---|---|---|
| 空闲（工作中） | ~11 MB | ~9 MB | ~11 MB | ~300 MB |
| 打开设置窗口 | ~24 MB (Slint 软件渲染器+字体) | ~10 MB | ~50–150 MB (WebView) | ~300 MB |
| 休息弹窗期间 | ~14 MB (Win32 浮层) | ~14 MB | ~14 MB | ~300–500 MB |
| 进程数 | 单进程，无 WebView2 | 单进程 | 单进程 | 主进程 + 多个 msedgewebview2 |

Slint 比纯 Win32 略高（多了软件渲染器与字体加载），但仍远低于 WebView 方案，且 UI 用声明式标记编写、跨平台。

## 架构与文件

- [src-tauri/src/main.rs](src-tauri/src/main.rs) — 主逻辑：Win32 托盘/菜单、slint::Timer 每秒计时、rdev 输入监听、`slint::run_event_loop()` 主循环、按需创建 Slint 设置窗口。
- [src-tauri/ui/settings.slint](src-tauri/ui/settings.slint) — **Slint 声明式设置界面**（SpinBox/Slider/ComboBox/CheckBox/Button），`default-font-family: "Microsoft YaHei"` 保证中文字形。
- [src-tauri/src/overlay.rs](src-tauri/src/overlay.rs) — 原生 Win32 全屏休息浮层。
- [src-tauri/src/settings.rs](src-tauri/src/settings.rs) — 设置持久化（`%APPDATA%\com.eyeprotection.app\settings.json`）。
- [src-tauri/src/i18n.rs](src-tauri/src/i18n.rs) — i18n，locale JSON 编译期嵌入。
- [src-tauri/build.rs](src-tauri/build.rs) — 用 `slint-build` 编译 `.slint`。

> Slint 事件循环（winit）会一并派发 Win32 托盘窗口与 `WM_COMMAND` 消息，因此托盘与 Slint 窗口能在同一线程共存。

## 关键集成点

- 托盘（Shell_NotifyIcon）用一个隐藏 Win32 窗口承载；其 `WndProc` 在 Slint/winit 的消息循环中被正常回调，实现托盘菜单。
- 计时用 `slint::Timer`（在 Slint 事件循环内稳定触发），无需 `SetTimer`/`WM_TIMER`。
- 设置窗口关闭按钮通过 `on_close_requested => HideWindow` 隐藏而非退出事件循环；托盘「退出」调用 `slint::quit_event_loop()`。

## 开发环境要求

- Rust（stable，rustup）
- **仅 Windows**（托盘/浮层用 Win32 API）。
- 无需 Node.js / npm / Tauri CLI。
- **首次构建需联网**下载 Slint 依赖树（较大）。国内网络若直连 crates.io 超时，可配置镜像（如 ustc）：在项目根建 `.cargo/config.toml`（已被 gitignore）：
  ```toml
  [source.crates-io]
  replace-with = "ustc"
  [source.ustc]
  registry = "sparse+https://mirrors.ustc.edu.cn/crates.io-index/"
  ```

## 编译与运行

```powershell
# 调试
cargo build --manifest-path src-tauri/Cargo.toml
cargo run  --manifest-path src-tauri/Cargo.toml

# 发布（opt-level="z" + lto + strip）
cargo build --release --manifest-path src-tauri/Cargo.toml
```

**产物路径**：`src-tauri/target/release/EyeProtection.exe`

## 运行说明

- 程序常驻托盘。点击托盘图标弹出菜单：设置（Slint 窗口）/ 立即休息 / 关于 / 退出。
- 达到「连续工作时间」触发短休息；累计工作达阈值触发长休息。休息浮层倒计时结束、点击「结束休息」或按 Esc 结束。

## 多语言（i18n）

语言文件 `ui/i18n/{zh-CN,en}.json` 编译期嵌入；设置窗口的标签由 Rust 按当前语言注入到 Slint 属性。切换语言保存后，托盘菜单与休息浮层即时生效（Slint 设置窗口的标签在下次打开时刷新）。
