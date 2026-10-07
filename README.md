# DevBox · 本地开发工具箱

一个面向 macOS 的本地开发辅助桌面应用，用 **Tauri 2 (Rust) + 原生 JS 前端** 构建。集中管理本机的开发项目、端口、hosts 与本地服务，一键启停。

## 功能

- **项目库**：以列表形式管理所有项目，按「项目组」分 tab 归类（全部 / 自定义分组 / 未分组），tab 上可新增、重命名、删除分组（删除分组不会删除项目）。每行都有操作按钮：启动 / 停止 / 重启，以及在编辑器、浏览器、终端、访达中打开、查看日志、编辑、删除；每个 tab 还有「本组启动 / 本组停止」批量按钮。
- **一键拉取代码**：每个项目行有「拉取」按钮，执行 `git pull --ff-only`（只快进，不会擅自产生 merge 提交），并明确告知：已是最新 / 更新了几个提交及改动统计 / 失败原因（分叉、本地有未提交修改、无上游分支、网络或认证问题等）。项目正在运行时会提示需要重启才生效。超过 120 秒自动中止，不会弹出口令输入卡住界面。
- **切换分支**：项目名下显示当前 git 分支标签（直接读 `.git/HEAD`，不起进程，在终端里切分支也会自动同步；游离 HEAD 会特别标出）。点标签打开切换窗口：本地分支在前（当前分支置顶），后面是仅存在于远程的分支，可搜索，可先「获取远程分支」（`git fetch --prune`）。选远程分支时，本地没有就新建并跟踪，已有就直接切过去。有未提交修改时会提示；若与目标分支冲突，切换会被 git 拒绝，修改不会丢失。
- **切换 Node / JDK 版本**：前端项目可指定 Node 版本，后端项目可指定 JDK 版本（「其它」类型两者都可选）。点项目名下的版本标签即可切换，也可在编辑项目里选。启动时会把选定版本放到 PATH 最前（JDK 同时设置 `JAVA_HOME`），启动命令与停止命令都生效；运行中的项目切换后需重启。版本来自直接扫描本机安装目录，不依赖 nvm 等 shell 函数：Node 支持 nvm / fnm / Volta / asdf / n / Homebrew，JDK 支持系统 JVM / SDKMAN / asdf / Homebrew。
- **自动识别版本要求**：新增项目点「自动获取」时，会读取项目声明的 Node / JDK 版本并在本机已安装的版本里自动选中——Node 来自 `.nvmrc`、`.node-version`、`.tool-versions`、`package.json` 的 `volta` / `engines`（范围按 semver 匹配，取满足要求的最低主版本，同主版本取最新，最保守）；JDK 来自 `.sdkmanrc`、`.tool-versions`、`.java-version`、`pom.xml`（`java.version` / `maven.compiler.*`）、Gradle（toolchain / sourceCompatibility），`1.8` 与 `8` 视为同一版本。本机没有满足要求的版本时会明确提示缺什么。对多模块 Maven 项目（根 pom 没有 spring-boot 插件）还会提醒应把工作目录改为哪个子模块。
- **退出与恢复**：关闭窗口、托盘「退出」或 Cmd+Q 时，若有运行中的项目会先弹确认，确认后停止所有项目再退出（项目的输出管道由 DevBox 持有，DevBox 退出后它们会因 SIGPIPE 崩溃，所以无法「退出但保持运行」）；收到 Ctrl+C / `kill` 信号也会先停掉项目，不留孤儿进程。运行中的项目集合会持续记录到配置文件（`last_running`），因此正常退出、崩溃甚至被强杀后，下次启动都会提示「上次有 N 个项目在运行，是否恢复启动」。后台每 2 秒巡检一次，窗口关着也能发现项目崩溃。
- **项目管理与一键启停**：注册项目（工作目录 + 启动命令 + 端口 + 环境变量 + 所属项目组），一键 启动 / 停止 / 重启，并给出成功 / 失败及失败原因；启动确认之后才崩溃的进程（如 Maven 编译几秒后才失败）也会弹出退出码和最后几行关键输出。停止时结束整棵进程树（含 npm→node 等子进程），可选崩溃自动重启。
- **拖拽排序**：在列表里按住项目行（或最左侧的手柄）即可拖动调整顺序；在某个分组 tab 内拖动时，只调整该组内的相对顺序。顺序会写入配置文件长期保留。
- **端口管理**：只列出与已登记项目相关的 LISTEN 端口（项目登记的端口，或运行中项目进程树监听的端口），标注所属项目、进程、PID、地址，可按关键字过滤，一键结束占用进程释放端口。无关的系统端口不显示。
- **服务管理 (Homebrew)**：读取 `brew services list`，对 MySQL / Redis / Nginx / PostgreSQL / MongoDB 等 一键 启动 / 停止 / 重启。
- **Hosts 管理**：图形化编辑 `/etc/hosts` 中 DevBox 的托管区块（增删域名映射、启用/停用），保存时弹出系统管理员授权并自动刷新 DNS 缓存。**不会改动区块之外的其它条目。**
- **日志聚合**：实时查看每个项目的 stdout / stderr，支持自动滚动与清空。
- **快捷入口**：一键在 编辑器(VSCode) / 浏览器 / 终端 / 访达 中打开项目。
- **菜单栏托盘与快速启停**：顶栏图标的菜单里列出所有项目（有分组时按分组显示），点一下就启动或停止，不用打开窗口；还有「全部停止」、「系统通知」开关，悬停提示会显示运行中的项目数。菜单只在内容变化时才重建。
- **系统通知**：项目意外退出（含启动确认之后才崩溃的）、从托盘启停项目的结果，会发 macOS 系统通知，内容是退出原因和最后一行关键输出。窗口在前台聚焦时不重复通知（应用内已有提示）；托盘里的操作因为用户本就没开窗口，总会通知。侧边栏底部的「通知」按钮或托盘菜单都可以关闭。
- **资源监控**：卡片上显示运行中项目的 CPU / 内存 / 运行时长 / 端口探测状态。

## 环境要求

- macOS
- [Node.js](https://nodejs.org) 18+（含 npm）
- [Rust](https://www.rust-lang.org/tools/install) 稳定版工具链
- Xcode Command Line Tools：`xcode-select --install`
- Homebrew（用于服务管理）

## 快速开始

```bash
cd devbox
npm install

# 首次运行前生成图标（可选，仓库已内置一套；如需自定义用下面命令）
# npm run tauri icon app-icon.png

# 开发模式（热更新，自动同时起前端 dev server 与 Rust 后端）
npm run tauri dev
```

打包为 `.app` / `.dmg`：

```bash
npm run tauri build
# 产物在 src-tauri/target/release/bundle/
```

## macOS 权限说明

- **Hosts 写入**：修改 `/etc/hosts` 需要管理员权限。保存时会通过 `osascript` 弹出系统密码框，授权后写入并执行 `dscacheutil -flushcache && killall -HUP mDNSResponder` 刷新 DNS。
- **快捷入口 / brew**：应用通过登录 shell (`/bin/sh -lc`) 调用命令，以便正确读取 `code`、`brew`、`nvm` 等的 PATH。若某命令找不到，请确认它在你的 shell 配置里可用。
- 首次通过 Finder 打开打包版本时，若提示"无法验证开发者"，在 系统设置 → 隐私与安全性 中允许即可（未签名应用的正常提示）。

## 数据存储

项目与项目组配置以 JSON 存于：

```
~/Library/Application Support/devbox/config.json
```

## 项目结构

```
devbox/
├── index.html               # 前端入口
├── src/                      # 前端（原生 JS）
│   ├── main.js               # 视图路由
│   ├── api.js                # 后端命令封装
│   ├── ui.js                 # DOM / toast 辅助
│   ├── style.css
│   └── views/                # 各功能面板
│       ├── projects.js  ports.js
│       ├── services.js  hosts.js     logs.js
└── src-tauri/                # Rust 后端
    ├── Cargo.toml  build.rs  tauri.conf.json
    ├── capabilities/default.json
    └── src/
        ├── main.rs  lib.rs   # 入口 + 命令注册 + 托盘
        ├── models.rs  state.rs
        └── commands/         # projects/process/ports/hosts/services/logs/shortcuts
```

## 已知限制 / 后续可扩展

- 服务管理目前走 Homebrew；Docker 容器管理、本地 HTTPS 证书 (mkcert) 可作为下一步扩展。
- 日志走 1 秒轮询获取缓冲（单项目上限 2000 行）；如需持久化到文件可后续增加。
- 自动重启在健康心跳（约 2.5s）中触发。
```
