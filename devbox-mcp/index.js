#!/usr/bin/env node
// DevBox MCP stdio 转发器。
//
// 大多数 AI 客户端（Claude Code、Cursor…）可以直接用 URL 连接 DevBox，不需要这个文件；
// 只有客户端只支持 stdio 时才用它：
//   claude mcp add devbox -- node /绝对路径/devbox/devbox-mcp/index.js
//
// 用法：
//   node index.js            作为 MCP 服务器运行（stdio）
//   node index.js --check    检查能否连上 DevBox，并列出可用工具
//   node index.js --help

import { createInterface } from "node:readline";
import { forward, resolveTarget, clientNameOf } from "./proxy.js";

const arg = process.argv[2];

if (arg === "--help" || arg === "-h") {
  console.log(`devbox-mcp — DevBox 的 MCP stdio 转发器

  node index.js           作为 MCP 服务器运行（stdio）
  node index.js --check   检查连接并列出当前权限档位下可用的工具

环境变量（可选，默认读 DevBox 写下的 bridge.json）：
  DEVBOX_MCP_URL      服务地址，如 http://127.0.0.1:8765/mcp
  DEVBOX_MCP_TOKEN    访问 token
  DEVBOX_CONFIG_DIR   bridge.json 所在目录`);
  process.exit(0);
}

if (arg === "--check") {
  const { url } = resolveTarget();
  console.log(`DevBox 地址：${url || "（没找到）"}`);
  const init = await forward({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "devbox-mcp --check", version: "1" } } });
  if (init?.error || init?.result?.isError || !init?.result) {
    console.error("✖ " + (init?.error?.message ?? init?.result?.content?.[0]?.text ?? "连接失败"));
    process.exit(1);
  }
  const list = await forward({ jsonrpc: "2.0", id: 2, method: "tools/list" });
  const tools = list?.result?.tools ?? [];
  console.log(`✔ 已连接 ${init.result.serverInfo.name} ${init.result.serverInfo.version}，当前档位下有 ${tools.length} 个工具：`);
  console.log(tools.map((t) => "  " + t.name).join("\n"));
  process.exit(0);
}

let client = "devbox-mcp-proxy";
let pending = 0;
let closed = false;
const done = () => {
  if (closed && pending === 0) process.exit(0);
};

const rl = createInterface({ input: process.stdin, crlfDelay: Infinity });
rl.on("line", (line) => {
  const text = line.trim();
  if (!text) return;
  let msg;
  try {
    msg = JSON.parse(text);
  } catch {
    process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: null, error: { code: -32700, message: "不是合法的 JSON" } }) + "\n");
    return;
  }
  client = clientNameOf(msg) ?? client;
  pending++;
  // 各条消息并发处理：一个慢请求（等日志、跑脚本）不会堵住别的
  forward(msg, { client })
    .then((reply) => {
      if (reply !== null && reply !== undefined) process.stdout.write(JSON.stringify(reply) + "\n");
    })
    .catch((e) => console.error("[devbox-mcp]", e))
    .finally(() => {
      pending--;
      done();
    });
});
rl.on("close", () => {
  closed = true;
  done();
});
