# EyeProtection （护眼程序）

本仓库是基于 Tauri + Rust 实现的跨平台护眼程序，前端使用静态 HTML/JS（位于 `ui/`），后端使用 Rust（位于 `src-tauri/`）。

**快速导航**
- 项目根: `Cargo.toml`, `package.json`, `README.md`（本文件）
- 前端 UI: [ui](ui)
  - [ui/settings.html](ui/settings.html) — 设置界面
  - [ui/settings.js](ui/settings.js) — 设置界面逻辑
  - [ui/reminder.html](ui/reminder.html) — 休息提醒界面
  - [ui/reminder.js](ui/reminder.js) — 提醒逻辑（倒计时、关闭）
  - [ui/i18n](ui/i18n) — 国际化语言文件（zh-CN.json / en.json）
- Tauri / Rust 后端: [src-tauri](src-tauri)
  - [src-tauri/Cargo.toml](src-tauri/Cargo.toml) — 后端依赖
  - [src-tauri/tauri.conf.json](src-tauri/tauri.conf.json) — Tauri 配置（窗口、打包等）
  - [src-tauri/src/main.rs](src-tauri/src/main.rs) — 后端主逻辑：系统托盘、活动监测、窗口控制、i18n 加载
  - [src-tauri/build.rs](src-tauri/build.rs) — Tauri build helper
  - [src-tauri/icons](src-tauri/icons) — 打包与托盘图标

## 目录与文件作用（简述）
- `ui/`：静态前端资源，直接由 Tauri 加载。包含设置页、提醒页和国际化 JSON 文件。
- `src-tauri/`：Tauri 的 Rust 应用代码与配置，负责系统托盘、全局输入监听、计时逻辑与窗口管理。
- `package.json`：前端/开发脚本（包含 `npm run tauri dev` / `npm run tauri build`）
- `settings.json`（运行时生成）：用于保存用户设置（work_time、rest_time、opacity、language 等）。

## 开发环境要求
- Node.js（推荐 16+ 或 LTS）
- Rust（推荐 stable，安装 rustup）
- `@tauri-apps/cli`（作为 devDependency，通过 `npm install` 安装）
- 平台打包所需的工具（见“打包”一节）

## 本地调试（前端 + 后端）
推荐在项目根运行（会由 Tauri CLI 启动后端并加载 `ui`）：

```bash
# 安装前端依赖（会安装 tauri CLI dev dependency）
npm install

# 开发模式（启动后端并加载本地 ui）
npm run tauri dev
```

如果只想调试后端（Rust），可以单独运行：

```bash
cd src-tauri
cargo run
```

如果需要仅在浏览器中查看前端页面，可直接打开 `ui/settings.html` 或用静态文件服务器（例如 `npx serve ui`）。

## 编译与打包 (Build & Bundle)

本项目基于 Tauri 框架，编译和打包过程分为调试编译、发布编译以及安装包打包。

### 1. 调试编译 (Debug Build)
用于开发过程中快速测试，生成的二进制文件包含调试信息，体积较大且运行速度非最优。

```powershell
# 方式一：使用 npm 脚本（推荐，会自动处理前端资源）
npm run tauri dev

# 方式二：直接使用 Cargo（仅编译后端）
cd src-tauri
cargo build
```
**产物路径**：`src-tauri/target/debug/EyeProtection.exe`

### 2. 发布编译 (Release Build)
生成经过优化的二进制文件，体积更小，运行效率更高，但不包含安装程序打包过程。

```powershell
# 在项目根目录运行
npm run tauri build --debug # 虽然是 build 命令，但带上 --debug 可以快速生成 release 目录下的文件

# 在项目根目录运行
npm run tauri build --release --no-bundle
# 或者直接使用 cargo
cd src-tauri
cargo build --release
```
**产物路径**：`src-tauri/target/release/EyeProtection.exe`

### 3. 打包安装程序 (Bundling)
将程序打包成可分发的安装包（如 Windows 上的 `.msi` 或 `.exe`）。

**前提条件 (Windows)**：
- 必须安装 [WiX Toolset v3](https://wixtoolset.org/releases/)。
- 确保 `src-tauri/tauri.conf.json` 中的 `bundle` 配置正确（已配置）。

**打包命令**：
```powershell
# 在项目根目录运行
npm install
npm run tauri build
```

**打包产物路径**：
- **安装包**：`src-tauri/target/release/bundle/msi/EyeProtection_0.1.0_x64_en-US.msi`
- **便携版**：`src-tauri/target/release/bundle/binary/EyeProtection.exe`

### 4. 安装与运行
- **安装**：双击生成的 `.msi` 文件，按照向导完成安装。安装后程序会自动添加到开始菜单。
- **运行**：安装完成后，可以从开始菜单启动，或在系统托盘找到图标进行设置。

## 多语言（i18n）说明
- 前端：语言文件位于 `[ui/i18n](ui/i18n)`，运行时由 `ui/i18n.js` 加载并替换页面 `data-i18n` 标记。
- 后端：Rust 侧会读取 `../ui/i18n/<lang>.json`（启动或设置保存时）来构建系统托盘菜单的本地化文本。设置中的 `language` 字段用于切换语言。

要添加新语言：
1. 在 `ui/i18n/` 下添加新的 `<locale>.json`（结构参照 `zh-CN.json`）
2. 在设置中把 `language` 设置为该 locale（例如 `en` 或 `zh-CN`），保存后后端会更新托盘菜单文本，前端会加载对应 JSON。

## 常见问题
- 如果打包时报错资源（如 Windows 的 ICO 格式错误），请确保使用合规的多尺寸 `.ico` 文件，或临时在 `tauri.conf.json` 中改为使用 `.png` 后回退为正确的 `.ico`。
- 若要在 CI 中做跨平台构建，推荐在各平台的 runner 上执行 `npm run tauri build`（如 GitHub Actions 的 windows-latest、macos-latest、ubuntu-latest）。

---
如果你希望我：
- 帮你把语言选择控件集成到设置页面并持久化为 `settings.language`，
- 或者为 Windows 生成一个合规的多尺寸 `.ico` 并替换到 `src-tauri/icons`，
请告诉我，我可以直接在仓库中实现。
