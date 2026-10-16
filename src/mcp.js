// MCP 页面的纯逻辑：接入配置的生成、token 遮盖、工具分组。不碰 DOM，方便测试。

export const MODES = [
  { id: "readonly", label: "只读", hint: "只能看：状态、日志、端口、git 信息。agent 不能改变任何东西。" },
  { id: "standard", label: "标准", hint: "在只读之上，可以启停项目和服务、跑脚本、清日志、切换环境、git fetch。日常推荐。" },
  { id: "full", label: "完全", hint: "再加上：结束任意端口的进程、改 hosts、增删改项目配置、git pull / 切分支。只在你信任这个 agent 时使用。" },
];

export function modeLabel(id) {
  return MODES.find((m) => m.id === id)?.label ?? id;
}

/** 屏幕上显示用：只露出前 6 位和后 4 位 */
export function maskToken(token) {
  if (!token) return "";
  if (token.length <= 12) return "•".repeat(token.length);
  return `${token.slice(0, 6)}••••••••${token.slice(-4)}`;
}

/** shell 里单引号包一下，路径 / token 里有空格、$ 之类也安全 */
function shq(s) {
  return /^[A-Za-z0-9_@%+=:,./-]+$/.test(s) ? s : `'${String(s).replace(/'/g, `'\\''`)}'`;
}

/**
 * 各种客户端的接入配置。mask=true 时 token 用遮盖版（给屏幕显示），false 时是真实值（给复制）。
 * 返回 [{ id, title, hint, lang, text }]
 */
export function clientConfigs(info, { mask = false } = {}) {
  if (!info?.url) return [];
  const token = mask ? maskToken(info.token) : info.token;
  const auth = `Bearer ${token}`;
  const httpJson = (withType) =>
    JSON.stringify({ mcpServers: { devbox: { ...(withType ? { type: "http" } : {}), url: info.url, headers: { Authorization: auth } } } }, null, 2);
  return [
    {
      id: "claude-code",
      title: "Claude Code",
      hint: "在终端执行一次。想只在某个项目里用，加 --scope project；之后在 Claude Code 里输入 /mcp 可以看到连接状态。",
      lang: "shell",
      text: `claude mcp add --transport http devbox ${shq(info.url)} --header ${shq(`Authorization: ${auth}`)}`,
    },
    {
      id: "json",
      title: "Cursor / Cline / 其它",
      hint: "写进客户端的 MCP 配置文件（Cursor 是 ~/.cursor/mcp.json，Claude Code 项目级是 .mcp.json）。支持 URL 方式的客户端都能用。",
      lang: "json",
      text: httpJson(true),
    },
    {
      id: "stdio",
      title: "只支持 stdio 的客户端",
      hint: "需要 Node 18+。index.js 在 DevBox 源码的 devbox-mcp 目录里，零依赖、不用 npm install；它会自动读取 DevBox 写下的连接信息，token 变了也不用改配置。",
      lang: "json",
      text: JSON.stringify({ mcpServers: { devbox: { command: "node", args: ["/绝对路径/devbox/devbox-mcp/index.js"] } } }, null, 2),
    },
  ];
}

const RISK_ORDER = ["read", "write", "danger"];
export const RISK_LABEL = { read: "只读", write: "标准", danger: "完全" };
const RISK_TITLE = {
  read: "查看信息",
  write: "启停与日常操作",
  danger: "影响范围更大的操作",
};

/** 工具按风险分组，保持后端给出的顺序 */
export function groupTools(tools) {
  return RISK_ORDER.map((risk) => ({
    risk,
    label: RISK_LABEL[risk],
    title: RISK_TITLE[risk],
    tools: (tools ?? []).filter((t) => t.risk === risk),
  })).filter((g) => g.tools.length > 0);
}

/** 当前档位下可用 / 总数，如 "29 / 38" */
export function toolSummary(tools) {
  const list = tools ?? [];
  return `${list.filter((t) => t.allowed).length} / ${list.length}`;
}

/** 调用记录的一行摘要 */
export function activityText(a) {
  const target = a.target ? ` · ${a.target}` : "";
  const err = a.ok ? "" : ` — ${a.error ?? "失败"}`;
  return `${a.ts} ${a.tool}${target} (${a.ms}ms)${err}`;
}
