# DevBox × MCP

DevBox 内置了一个标准的 [MCP](https://modelcontextprotocol.io) 服务，让 AI agent（Claude Code、Cursor、Cline、Hermes……）可以查看和操作你的本地开发环境：看项目状态和日志、启停项目、切换环境、跑脚本、查端口占用、管理 hosts 等。

这个目录里的 `index.js` 是给**只支持 stdio 的客户端**用的转发器；支持 URL 连接的客户端（大多数）不需要它，直接连 DevBox 即可。

## 1. 接入

打开 DevBox 的「**MCP**」页面：这里有地址、token、权限档位和各客户端可直接复制的配置。

**Claude Code**

```bash
claude mcp add --transport http devbox http://127.0.0.1:8765/mcp \
  --header "Authorization: Bearer <token>"
```

之后在 Claude Code 里输入 `/mcp` 能看到 devbox 已连接。

**Cursor / Cline / 其它支持 URL 的客户端**

```json
{
  "mcpServers": {
    "devbox": {
      "type": "http",
      "url": "http://127.0.0.1:8765/mcp",
      "headers": { "Authorization": "Bearer <token>" }
    }
  }
}
```

端口默认 8765（被占用时顺延到 8784），上次用过的端口和 token 会保留，重启 DevBox 后配置不用改。

**只支持 stdio 的客户端**

```json
{
  "mcpServers": {
    "devbox": { "command": "node", "args": ["/绝对路径/devbox/devbox-mcp/index.js"] }
  }
}
```

需要 Node 18+，**不需要 `npm install`**（零依赖）。转发器每次请求都会重新读取 DevBox 写下的 `bridge.json`，所以 token 重置、端口变化都不用改配置。

检查连接：

```bash
node devbox-mcp/index.js --check
```

环境变量（可选）：`DEVBOX_MCP_URL`、`DEVBOX_MCP_TOKEN`（直接指定地址和 token）、`DEVBOX_CONFIG_DIR`（`bridge.json` 所在目录）。

> 前提：**DevBox 必须在运行**，并且「MCP」页面里的开关是开着的。

## 2. 权限档位

在「MCP」页面选择，立即生效。由 DevBox **服务端强制执行**：超出档位的工具既不会出现在 agent 的工具列表里，直接调用也会被拒绝并告诉 agent 原因。

| 档位 | 能做什么 |
| --- | --- |
| **只读** | 只能看：状态、日志、端口、git、环境等 |
| **标准**（默认） | 再加上：启停项目 / 服务、跑脚本、清日志、切换环境、git fetch |
| **完全** | 再加上：结束任意端口的进程、改 hosts、增删改项目配置、git pull / 切分支 |

## 3. 工具一览

下表的「档位」是该工具最低需要的档位。所有工具的项目参数 `project` 都接受项目名称（不区分大小写，唯一的片段也行）或 id。

### 查看（只读档）

| 工具 | 说明 |
| --- | --- |
| `devbox_overview` | **先调这个**：所有项目按组的运行状态、最近崩溃、当前权限档位 |
| `devbox_list_projects` | 项目列表，可按组 / 类型 / 是否运行 / 关键词筛选 |
| `devbox_get_project` | 项目详情：配置、状态、git、可选环境、日志概况；端口被占时指出是谁（环境变量只给名字不给值） |
| `devbox_project_status` | 运行状态：pid、时长、CPU / 内存、端口是否监听、是否在跑脚本 |
| `devbox_project_logs` | 读日志（去颜色）。`level=error` 只看报错和堆栈；`grep`（`a\|b` 多关键词）+ `context` 带上下文；`exclude` 排除噪音；`stream`；`since_seq` 增量拉取；返回 `next_since` |
| `devbox_wait_for_log` | 等日志里出现某内容（或某个级别）再返回，不用轮询；进程中途退出会立刻返回 |
| `devbox_recent_exits` | 最近崩溃的项目：退出原因 + 最后几行输出 |
| `devbox_list_groups` | 项目组及每组的运行数 |
| `devbox_list_scripts` | 项目里可运行的脚本（build / test / lint…） |
| `devbox_project_envs` | 自动识别出的环境（dev / test / prod…）及当前选中的 |
| `devbox_git_status` | 当前分支、领先 / 落后、未提交修改数 |
| `devbox_git_branches` | 本地 + 远程分支 |
| `devbox_list_runtimes` | 本机的 Node / JDK 版本 |
| `devbox_list_ports` | 监听端口（默认只列与项目相关的，`scope=all` 列全部） |
| `devbox_check_port` | 某端口被谁占用（DevBox 项目 / 外部进程） |
| `devbox_get_hosts` | DevBox 管理的 hosts 记录（`raw=true` 返回整个文件） |
| `devbox_list_services` | brew 服务（仅 macOS） |
| `devbox_detect_project` | 分析一个目录：名称、类型、启动命令、端口、Node / JDK 要求、可选环境 |

### 启停与日常操作（标准档）

| 工具 | 说明 |
| --- | --- |
| `devbox_start_project` | 启动项目：先按依赖启动它依赖的项目，等端口监听（或确认进程存活）才返回；已在运行则直接返回状态；失败带原因 |
| `devbox_stop_project` | 停止项目（整棵进程树） |
| `devbox_restart_project` | 重启项目 |
| `devbox_start_group` | 启动一个项目组 |
| `devbox_stop_group` | 并行停止一个项目组 |
| `devbox_stop_all` | 停止全部运行中的项目 |
| `devbox_clear_logs` | 清空项目日志（想让日志只含接下来的操作时先清再重启） |
| `devbox_set_env` | 切换项目环境（不传 `env` 回到默认）；运行中的项目需重启才生效 |
| `devbox_run_script` | 运行项目里识别出的脚本（**不能执行任意命令**）；最多等 `wait_secs` 秒，没结束就留在后台，用日志查看 |
| `devbox_cancel_script` | 取消正在运行的脚本 |
| `devbox_git_fetch` | `git fetch --prune`，不改工作区 |
| `devbox_start_service` / `devbox_stop_service` / `devbox_restart_service` | 启停 brew 服务（仅 macOS） |

### 影响范围更大的操作（完全档）

| 工具 | 说明 |
| --- | --- |
| `devbox_kill_port` | 结束占用某端口（或指定 pid）的进程树；拒绝结束 DevBox 自己和 pid ≤ 1 |
| `devbox_edit_hosts` | 增量改 hosts：add / remove / enable / disable（弹系统管理员授权，需要你本人确认） |
| `devbox_save_hosts` | 整体替换 DevBox 管理的 hosts 块（弹系统管理员授权） |
| `devbox_add_project` | 登记一个目录为项目；只需 `path`，其余按识别结果补全 |
| `devbox_update_project` | 修改项目配置，只改传了的字段 |
| `devbox_delete_project` | 删除项目登记（需 `confirm=true`；不动磁盘上的代码） |
| `devbox_git_pull` | `git pull --ff-only` |
| `devbox_git_checkout` | 切换分支 |

### 资源与提示词

- 资源：`devbox://overview`、`devbox://projects`、`devbox://ports`、`devbox://logs/{项目名}`（最近 200 行日志）
- 提示词：`diagnose_project`（排查项目问题）、`start_dev_stack`（拉起开发环境）、`free_port`（释放端口）、`health_check`（开发环境体检）

## 4. 典型用法

- 「后端起不来了」→ agent 会 `devbox_get_project` → `devbox_recent_exits` → `devbox_project_logs level=error context=3` →（端口冲突时）`devbox_check_port`，给出原因再建议怎么做。
- 「把前端切到 test 环境重启」→ `devbox_project_envs` → `devbox_set_env` → `devbox_restart_project` → `devbox_wait_for_log pattern=ready|listening`。
- 「帮我把整个开发环境拉起来」→ `devbox_start_group`，或 `start_dev_stack` 提示词。

## 5. 安全说明

- 只监听回环地址 `127.0.0.1`，不对外网开放；校验 `Host` / `Origin`，防止网页通过 DNS 重绑定攻击本机服务。
- 每个请求都要带 token（`Authorization: Bearer …` 或 `X-DevBox-Token`），常量时间比较。token 存在配置目录的 `bridge.json`（Unix 上权限 0600），重启 DevBox 后不变；在「MCP」页面可以重置，旧 token 立即失效。
- 权限档位和总开关在服务端强制执行；每次调用（含被拒绝的）都会记在「MCP」页面的调用记录里。
- 项目的环境变量值不会通过 MCP 返回；`devbox_run_script` 只能运行从项目文件里识别出的脚本；hosts 的 ip / 域名在写入前会校验，防止注入。
- 日志里可能含有密钥、手机号等敏感信息，会被 agent 读到。只把 DevBox 接入你信任的 agent。

## 6. 排错

| 现象 | 处理 |
| --- | --- |
| 连不上 | 确认 DevBox 在运行、「MCP」页面开关是开的；`node devbox-mcp/index.js --check` 看具体原因 |
| 401 / token 不对 | 重置过 token 后旧的会失效，从页面重新复制；stdio 转发器不受影响 |
| 工具列表里缺少某个工具 | 当前档位不够，在页面里调高；agent 调用时会收到说明 |
| 端口被占用，DevBox 没起来 | 8765–8784 都被占时服务无法启动，释放其中一个后重启 DevBox |
