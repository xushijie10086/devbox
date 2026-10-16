// DevBox MCP 的 stdio 转发逻辑。
//
// DevBox 应用本身就是一个标准的 MCP 服务器（Streamable HTTP，地址见应用里的「MCP」页面），
// 工具、资源、提示词都由它提供。这个文件只负责把 stdio 上的 JSON-RPC 消息原样转发过去，
// 所以应用升级后 agent 看到的能力自动跟着升级，不需要更新适配器。
// 没有任何第三方依赖，也不需要 npm install。

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { configDir } from "./paths.js";

/** 读取 DevBox 写下的 bridge.json；读不到返回 null */
export function readBridge(env = process.env, platform = process.platform) {
  const dir = env.DEVBOX_CONFIG_DIR || join(configDir(platform, env), "devbox");
  try {
    return JSON.parse(readFileSync(join(dir, "bridge.json"), "utf8"));
  } catch {
    return null;
  }
}

/**
 * 连接目标。环境变量优先（DEVBOX_MCP_URL / DEVBOX_MCP_TOKEN），其次是 bridge.json。
 * 每个请求都重新读一遍：DevBox 重启后端口、token 可能变了，不需要重启 agent。
 */
export function resolveTarget(env = process.env, platform = process.platform) {
  const bridge = readBridge(env, platform);
  const url = env.DEVBOX_MCP_URL || bridge?.mcp_url || (bridge?.port ? `http://127.0.0.1:${bridge.port}/mcp` : "");
  const token = env.DEVBOX_MCP_TOKEN || bridge?.token || "";
  return { url, token, fromBridge: !env.DEVBOX_MCP_URL };
}

const DOWN = "连不上 DevBox。请确认 DevBox 应用正在运行（菜单栏 / 托盘里有图标），并且在它的「MCP」页面里已开启。";

function rpcError(id, code, message) {
  return { jsonrpc: "2.0", id: id ?? null, error: { code, message } };
}

/** 连接失败时的应答：工具调用走 isError，让模型读到原因；其它请求走 JSON-RPC 错误 */
function failure(msg, text) {
  if (msg?.method === "tools/call") {
    return { jsonrpc: "2.0", id: msg.id, result: { content: [{ type: "text", text }], isError: true } };
  }
  return rpcError(msg?.id, -32000, text);
}

/**
 * 转发一条（或一批）JSON-RPC 消息到 DevBox。
 * 返回要写回 stdout 的消息（对象或数组），或 null（通知，没有应答）。
 */
export async function forward(msg, { env = process.env, fetchImpl = globalThis.fetch, client = "devbox-mcp-proxy", timeoutMs = 10 * 60 * 1000 } = {}) {
  const messages = Array.isArray(msg) ? msg : [msg];
  const { url, token } = resolveTarget(env);
  const reply = (text) => (Array.isArray(msg) ? messages.filter((m) => m?.id !== undefined).map((m) => failure(m, text)) : failure(msg, text));

  if (!url) return reply(DOWN + "（没有找到 bridge.json，也没有设置 DEVBOX_MCP_URL）");

  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), timeoutMs);
  let res;
  try {
    res = await fetchImpl(url, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        accept: "application/json, text/event-stream",
        authorization: `Bearer ${token}`,
        "x-devbox-client": client,
      },
      body: JSON.stringify(msg),
      signal: ctl.signal,
    });
  } catch (e) {
    clearTimeout(timer);
    if (e?.name === "AbortError") return reply(`DevBox 在 ${Math.round(timeoutMs / 1000)} 秒内没有应答，已放弃等待。`);
    return reply(DOWN);
  }
  clearTimeout(timer);

  if (res.status === 202) return null;
  if (res.status === 401) return reply("DevBox 拒绝了访问：token 不对。在 DevBox 的「MCP」页面复制最新的 token（重置过 token 后旧的会失效），或删掉 DEVBOX_MCP_TOKEN 让适配器读 bridge.json。");
  if (res.status === 403) {
    let why = "";
    try { why = (await res.json()).error ?? ""; } catch { /* 不是 JSON */ }
    return reply(`DevBox 拒绝了请求：${why || "403"}`);
  }
  let body;
  try {
    body = await res.json();
  } catch {
    return reply(`DevBox 返回了无法解析的内容（HTTP ${res.status}）`);
  }
  // 协议层错误 / 正常应答都原样转交
  if (res.status >= 400 && !body?.jsonrpc) return reply(`DevBox 返回 HTTP ${res.status}：${body?.error ?? ""}`);
  return body;
}

/** 从 initialize 里取客户端名字，之后的请求带上，DevBox 的调用记录里就能看出是谁在调 */
export function clientNameOf(msg) {
  const m = Array.isArray(msg) ? msg.find((x) => x?.method === "initialize") : msg;
  if (m?.method !== "initialize") return null;
  const n = m.params?.clientInfo?.name;
  return typeof n === "string" && n ? n.slice(0, 40) : null;
}
