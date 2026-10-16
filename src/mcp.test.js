import test from "node:test";
import assert from "node:assert/strict";
import { MODES, modeLabel, maskToken, clientConfigs, groupTools, toolSummary, activityText } from "./mcp.js";

const info = { url: "http://127.0.0.1:8765/mcp", token: "abcdef0123456789abcdef0123456789" };

test("档位：三档，顺序从严到宽", () => {
  assert.deepEqual(MODES.map((m) => m.id), ["readonly", "standard", "full"]);
  assert.equal(modeLabel("standard"), "标准");
  assert.equal(modeLabel("other"), "other");
});

test("token 遮盖只露出头尾", () => {
  assert.equal(maskToken(info.token), "abcdef••••••••6789");
  assert.equal(maskToken("short"), "•••••");
  assert.equal(maskToken(""), "");
});

test("接入配置：显示版遮盖 token，复制版是真实值", () => {
  const shown = clientConfigs(info, { mask: true });
  const real = clientConfigs(info);
  assert.deepEqual(shown.map((c) => c.id), ["claude-code", "json", "stdio"]);
  for (const c of shown.filter((c) => c.id !== "stdio")) {
    assert.ok(!c.text.includes(info.token), `${c.id} 不该露出完整 token`);
    assert.ok(c.text.includes("abcdef"), c.id);
  }
  for (const c of real.filter((c) => c.id !== "stdio")) assert.ok(c.text.includes(info.token), c.id);
});

test("Claude Code 命令可直接粘贴到 shell", () => {
  const cmd = clientConfigs(info).find((c) => c.id === "claude-code").text;
  assert.equal(cmd, `claude mcp add --transport http devbox http://127.0.0.1:8765/mcp --header 'Authorization: Bearer ${info.token}'`);
});

test("shell 引用：token 里有单引号也不会被截断", () => {
  const cmd = clientConfigs({ url: "http://x/mcp", token: "a'b c" }).find((c) => c.id === "claude-code").text;
  assert.ok(cmd.includes(`'Authorization: Bearer a'\\''b c'`), cmd);
});

test("JSON 配置合法且字段正确", () => {
  const cfg = JSON.parse(clientConfigs(info).find((c) => c.id === "json").text);
  assert.deepEqual(cfg.mcpServers.devbox, { type: "http", url: info.url, headers: { Authorization: `Bearer ${info.token}` } });
  const stdio = JSON.parse(clientConfigs(info).find((c) => c.id === "stdio").text);
  assert.equal(stdio.mcpServers.devbox.command, "node");
  assert.ok(!JSON.stringify(stdio).includes(info.token), "stdio 方式不需要 token");
});

test("服务还没起来（没有地址）时不给配置", () => {
  assert.deepEqual(clientConfigs({ url: "", token: "" }), []);
  assert.deepEqual(clientConfigs(null), []);
});

test("工具按风险分组并统计可用数", () => {
  const tools = [
    { name: "a", risk: "write", allowed: true },
    { name: "b", risk: "read", allowed: true },
    { name: "c", risk: "danger", allowed: false },
    { name: "d", risk: "read", allowed: true },
  ];
  const groups = groupTools(tools);
  assert.deepEqual(groups.map((g) => [g.risk, g.tools.map((t) => t.name)]), [["read", ["b", "d"]], ["write", ["a"]], ["danger", ["c"]]]);
  assert.equal(toolSummary(tools), "3 / 4");
  assert.deepEqual(groupTools([{ name: "x", risk: "read", allowed: true }]).map((g) => g.risk), ["read"]);
  assert.deepEqual(groupTools(undefined), []);
});

test("调用记录摘要", () => {
  assert.equal(activityText({ ts: "10:00:01", tool: "devbox_start_project", target: "api", ok: true, ms: 1234 }), "10:00:01 devbox_start_project · api (1234ms)");
  assert.equal(activityText({ ts: "10:00:02", tool: "devbox_kill_port", target: null, ok: false, ms: 3, error: "权限档位不足" }), "10:00:02 devbox_kill_port (3ms) — 权限档位不足");
});
