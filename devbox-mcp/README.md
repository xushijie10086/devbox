# devbox-mcp

DevBox 的 MCP 适配器。把 DevBox 内置的本地 HTTP 桥接服务暴露成一组 MCP 工具，让 agent（claude code / Hermes 等）能够：

- 列出项目、查看运行状态（pid / CPU / 内存 / 端口是否已监听）
- 启动 / 停止 / 重启项目
- 读取项目运行日志
- 查看端口占用、结束占用进程
- 读取 / 写入 DevBox 管理的 hosts 记录
- 列出 / 启停 / 重启 brew 本地服务

## 工作原理

DevBox 启动时会在 `127.0.0.1` 上开一个仅本机可访问的 HTTP 服务（端口在 8765-8784 之间自动选择），并把端口与访问 token 写入：

```
~/Library/Application Support/devbox/bridge.json
```

本适配器读取该文件连接并鉴权，再通过标准输入输出（stdio）与 agent 通信。

> 前提：**DevBox 必须处于运行状态**，适配器才能连上。

## 安装

```bash
cd devbox-mcp
npm install
```

需要 Node.js 18+（依赖内置 `fetch`）。

## 在 Claude Code 中配置

```bash
claude mcp add devbox -- node /绝对路径/devbox/devbox-mcp/index.js
```

或手动写入 MCP 配置（如 `~/.claude.json` 或项目 `.mcp.json`）：

```json
{
  "mcpServers": {
    "devbox": {
      "command": "node",
      "args": ["/绝对路径/devbox/devbox-mcp/index.js"]
    }
  }
}
```

配置好后，在 Claude Code 里即可直接说「启动 admin 项目」「重启云应用商店」「看下 8083 端口被谁占了」等。

## 工具一览

| 工具 | 说明 |
| --- | --- |
| `devbox_list_projects` | 列出所有项目 |
| `devbox_project_status` | 所有项目运行状态 |
| `devbox_start_project` | 启动项目（id 或 name） |
| `devbox_stop_project` | 停止项目 |
| `devbox_restart_project` | 重启项目 |
| `devbox_project_logs` | 读取项目日志（可 tail） |
| `devbox_detect_project` | 探测目录的项目类型 |
| `devbox_list_ports` | 列出监听端口 |
| `devbox_kill_port` | 结束占用端口的进程 |
| `devbox_get_hosts` | 读取 hosts 记录 |
| `devbox_save_hosts` | 写入 hosts（需管理员授权） |
| `devbox_list_services` | 列出 brew 服务 |
| `devbox_start_service` / `devbox_stop_service` / `devbox_restart_service` | 启停 brew 服务 |

## 安全说明

- HTTP 服务只绑定回环地址 `127.0.0.1`，不对外网开放。
- 每次 DevBox 启动都会生成新的随机 token，写在 `bridge.json` 里，接口调用需带 `X-DevBox-Token` 头。
