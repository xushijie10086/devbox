# DevBox · 本地开发工具箱

[![CI](https://github.com/xushijie10086/devbox/actions/workflows/ci.yml/badge.svg)](https://github.com/xushijie10086/devbox/actions/workflows/ci.yml)

一个本地开发辅助桌面应用，支持 **macOS、Windows、Linux**，用 **Tauri 2 (Rust) + 原生 JS 前端** 构建。集中管理本机的开发项目、端口、hosts 与本地服务，一键启停。

## 功能

- **项目库**：以列表形式管理所有项目，按「项目组」分 tab 归类（全部 / 自定义分组 / 未分组），tab 上可新增、重命名、删除分组（删除分组不会删除项目）。每行都有操作按钮：启动 / 停止 / 重启，以及在编辑器、浏览器、终端、访达中打开、查看日志、编辑、删除；每个 tab 还有「本组启动 / 本组停止」批量按钮。
- **一键拉取代码**：每个项目行有「拉取」按钮，执行 `git pull --ff-only`（只快进，不会擅自产生 merge 提交），并明确告知：已是最新 / 更新了几个提交及改动统计 / 失败原因（分叉、本地有未提交修改、无上游分支、网络或认证问题等）。项目正在运行时会提示需要重启才生效。超过 120 秒自动中止，不会弹出口令输入卡住界面。
- **git 状态徽标**：分支标签后面显示 `↑N`（本地领先、待推送）、`↓N`（落后远程、可拉取，以最近一次 fetch 为准）、`●N`（有未提交修改的文件数），悬停可看详细说明。由独立的后台线程每 20 秒刷新（不经登录 shell、`GIT_OPTIONAL_LOCKS=0`，不会拖慢巡检，也不会和你自己的 git 操作抢索引锁）；拉取、切换分支、获取远程后立即更新。
- **本组批量拉取**：分组 tab 右上角的「⬇ 本组拉取」对当前 tab 里的 git 项目依次执行 `git pull --ff-only`（3 个并发），结果汇总成一条提示：哪些有更新（及改动统计）、几个已是最新、哪些失败及原因；运行中的项目会提示需重启。
- **拖拽换分组**：把项目行拖到某个分组 tab 上松手，即可把它移到该分组（悬停时被拖的行变半透明、目标 tab 高亮）。
- **切换分支**：项目名下显示当前 git 分支标签（直接读 `.git/HEAD`，不起进程，在终端里切分支也会自动同步；游离 HEAD 会特别标出）。点标签打开切换窗口：本地分支在前（当前分支置顶），后面是仅存在于远程的分支，可搜索，可先「获取远程分支」（`git fetch --prune`）。选远程分支时，本地没有就新建并跟踪，已有就直接切过去。有未提交修改时会提示；若与目标分支冲突，切换会被 git 拒绝，修改不会丢失。
- **切换 Node / JDK 版本**：前端项目可指定 Node 版本，后端项目可指定 JDK 版本（「其它」类型两者都可选）。点项目名下的版本标签即可切换，也可在编辑项目里选。启动时会把选定版本放到 PATH 最前（JDK 同时设置 `JAVA_HOME`），启动命令与停止命令都生效；运行中的项目切换后需重启。版本来自直接扫描本机安装目录，不依赖 nvm 等 shell 函数：Node 支持 nvm / fnm / Volta / asdf / n / Homebrew，JDK 支持系统 JVM / SDKMAN / asdf / Homebrew。
- **自动获取启动命令**：编辑项目时点启动命令右上角的「获取启动命令」，读取工作目录里的配置文件（只读，不执行任何东西），列出所有能用的启动命令供点选，推荐的排在最前；命令框是空的会先填推荐项，已有内容不会被覆盖。覆盖 Node（按 `pnpm-lock.yaml` / `yarn.lock` / `bun.lockb` 选包管理器，dev / start / serve 类脚本；Tauri 项目优先 `tauri dev`；没装依赖会提醒）、Maven（`mvnw`、Spring Boot / Quarkus / Jetty；多模块项目会指定带插件的子模块，并给出「先安装依赖再启动」和「跳过安装」两种）、Gradle（`bootRun` / `run`，含子项目）、Cargo、Go（`cmd/*` 入口、air）、Python（Django / FastAPI / Flask，优先用项目自己的 `.venv` / poetry / pipenv / uv）、Rails、Laravel、Deno、docker compose、Procfile、Makefile。原来的「自动获取」填表也改用这里的推荐项。外部工具也可以调用 `/projects/start-commands`。
- **环境切换（dev / test / prod …）**：每个项目（前端、后端都行）自动读取项目里声明的环境，点项目名下的「环境」标签即可切换，列表里一眼能看到每个项目当前用的是哪个；新增项目点「自动获取」时也会告诉你读到了哪些。选定后**只在启动时生效**，不会修改项目里的任何文件，也不影响「运行脚本」（否则 `NODE_ENV=production` 会让 `npm install` 把开发依赖装丢）；选「默认」就是项目原本的行为。运行中的项目切换后需重启；选中 `prod` 一类的生产环境时标签会标红。各技术栈的识别方式：

  | 技术栈 | 从哪里读 | 选中后做什么 |
  |---|---|---|
  | Spring Boot（Maven / Gradle，含多模块） | `application-<名>.yml` / `.yaml` / `.properties`、`bootstrap-<名>.*`；并读出 `spring.profiles.active` 作为「默认环境」 | 设置 `SPRING_PROFILES_ACTIVE=<名>` |
  | Node 项目的脚本 | `package.json` 里 `dev:test`、`start:prod`、`serve-staging` 这类「启动词 + 环境词」的脚本 | 改用该脚本（按项目的包管理器拼命令） |
  | Vite / Vue CLI | `.env.<模式>`（忽略 `.local`、`.example` 等） | 启动命令追加 `--mode <模式>`（npm 自动补 `--`） |
  | Node 后端（Nest / Express / Koa / Fastify…） | `.env.<名>`、`config/<名>` 下的 json / js / ts / yml 等（名字必须是 dev / test / prod 这类环境词） | 设置 `NODE_ENV=<名>` |
  | Rails | `config/environments/<名>.rb` | 设置 `RAILS_ENV=<名>` |
  | Laravel | `.env.<名>` | 设置 `APP_ENV=<名>` |

  几点说明：同名环境（比如同一个目录里既有 Spring 又有前端）会合并成一个；选中的环境若在项目里找不到了（文件被删或切了分支），启动会被拒绝并说明原因，**不会悄悄退回默认环境**去启动；环境里的变量会盖过「编辑项目」里手填的同名环境变量，并在项目日志里写明；启动命令里已经写死 `-Dspring-boot.run.profiles=…` 之类时，它的优先级更高，弹窗里会提醒。暂不处理 Maven 的 `-P` 构建 profile。

- **自动识别版本要求**：新增项目点「自动获取」时，会读取项目声明的 Node / JDK 版本并在本机已安装的版本里自动选中——Node 来自 `.nvmrc`、`.node-version`、`.tool-versions`、`package.json` 的 `volta` / `engines`（范围按 semver 匹配，取满足要求的最低主版本，同主版本取最新，最保守）；JDK 来自 `.sdkmanrc`、`.tool-versions`、`.java-version`、`pom.xml`（`java.version` / `maven.compiler.*`）、Gradle（toolchain / sourceCompatibility），`1.8` 与 `8` 视为同一版本。本机没有满足要求的版本时会明确提示缺什么。对多模块 Maven 项目（根 pom 没有 spring-boot 插件）还会提醒应把工作目录改为哪个子模块。
- **退出与恢复**：关闭窗口、托盘「退出」或 Cmd+Q 时，若有运行中的项目会先弹确认，确认后停止所有项目再退出（项目的输出管道由 DevBox 持有，DevBox 退出后它们会因 SIGPIPE 崩溃，所以无法「退出但保持运行」）；收到 Ctrl+C / `kill` 信号也会先停掉项目，不留孤儿进程。运行中的项目集合会持续记录到配置文件（`last_running`），因此正常退出、崩溃甚至被强杀后，下次启动都会提示「上次有 N 个项目在运行，是否恢复启动」。记录里同时保存每个项目进程的 pid 与进程启动时间：DevBox 被强杀 / 崩溃（含 `tauri dev` 重编译重启）后项目进程会变成孤儿继续占着端口，下次启动时会先把它们清理掉（pid 和启动时间都对得上才清理，pid 被别的进程复用不会误杀），再提示恢复。后台每 2 秒巡检一次，窗口关着也能发现项目崩溃。
- **脚本 / 构建任务**：每个项目行有「⚙ 脚本」按钮，列出识别到的脚本，点一下就运行：Node（`package.json` 的 scripts，按 npm / pnpm / yarn 自动选命令，没有 `node_modules` 时提醒先安装依赖）、Maven（优先用 `./mvnw`；多模块项目提示先 `install`）、Gradle、Cargo、Go、Python、Makefile 目标，也可以输入自定义命令。脚本与启动项目共用同一套环境（选定的 Node / JDK、项目环境变量），输出写入项目日志；每个项目同时一个脚本任务，运行中行内显示带计时的标签，点击可取消（连同子进程一起结束）；完成 / 失败（带退出原因与最后几行输出）都会提示，窗口在后台时还会发系统通知。
- **启动依赖**：项目可设置「启动前依赖」的其他项目（编辑项目里勾选）。启动它时会先确保依赖已就绪——没运行就先启动（递归处理依赖的依赖），有端口就等端口监听、没端口就看进程存活——然后才启动本项目；提示里会写明「已先启动依赖：…」。依赖启动失败或迟迟不就绪时取消启动并说明是哪个依赖、为什么（间接依赖会写成「依赖「后端」的依赖「存储」启动失败…」）。保存时检测并拒绝循环依赖，删除项目时自动从别人的依赖里摘掉；停止一个仍被运行中项目依赖的项目时会先确认。「本组启动」「托盘」「恢复」等所有入口并发启动时，靠每个项目一把启动锁保证顺序与去重（共享的依赖只会启动一次）。
- **端口占用预检**：启动项目前（以及它还没运行的依赖），如果登记的端口已被占用，会先弹窗说明是谁占的——外部进程（进程名与 PID，由 `lsof` 检测）或 DevBox 里另一个运行中的项目——并让你选「结束占用并启动 / 仍然启动 / 取消」。结束占用时，外部进程会连同子进程一起结束并确认端口已释放，DevBox 项目则正常停止。检测不可用时不阻塞启动；批量启动、托盘、恢复不弹窗，改为在启动结果里点名占用者。
- **项目管理与一键启停**：注册项目（工作目录 + 启动命令 + 端口 + 环境变量 + 所属项目组），一键 启动 / 停止 / 重启，并给出成功 / 失败及失败原因；启动确认之后才崩溃的进程（如 Maven 编译几秒后才失败）也会弹出退出码和最后几行关键输出。停止时结束整棵进程树（含 npm→node 等子进程），可选崩溃自动重启。
- **拖拽排序**：在列表里按住项目行（或最左侧的手柄）即可拖动调整顺序；在某个分组 tab 内拖动时，只调整该组内的相对顺序。顺序会写入配置文件长期保留。
- **端口管理**：只列出与已登记项目相关的 LISTEN 端口（项目登记的端口，或运行中项目进程树监听的端口），标注所属项目、进程、PID、地址，可按关键字过滤，一键结束占用进程释放端口。无关的系统端口不显示。
- **服务管理 (Homebrew)**：读取 `brew services list`，对 MySQL / Redis / Nginx / PostgreSQL / MongoDB 等 一键 启动 / 停止 / 重启。
- **Hosts 管理**：图形化编辑 `/etc/hosts` 中 DevBox 的托管区块（增删域名映射、启用/停用），保存时弹出系统管理员授权并自动刷新 DNS 缓存。**不会改动区块之外的其它条目。**
- **日志**：项目行的日志弹窗和「日志」页共用同一个查看器。支持 **ANSI 颜色**（16 / 256 / 真彩色，进度条的 `\r` 覆盖只保留最后一次，光标控制等乱码序列自动去掉）、**错误 / 警告高亮**（`[ERROR]`、`Exception`、`Caused by:`、`npm ERR!`、`error[E0308]` 等；堆栈续行变淡）、**搜索**（不区分大小写，匹配处高亮，⌘F 聚焦）、**级别过滤**（全部 / 警告及以上 / 仅错误）、**复制**（复制当前过滤后看到的内容，不含颜色转义）、**自动滚动**（手动上翻自动暂停，滚回底部恢复）、**清空**、**日志文件**（在访达里定位）。每行日志带递增序号，前端只拉取新增的行、只追加不重绘；页面最多渲染 3000 行。
- **日志落盘与历史**：每个项目的日志同时追加写入配置目录下的 `logs/<项目>.log`（`日期 时间<TAB>流<TAB>内容`），超过 5MB 轮转成 `.log.1`；重启 DevBox 后自动载入最近 300 行历史并用分隔线隔开；「清空」会连文件一起删。系统消息（`▶ 启动: …` 之类）不按内容判断级别，避免启动命令里恰好含有 `[ERROR]` 字样被误标成错误。
- **快捷入口**：一键在 编辑器(VSCode) / 浏览器 / 终端 / 文件管理器（访达 / 资源管理器 / 系统默认）中打开项目。
- **菜单栏托盘与快速启停**：顶栏图标的菜单里列出所有项目（有分组时按分组显示），点一下就启动或停止，不用打开窗口；还有「全部停止」、「系统通知」开关，悬停提示会显示运行中的项目数。菜单只在内容变化时才重建。
- **系统通知**：项目意外退出（含启动确认之后才崩溃的）、从托盘启停项目的结果，会发系统通知，内容是退出原因和最后一行关键输出。窗口在前台聚焦时不重复通知（应用内已有提示）；托盘里的操作因为用户本就没开窗口，总会通知。侧边栏底部的「通知」按钮或托盘菜单都可以关闭。
- **资源监控**：卡片上显示运行中项目的 CPU / 内存 / 运行时长 / 端口探测状态。

## 下载安装

到 [Releases](https://github.com/xushijie10086/devbox/releases) 下载：

| 系统 | 文件 | 安装 |
|---|---|---|
| macOS | `DevBox_*_universal.dmg` | 拖进「应用程序」（Apple Silicon / Intel 通用） |
| Windows 10 / 11 | `DevBox_*_x64-setup.exe` | 双击安装，装在当前用户目录，不需要管理员权限 |
| Linux | `DevBox_*_amd64.deb` / `DevBox_*_amd64.AppImage` | Debian / Ubuntu：`sudo apt install ./DevBox_*.deb`；其他发行版：`chmod +x` 后直接运行 AppImage |

应用目前没有做代码签名，首次打开系统会拦一下：macOS 右键 → 打开（或 `xattr -cr /Applications/DevBox.app`）；Windows 在 SmartScreen 里点「更多信息」→「仍要运行」。

### 各平台差异

| 功能 | macOS | Windows | Linux |
|---|---|---|---|
| 项目启停、日志、端口、git、Node / JDK 切换 | ✓ | ✓ | ✓ |
| 启动命令的执行方式 | 登录 shell（`/bin/sh -lc`） | `cmd /C`（不弹黑窗） | 登录 shell（`/bin/sh -lc`） |
| 端口占用查询 | `lsof` | 系统 API | 系统 API（不依赖 `lsof`） |
| 修改 hosts | 系统密码框（`osascript`） | UAC 授权弹窗 | `pkexec` 授权弹窗（需已安装 polkit） |
| 打开终端 / 文件管理器 | Terminal / 访达 | Windows Terminal（没有则 cmd）/ 资源管理器 | 常见终端（也认 `$TERMINAL`）/ `xdg-open` |
| 本地服务（Homebrew） | ✓ | 不支持，入口自动隐藏 | 不支持，入口自动隐藏 |
| 托盘 | 菜单栏 | 通知区域 | 需要桌面环境支持托盘（GNOME 需装 AppIndicator 扩展） |

## 开发环境要求

- [Node.js](https://nodejs.org) 18+（含 npm）
- [Rust](https://www.rust-lang.org/tools/install) 稳定版工具链
- macOS：Xcode Command Line Tools（`xcode-select --install`）；要用「本地服务」功能需要 Homebrew
- Windows：[Visual Studio 生成工具](https://visualstudio.microsoft.com/visual-cpp-build-tools/)（勾选「使用 C++ 的桌面开发」）和 WebView2（Windows 10 / 11 自带）
- Linux（Debian / Ubuntu）：
  ```bash
  sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
    librsvg2-dev libxdo-dev libssl-dev patchelf
  ```

## 快速开始

```bash
cd devbox
npm install

# 首次运行前生成图标（可选，仓库已内置一套；如需自定义用下面命令）
# npm run tauri icon app-icon.png

# 开发模式（热更新，自动同时起前端 dev server 与 Rust 后端）
npm run tauri dev
```

打包（产物在 `src-tauri/target/release/bundle/`，只会生成当前系统对应的格式）：

```bash
npm run tauri build
# macOS：.app / .dmg；Windows：NSIS 安装包（.exe）；Linux：.deb / .AppImage
```

## 持续集成（CI）

`.github/workflows/ci.yml`，推送到 `main` 和每个 Pull Request 都会跑，也可以在 Actions 页面手动运行：

| 任务 | 内容 |
|---|---|
| 前端测试与构建 | `npm test`（含 tauri crate 与 npm 包版本一致性检查）、`npm run build`、MCP 适配器语法检查 |
| Rust（macOS / Linux / Windows） | `cargo clippy -D warnings`、`cargo test --lib`（都带 `--locked`，`Cargo.lock` 不同步会失败）；三个系统各跑一遍，Windows 上有专门的启动、进程树、版本探测、hosts 换行等测试 |
| 应用构建（macOS / Linux / Windows） | `tauri build --debug --no-bundle`，验证前端和 Rust 能一起构建 |
| 签名流程（macOS，仅手动运行） | 验证「创建证书 → 签名 → 打包」整条流程 |

发布见 `.github/workflows/release.yml`：推送 `v*` 标签后，三个系统并行打包，统一创建 GitHub Release。工作流本身的 PR 或手动运行只打包验证、不发布。

Rust 版本固定在工作流顶部的 `RUST_TOOLCHAIN`，避免新版本 clippy 新增规则让 CI 无故变红；升级时改这一处，并在本地先跑 `cargo clippy --all-targets -- -D warnings`。本地提交前可以直接跑：

```bash
npm test
cd src-tauri && cargo clippy --all-targets -- -D warnings && cargo test --lib
```

## 固定签名，消除文件夹权限弹窗

如果项目放在「文稿」「桌面」「下载」里，macOS 会弹「DevBox 想访问“文稿”文件夹中的文件」。macOS 按 **代码签名身份** 记住你的选择；DevBox 没有固定证书时，每次重新编译、升级版本，签名都会变，之前的授权就作废，于是反复弹。
解决办法是用同一个证书签名，身份不变，授权就一直有效。

**1. 创建自签名证书（只需一次，免费）**

1. 打开「钥匙串访问」→ 菜单「证书助理」→「创建证书」
2. 名称填 `DevBox Local Signing`，身份类型选「自签名根证书」，证书类型选「**代码签名**」，创建
3. 在「登录」钥匙串里找到它，双击 → 展开「信任」→「代码签名」选「始终信任」
4. 终端确认能看到它：

   ```bash
   security find-identity -v -p codesigning
   ```

想用别的证书名：设置环境变量 `DEVBOX_SIGN_IDENTITY="证书名"`。

**2. 开发模式（`npm run tauri dev`）**：什么都不用做。`.cargo/config.toml` 配了运行前签名（`scripts/sign-and-run.sh`），每次编译后会自动用这个证书签名再运行；没有证书时原样运行，不影响开发。

**3. 打包安装（`.app` / `.dmg`）**：

```bash
scripts/build-signed.sh
```

构建完会打印签名信息，应该是 `certificate leaf = H"…"`；如果还是 `cdhash H"…"`，说明没签上。

**4. 切换后的第一次**：换成固定签名后，macOS 会把它当成一个新应用，**再弹最后一次，点「允许」**。之后重新构建、升级都不会再弹。想清掉以前残留的授权记录：

```bash
tccutil reset SystemPolicyDocumentsFolder dev.devbox.app
```

注意：
- GitHub Actions 上打出来的发布包仍是未签名的（CI 里没有你本机的证书），自己用请用 `scripts/build-signed.sh` 本地打包。
- 自签名证书只对你自己的 Mac 有效；要给别人用，需要 Apple 开发者账号做 Developer ID 签名和公证。
- 最省事的办法其实是把项目放到不受保护的目录（如 `~/code`），那样根本不会弹。

## 发布包也固定签名（让所有用户升级后不再弹框）

上一节解决的是你自己机器上的弹窗。给别人用的 GitHub Release，也要**每个版本都用同一张证书签名**，用户才只在第一次弹一次，之后升级不再弹。做法是把你本机那张证书导出，放进仓库的 Secrets，发布时由 CI 签名（`scripts/import-signing-cert.sh`）。

**1. 导出证书**（只需要一次）：打开「钥匙串访问」→ 左侧选「登录」→ 上方选「我的证书」→ 找到 `DevBox Local Signing`（展开能看到下面有私钥）→ 右键「导出…」→ 格式选 `.p12` → 设一个密码（记住它）。只导出这一张，不要用「导出全部」。

**2. 放进仓库 Secrets**：GitHub 仓库 → Settings → Secrets and variables → Actions → New repository secret，添加两个：

| 名称 | 内容 |
|---|---|
| `SIGNING_CERT_P12` | `.p12` 文件的 base64：终端执行 `base64 -i DevBox-signing.p12 \| pbcopy`，粘贴 |
| `SIGNING_CERT_PASSWORD` | 第 1 步设的密码 |

装了 [gh](https://cli.github.com) 的话也可以：`gh secret set SIGNING_CERT_P12 < <(base64 -i DevBox-signing.p12)` 和 `gh secret set SIGNING_CERT_PASSWORD`。

**3. 保管好 `.p12` 和密码**。证书丢了或换了，签名身份就变了，所有用户升级时会再弹一次。

配好之后，打标签发布时 macOS 包自动用这张证书签名，并检查签名里是 `certificate leaf`（固定身份）而不是 `cdhash`（临时签名，每次不同），不满足就让发布失败。**打标签时如果没配 Secrets，发布会直接失败**，而不是悄悄发出一个每次升级都弹框的包；配好后在 Actions 里重新运行那次发布即可，不需要重打标签。

本机打的包和 CI 打的包用的是同一张证书，所以身份一致，你自己装官方包也不会多弹一次。

**局限**：自签名证书在别人的 Mac 上不被系统信任，所以**首次打开仍会提示「无法验证开发者」**（右键 → 打开，或 `xattr -cr /Applications/DevBox.app`）；这是自签名和 Apple 开发者证书（99 美元/年，签名 + 公证）的区别。想彻底消除这个提示，需要后者。

## 依赖版本约定

Rust 的 `tauri` crate 与前端的 `@tauri-apps/api`、`@tauri-apps/cli` 必须是同一个「主.次」版本，否则 `tauri dev` 启动会报 `version mismatched Tauri packages`。两边都用 `~2.12` 锁定了次版本；要升级时一起改（`cargo update -p tauri` 同时 `npm install @tauri-apps/api@~X.Y @tauri-apps/cli@~X.Y`），`npm test` 会在漂移时直接报错并给出修复命令。

## 测试

```bash
cd src-tauri && cargo test --lib   # 后端单元测试（含真实 git 仓库、真实 shell 进程、lsof）
npm test                           # 前端纯逻辑测试（日志的 ANSI 解析 / 级别识别 / 搜索过滤），
                                   # 并检查 Rust 的 tauri crate 与 @tauri-apps/api / cli 的主.次版本一致
```

## 系统权限说明

- **Hosts 写入**：修改 hosts 需要管理员权限，保存时会弹出系统的授权框，授权后写入并刷新 DNS 缓存。
  - macOS：`osascript` 密码框，写入后执行 `dscacheutil -flushcache && killall -HUP mDNSResponder`。
  - Windows：UAC 提示（经 PowerShell 提权），文件原来是 CRLF 换行的会保持 CRLF；写入后执行 `ipconfig /flushdns`。
  - Linux：`pkexec` 授权框（需要系统装了 polkit），写入后如果有 `resolvectl` 会顺带刷新缓存。
- **快捷入口 / 版本管理**：macOS / Linux 通过登录 shell (`/bin/sh -lc`) 调用命令，以便正确读取 `code`、`brew`、`nvm` 等的 PATH，某命令找不到时请确认它在你的 shell 配置里可用；Windows 通过 `cmd` 调用，读取的是系统与用户的 PATH。
- 首次通过 Finder 打开打包版本时，若提示"无法验证开发者"，在 系统设置 → 隐私与安全性 中允许即可（未签名应用的正常提示）。

## 数据存储

项目与项目组配置以 JSON 存于系统的配置目录下：

```
macOS    ~/Library/Application Support/devbox/config.json
Windows  %APPDATA%\devbox\config.json
Linux    ~/.config/devbox/config.json
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
        ├── platform/         # 平台差异集中在这里：unix.rs / windows.rs / listen.rs（端口枚举）
        └── commands/         # projects/process/ports/hosts/services/logs/shortcuts
```

## 已知限制 / 后续可扩展

- 服务管理目前走 Homebrew，仅 macOS 可用；Docker 容器管理、本地 HTTPS 证书 (mkcert) 可作为下一步扩展。
- 日志走 1 秒轮询获取缓冲（单项目上限 2000 行）；如需持久化到文件可后续增加。
- 自动重启在健康心跳（约 2.5s）中触发。
```
