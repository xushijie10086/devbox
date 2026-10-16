import test from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import { spawn } from "node:child_process";
import { mkdtempSync, writeFileSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { forward, resolveTarget, readBridge, clientNameOf } from "../devbox-mcp/proxy.js";

const here = dirname(fileURLToPath(import.meta.url));
const indexJs = join(here, "..", "devbox-mcp", "index.js");

/** 起一个假的 DevBox：记录收到的请求，按 handler 的返回应答 */
async function fakeDevBox(handler) {
  const seen = [];
  const server = http.createServer((req, res) => {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      const parsed = body ? JSON.parse(body) : null;
      seen.push({ method: req.method, url: req.url, headers: req.headers, body: parsed });
      const r = handler(parsed, req) ?? { status: 200, json: { jsonrpc: "2.0", id: parsed?.id, result: {} } };
      res.writeHead(r.status ?? 200, { "content-type": "application/json" });
      res.end(r.json === undefined ? "" : JSON.stringify(r.json));
    });
  });
  await new Promise((ok) => server.listen(0, "127.0.0.1", ok));
  const port = server.address().port;
  return { port, url: `http://127.0.0.1:${port}/mcp`, seen, close: () => new Promise((ok) => server.close(ok)) };
}

const env = (extra = {}) => ({ DEVBOX_CONFIG_DIR: mkdtempSync(join(tmpdir(), "devbox-mcp-")), ...extra });

test("没有任何配置：工具调用得到可读的错误，其它请求得到 JSON-RPC 错误", async () => {
  const e = env();
  const call = await forward({ jsonrpc: "2.0", id: 3, method: "tools/call", params: { name: "devbox_overview" } }, { env: e });
  assert.equal(call.id, 3);
  assert.equal(call.result.isError, true);
  assert.match(call.result.content[0].text, /DevBox/);

  const other = await forward({ jsonrpc: "2.0", id: 4, method: "tools/list" }, { env: e });
  assert.equal(other.error.code, -32000);
  assert.equal(other.id, 4);
});

test("读取 bridge.json 的地址和 token；环境变量优先", () => {
  const dir = mkdtempSync(join(tmpdir(), "devbox-mcp-"));
  writeFileSync(join(dir, "bridge.json"), JSON.stringify({ port: 8771, token: "from-file", mcp_url: "http://127.0.0.1:8771/mcp" }));
  assert.deepEqual(readBridge({ DEVBOX_CONFIG_DIR: dir })?.port, 8771);
  assert.equal(resolveTarget({ DEVBOX_CONFIG_DIR: dir }).url, "http://127.0.0.1:8771/mcp");
  assert.equal(resolveTarget({ DEVBOX_CONFIG_DIR: dir }).token, "from-file");
  const over = resolveTarget({ DEVBOX_CONFIG_DIR: dir, DEVBOX_MCP_URL: "http://127.0.0.1:9/mcp", DEVBOX_MCP_TOKEN: "env-token" });
  assert.equal(over.url, "http://127.0.0.1:9/mcp");
  assert.equal(over.token, "env-token");
  // 旧版 bridge.json 没有 mcp_url：由端口拼出来
  writeFileSync(join(dir, "bridge.json"), JSON.stringify({ port: 8800, token: "t" }));
  assert.equal(resolveTarget({ DEVBOX_CONFIG_DIR: dir }).url, "http://127.0.0.1:8800/mcp");
  assert.equal(readBridge({ DEVBOX_CONFIG_DIR: join(dir, "nope") }), null);
});

test("转发请求：带 token 和客户端名，原样返回应答", async () => {
  const box = await fakeDevBox((m) => ({ json: { jsonrpc: "2.0", id: m.id, result: { tools: [{ name: "devbox_overview" }] } } }));
  try {
    const reply = await forward({ jsonrpc: "2.0", id: 9, method: "tools/list" }, { env: env({ DEVBOX_MCP_URL: box.url, DEVBOX_MCP_TOKEN: "secret" }), client: "claude-code" });
    assert.deepEqual(reply, { jsonrpc: "2.0", id: 9, result: { tools: [{ name: "devbox_overview" }] } });
    const req = box.seen[0];
    assert.equal(req.method, "POST");
    assert.equal(req.url, "/mcp");
    assert.equal(req.headers.authorization, "Bearer secret");
    assert.equal(req.headers["x-devbox-client"], "claude-code");
    assert.match(req.headers.accept, /application\/json/);
  } finally {
    await box.close();
  }
});

test("通知：服务端回 202，转发器不输出任何内容", async () => {
  const box = await fakeDevBox(() => ({ status: 202 }));
  try {
    const reply = await forward({ jsonrpc: "2.0", method: "notifications/initialized" }, { env: env({ DEVBOX_MCP_URL: box.url, DEVBOX_MCP_TOKEN: "t" }) });
    assert.equal(reply, null);
  } finally {
    await box.close();
  }
});

test("批量请求整体转发", async () => {
  const box = await fakeDevBox((m) => ({ json: m.map((x) => ({ jsonrpc: "2.0", id: x.id, result: {} })) }));
  try {
    const reply = await forward(
      [{ jsonrpc: "2.0", id: 1, method: "ping" }, { jsonrpc: "2.0", id: 2, method: "ping" }],
      { env: env({ DEVBOX_MCP_URL: box.url, DEVBOX_MCP_TOKEN: "t" }) },
    );
    assert.equal(reply.length, 2);
    assert.deepEqual(box.seen[0].body.map((x) => x.id), [1, 2]);
  } finally {
    await box.close();
  }
});

test("401 / 403 给出能照着做的提示", async () => {
  let status = 401;
  const box = await fakeDevBox(() => ({ status, json: { error: "DevBox 的 MCP 服务已被用户关闭" } }));
  try {
    const e = env({ DEVBOX_MCP_URL: box.url, DEVBOX_MCP_TOKEN: "wrong" });
    const r401 = await forward({ jsonrpc: "2.0", id: 1, method: "tools/call", params: {} }, { env: e });
    assert.equal(r401.result.isError, true);
    assert.match(r401.result.content[0].text, /token/);

    status = 403;
    const r403 = await forward({ jsonrpc: "2.0", id: 2, method: "tools/call", params: {} }, { env: e });
    assert.match(r403.result.content[0].text, /已被用户关闭/);
  } finally {
    await box.close();
  }
});

test("连接被拒绝（DevBox 没开）", async () => {
  const box = await fakeDevBox(() => ({}));
  const url = box.url;
  await box.close();
  const reply = await forward({ jsonrpc: "2.0", id: 5, method: "tools/call", params: {} }, { env: env({ DEVBOX_MCP_URL: url, DEVBOX_MCP_TOKEN: "t" }) });
  assert.equal(reply.result.isError, true);
  assert.match(reply.result.content[0].text, /正在运行/);
});

test("超时会放弃等待并说明", async () => {
  const server = http.createServer(() => {}); // 永远不应答
  await new Promise((ok) => server.listen(0, "127.0.0.1", ok));
  try {
    const url = `http://127.0.0.1:${server.address().port}/mcp`;
    const reply = await forward({ jsonrpc: "2.0", id: 6, method: "ping" }, { env: env({ DEVBOX_MCP_URL: url, DEVBOX_MCP_TOKEN: "t" }), timeoutMs: 200 });
    assert.equal(reply.error.code, -32000);
    assert.match(reply.error.message, /没有应答/);
  } finally {
    server.closeAllConnections?.();
    await new Promise((ok) => server.close(ok));
  }
});

test("服务端返回协议错误时原样转交", async () => {
  const err = { jsonrpc: "2.0", id: 1, error: { code: -32601, message: "不支持的方法" } };
  const box = await fakeDevBox(() => ({ status: 200, json: err }));
  try {
    const reply = await forward({ jsonrpc: "2.0", id: 1, method: "x" }, { env: env({ DEVBOX_MCP_URL: box.url, DEVBOX_MCP_TOKEN: "t" }) });
    assert.deepEqual(reply, err);
  } finally {
    await box.close();
  }
});

test("clientNameOf 只从 initialize 里取名字", () => {
  assert.equal(clientNameOf({ method: "initialize", params: { clientInfo: { name: "cursor" } } }), "cursor");
  assert.equal(clientNameOf([{ method: "ping" }, { method: "initialize", params: { clientInfo: { name: "cline" } } }]), "cline");
  assert.equal(clientNameOf({ method: "tools/list" }), null);
  assert.equal(clientNameOf({ method: "initialize", params: {} }), null);
  assert.equal(clientNameOf({ method: "initialize", params: { clientInfo: { name: "x".repeat(100) } } }).length, 40);
});

/** 起真正的 index.js 进程，按行喂消息，收集 stdout */
function runProxy(envVars, lines, { args = [] } = {}) {
  return new Promise((resolve) => {
    const p = spawn(process.execPath, [indexJs, ...args], { env: { ...process.env, ...envVars }, stdio: ["pipe", "pipe", "pipe"] });
    let out = "", err = "";
    p.stdout.on("data", (c) => (out += c));
    p.stderr.on("data", (c) => (err += c));
    const guard = setTimeout(() => p.kill(), 15000); // 卡住时别让整个测试一直挂着
    p.on("close", (code) => {
      clearTimeout(guard);
      const replies = out.split("\n").filter(Boolean).flatMap((l) => { try { return [JSON.parse(l)]; } catch { return []; } });
      resolve({ code, out, err, replies });
    });
    for (const l of lines) p.stdin.write(l + "\n");
    p.stdin.end();
  });
}

test("stdio 端到端：多条消息并发转发，通知无输出，坏 JSON 得到解析错误", async () => {
  const box = await fakeDevBox((m) => (m?.method === "notifications/initialized" ? { status: 202 } : { json: { jsonrpc: "2.0", id: m.id, result: { echoed: m.method } } }));
  try {
    const r = await runProxy(
      { DEVBOX_MCP_URL: box.url, DEVBOX_MCP_TOKEN: "t", DEVBOX_CONFIG_DIR: env().DEVBOX_CONFIG_DIR },
      [
        JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: { clientInfo: { name: "my-agent" } } }),
        JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }),
        JSON.stringify({ jsonrpc: "2.0", id: 2, method: "tools/list" }),
        "{坏的",
        "",
      ],
    );
    assert.equal(r.code, 0, r.err);
    const byId = Object.fromEntries(r.replies.filter((x) => x.id !== null).map((x) => [x.id, x]));
    assert.equal(byId[1].result.echoed, "initialize");
    assert.equal(byId[2].result.echoed, "tools/list");
    assert.equal(r.replies.find((x) => x.id === null).error.code, -32700);
    assert.equal(r.replies.length, 3, "通知不产生输出");
    assert.ok(box.seen.every((s) => s.headers.authorization === "Bearer t"));
    // 初始化之后的请求带上了客户端自报的名字
    assert.equal(box.seen.find((s) => s.body?.method === "tools/list").headers["x-devbox-client"], "my-agent");
  } finally {
    await box.close();
  }
});

test("--check 能连上时列出工具，连不上时退出码非 0", async () => {
  const box = await fakeDevBox((m) =>
    m.method === "initialize"
      ? { json: { jsonrpc: "2.0", id: m.id, result: { serverInfo: { name: "devbox", version: "9.9.9" } } } }
      : { json: { jsonrpc: "2.0", id: m.id, result: { tools: [{ name: "devbox_overview" }, { name: "devbox_project_logs" }] } } },
  );
  const cfg = env().DEVBOX_CONFIG_DIR;
  try {
    const ok = await runProxy({ DEVBOX_MCP_URL: box.url, DEVBOX_MCP_TOKEN: "t", DEVBOX_CONFIG_DIR: cfg }, [], { args: ["--check"] });
    assert.equal(ok.code, 0, ok.err);
    assert.match(ok.out, /2 个工具/);
    assert.match(ok.out, /devbox_project_logs/);
  } finally {
    await box.close();
  }
  const bad = await runProxy({ DEVBOX_MCP_URL: "http://127.0.0.1:1/mcp", DEVBOX_MCP_TOKEN: "t", DEVBOX_CONFIG_DIR: cfg }, [], { args: ["--check"] });
  assert.equal(bad.code, 1);
  assert.match(bad.err, /✖/);
});

test("转发器没有第三方依赖", () => {
  const pkg = JSON.parse(readFileSync(join(here, "..", "devbox-mcp", "package.json"), "utf8"));
  assert.equal(pkg.dependencies, undefined);
  assert.equal(pkg.devDependencies, undefined);
});
