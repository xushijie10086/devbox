#!/usr/bin/env node
// DevBox MCP 适配器（stdio）
//
// 读取 DevBox 写在 ~/Library/Application Support/devbox/bridge.json 里的端口与 token，
// 把本地 HTTP 桥接服务转发成一组 MCP 工具。DevBox 必须处于运行状态。

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const BRIDGE_FILE = join(
  homedir(),
  "Library",
  "Application Support",
  "devbox",
  "bridge.json"
);

function readBridge() {
  try {
    const raw = readFileSync(BRIDGE_FILE, "utf8");
    const j = JSON.parse(raw);
    if (!j.base_url || !j.token) throw new Error("bridge.json 缺少 base_url 或 token");
    return j;
  } catch (e) {
    throw new Error(
      `无法读取 DevBox 桥接信息（${BRIDGE_FILE}）。请确认 DevBox 正在运行。原始错误：${e.message}`
    );
  }
}

// 调用 DevBox 本地 HTTP 接口，返回 data；出错时抛出可读错误
async function call(path, body = {}) {
  const bridge = readBridge();
  let res;
  try {
    res = await fetch(bridge.base_url + path, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-DevBox-Token": bridge.token,
      },
      body: JSON.stringify(body ?? {}),
    });
  } catch (e) {
    throw new Error(`连接 DevBox 失败：${e.message}（DevBox 是否在运行？）`);
  }
  const text = await res.text();
  let j;
  try {
    j = JSON.parse(text);
  } catch {
    throw new Error(`DevBox 返回了非 JSON 响应：${text.slice(0, 200)}`);
  }
  if (!j.ok) throw new Error(j.error || "未知错误");
  return j.data;
}

// 统一把结果包成 MCP 文本内容
function textResult(data) {
  const text =
    typeof data === "string" ? data : JSON.stringify(data, null, 2);
  return { content: [{ type: "text", text: text || "(无内容)" }] };
}

function errResult(e) {
  return {
    isError: true,
    content: [{ type: "text", text: `错误：${e.message}` }],
  };
}

// 包装：把一次 HTTP 调用注册为工具处理器
function handler(path, mapArgs) {
  return async (args) => {
    try {
      const body = mapArgs ? mapArgs(args) : args;
      const data = await call(path, body);
      return textResult(data);
    } catch (e) {
      return errResult(e);
    }
  };
}

const server = new McpServer({ name: "devbox", version: "1.0.0" });

const projectSelector = {
  id: z.string().optional().describe("项目 id（与 name 二选一）"),
  name: z.string().optional().describe("项目名称（与 id 二选一）"),
};

// ---- 项目 ----
server.tool(
  "devbox_list_projects",
  "列出 DevBox 管理的所有项目（含 id、名称、路径、启动命令、端口）。",
  {},
  handler("/projects/list")
);
server.tool(
  "devbox_project_status",
  "查看所有项目的运行状态：是否在跑、pid、CPU、内存、运行时长、端口是否已监听。",
  {},
  handler("/projects/status")
);
server.tool(
  "devbox_start_project",
  "启动一个项目（按 id 或 name 指定）。后端项目首次启动会先编译，可能耗时较久。",
  projectSelector,
  handler("/projects/start")
);
server.tool(
  "devbox_stop_project",
  "停止一个项目（结束其整棵进程树）。",
  projectSelector,
  handler("/projects/stop")
);
server.tool(
  "devbox_restart_project",
  "重启一个项目（先停止再启动）。",
  projectSelector,
  handler("/projects/restart")
);
server.tool(
  "devbox_project_logs",
  "读取某项目最近的运行日志（stdout/stderr/system），可用 tail 只取末尾若干行。",
  { ...projectSelector, tail: z.number().int().positive().optional().describe("只取末尾多少行") },
  handler("/projects/logs")
);
server.tool(
  "devbox_detect_project",
  "探测某工作目录的项目类型，返回建议的名称/启动命令/端口，便于新增项目。",
  { path: z.string().describe("项目工作目录的绝对路径") },
  handler("/projects/detect")
);

// ---- 端口 ----
server.tool(
  "devbox_list_ports",
  "列出本机所有处于 LISTEN 状态的 TCP 端口占用。",
  {},
  handler("/ports/list")
);
server.tool(
  "devbox_kill_port",
  "结束占用某端口的进程（会结束整棵进程树并核验端口释放）。",
  {
    pid: z.number().int().describe("lsof 报告的进程 pid"),
    port: z.number().int().optional().describe("要释放的端口，便于核验"),
  },
  handler("/ports/kill")
);

// ---- hosts ----
server.tool(
  "devbox_get_hosts",
  "读取 DevBox 管理块内的 /etc/hosts 记录。",
  {},
  handler("/hosts/get")
);
server.tool(
  "devbox_save_hosts",
  "用给定记录重建 DevBox 管理的 hosts 块（需要管理员授权，会弹出系统密码框）。",
  {
    entries: z
      .array(
        z.object({
          ip: z.string(),
          hostnames: z.array(z.string()),
          enabled: z.boolean(),
          comment: z.string().optional(),
        })
      )
      .describe("完整的 hosts 记录列表"),
  },
  handler("/hosts/save")
);

// ---- brew 服务 ----
server.tool(
  "devbox_list_services",
  "列出 Homebrew 管理的本地服务及其状态。",
  {},
  handler("/services/list")
);
server.tool(
  "devbox_start_service",
  "启动一个 brew 服务。",
  { name: z.string().describe("服务名，如 mysql、redis") },
  handler("/services/start")
);
server.tool(
  "devbox_stop_service",
  "停止一个 brew 服务。",
  { name: z.string().describe("服务名") },
  handler("/services/stop")
);
server.tool(
  "devbox_restart_service",
  "重启一个 brew 服务。",
  { name: z.string().describe("服务名") },
  handler("/services/restart")
);

const transport = new StdioServerTransport();
await server.connect(transport);
console.error("[devbox-mcp] 已连接，等待 MCP 请求…");
