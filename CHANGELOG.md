# 更新日志

## 1.3.0 · 2026-10-14

DevBox 现在支持 **Windows** 和 **Linux**，不再只是 macOS 应用。

### 新增
- **Windows 安装包**（NSIS，装在当前用户目录，不需要管理员权限）；**Linux 安装包**（`.deb` 与 `.AppImage`）。发布流程改为三个系统并行打包、统一发布
- 项目启停、日志、端口、git、Node / JDK 版本切换、托盘、通知等核心功能三个系统通用
- Windows：
  - 启动命令走 `cmd`，不弹黑窗；结束项目时整棵进程树一起结束
  - 子进程输出按系统代码页解码（中文版 Windows 的 GBK 不再乱码）
  - 识别 nvm-windows / fnm / Volta / Scoop 装的 Node，以及 Program Files 下各家 JDK
  - 修改 hosts 通过 UAC 授权，原文件是 CRLF 换行会保持
  - 在 Windows Terminal（没有则 cmd）/ 资源管理器 / 默认浏览器中打开
  - 获取启动命令认得 `mvnw.cmd`、`gradlew.bat`、`python`、`.venv\Scripts\python.exe`
- Linux：
  - 端口占用查询不再依赖 `lsof`（精简发行版常没装）
  - 修改 hosts 通过 `pkexec` 授权；打开终端支持常见终端模拟器与 `$TERMINAL`；定位文件走 `xdg-open` / FileManager1
- 选择项目目录在 Windows / Linux 上用系统自带的文件夹选择框
- 「本地服务」依赖 Homebrew，非 macOS 上入口自动隐藏
- 界面文案随系统变化（访达 / 资源管理器 / 文件管理器，⌘ / Ctrl），字体加入 Windows 与 Linux 的回退
- MCP 适配器按系统查找 `bridge.json`（`%APPDATA%` / `XDG_CONFIG_HOME` / `~/Library/Application Support`）

### 修复
- 子进程输出里出现非 UTF-8 字节时，日志读取线程会直接退出，之后没人读管道，子进程写满缓冲区就卡死。现在按字节读取再解码

### 工程
- 平台差异集中到 `src-tauri/src/platform/`（`unix.rs` / `windows.rs` / `listen.rs`），业务代码只调统一接口
- CI 的 Rust 测试与应用构建扩展到 macOS / Linux / Windows 三个系统；Windows 上新增启动、进程树、版本探测、脚本、hosts 换行等测试
- Windows 的测试程序和正式程序一样嵌入 Common Controls v6 清单，否则测试程序一启动就崩溃
- 发布前检查：标签与版本号一致、`CHANGELOG.md` 里有本版本的说明，任何一项不满足就在打包前失败

### 已知限制
- Windows / Linux 的安装包没有做代码签名：Windows 首次运行 SmartScreen 会提示「未知发布者」
- Windows 的 UAC 弹窗、Linux 各桌面环境（托盘、`pkexec`、终端）目前只在自动化测试之外通过代码推断，未在真机上逐一验证；遇到问题欢迎反馈

## 1.2.1 · 2026-10-14

解决 macOS 上反复弹出「DevBox 想访问“文稿”文件夹」的问题。

### 修复
- macOS 按「代码签名身份」记住文件夹访问授权，而 DevBox 之前没有固定证书，每次重新编译、升级版本签名都变，授权就作废，于是反复弹窗。现在支持用固定的自签名证书签名：
  - `scripts/create-signing-cert.sh`：一条命令创建证书并设为受信任，可重复运行
  - `scripts/build-signed.sh`：本机打包并用该证书签名
  - 开发模式（`npm run tauri dev`）编译后自动签名（`scripts/sign-and-run.sh`），没有证书时原样运行
  - 步骤见 README「固定签名，消除文件夹权限弹窗」
- 文件夹访问弹窗里加上用途说明，说明 DevBox 为什么要读项目目录

### 工程
- CI 新增只在手动运行时执行的「签名流程 (macOS)」任务，在真实 macOS 上验证「创建证书 → 签名 → 打包」整条流程
- 新增脚本检查：shell 脚本和工作流里变量后面不能直接紧跟中文（macOS 自带的 bash 3.2 会把中文的第一个字节当成变量名，配合 `set -u` 直接报错）；顺带修了发布工作流里同类的潜在问题

## 1.2.0 · 2026-10-08

项目管理能力大幅增强，并加入持续集成。

### 项目库
- 项目改为「项目库」：按项目组分 tab，列表展示并配置操作按钮；项目可拖拽排序，也可拖到别的 tab 换组
- 启动给出明确反馈：成功、失败、失败原因；启动前检查端口占用，被占用时说明占用者，可「结束占用并启动」
- 启动前依赖：勾选的项目会先启动并等它就绪，再启动本项目；依赖出错会说明是哪一个、为什么
- 运行中的停止 / 重启按钮不再变形；所有按钮文字不再折行
- 新增「获取启动命令」：扫描工作目录，列出可用的启动命令供点选（Node、Maven、Gradle、Cargo、Go、Python、docker compose 等；多模块 Maven 会指定带插件的子模块）

### 环境与 git
- Node / JDK 版本切换；新增项目时自动识别 `.nvmrc`、`engines`、`pom.xml`、Gradle 等声明的版本要求，并匹配本机已安装的版本
- 一键拉取代码、切换分支、状态徽标（领先 / 落后 / 有改动）、本组批量拉取
- 脚本 / 构建任务：每个项目一个「运行脚本」菜单（install、build、test 等），输出写进项目日志

### 日志
- ANSI 颜色、级别高亮、搜索、级别过滤、复制、落盘与历史（重启后仍能看到上次的输出）

### 系统
- 系统通知（启动失败、崩溃）与托盘快速启停
- 退出时停止运行中的项目，下次启动时提示恢复；强杀后残留的孤儿进程会被清理
- 端口页只显示与项目相关的端口，并新增项目组 tab 与项目组列

### 修复
- 修复 `tauri` crate 与 `@tauri-apps/api` 版本不一致导致 `tauri dev` 启动报错，并加入防漂移检查
- 修复启动确认与进程回收之间的竞态（误报「被停止」）
- 修复 `GIT_OPTIONAL_LOCKS` 进程级污染

### 工程
- 新增 CI（GitHub Actions）：前端测试与构建、Rust clippy + 单元测试（macOS / Linux）、应用构建
- 新增发布工作流：推送 `v*` 标签后在 macOS 上打包通用 dmg 并创建 GitHub Release

## 1.1.0

- 新增 MCP 桥接服务：DevBox 内置本地 HTTP 桥接 + Node MCP 适配器，可供 agent 管理项目 / 端口 / hosts / brew 服务

## 1.0.1

- 修复端口无法结束与界面无反馈；工作目录支持目录选择器；启动命令支持多行；新增自动获取项目信息、项目单独日志查看

## 1.0.0

- 首个版本：项目、端口、hosts 与本地服务管理
