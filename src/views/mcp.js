// MCP 页面：让 AI agent（Claude Code、Cursor…）连接并操作 DevBox。
// 这里管三件事：开关与权限档位、怎么接入（可直接复制的配置）、agent 做过什么（调用记录）。

import { api } from "../api.js";
import { el, guard, toast, confirmDialog } from "../ui.js";
import { copyText } from "../logview.js";
import { MODES, RISK_LABEL, activityText, clientConfigs, groupTools, maskToken, modeLabel, toolSummary } from "../mcp.js";

export function mount(root) {
  let info = null;
  let activity = [];
  let revealToken = false;
  let activeClient = "claude-code";
  let timer = null;
  let destroyed = false;

  const page = el("div", { class: "mcp-page" });
  root.append(page);

  async function loadInfo() {
    try {
      info = await api.mcpInfo();
    } catch (e) {
      page.replaceChildren(el("div", { class: "empty" }, `无法读取 MCP 状态：${e}`));
      return false;
    }
    return true;
  }

  async function refreshActivity() {
    try {
      activity = await api.mcpActivity();
    } catch (_) {
      return;
    }
    if (destroyed) return;
    const box = page.querySelector("#mcp-activity");
    if (box) renderActivity(box);
    const total = page.querySelector("#mcp-calls");
    if (total && info) {
      // 总调用数随记录增长；只在变化时更新，避免整页重绘
      try {
        const fresh = await api.mcpInfo();
        info.calls_total = fresh.calls_total;
        total.textContent = `累计 ${fresh.calls_total} 次调用`;
      } catch (_) {}
    }
  }

  async function copy(text, what) {
    const ok = await copyText(text);
    toast(ok ? `已复制${what}` : "复制失败，请手动选中复制", ok ? "success" : "error");
  }

  async function apply(enabled, mode) {
    try {
      info = await api.mcpSetSettings(enabled, mode);
    } catch (e) {
      toast(String(e), "error");
    }
    render();
  }

  async function pickMode(id) {
    if (id === info.mode) return;
    if (id === "full") {
      const ok = await confirmDialog(
        "「完全」档位允许 agent 结束任意端口上的进程、修改系统 hosts、增删改项目的启动命令、拉取代码和切换分支。\n\n只有在你信任所连接的 agent、并且会留意它的调用记录时才建议开启。确定切换吗？",
        { okText: "切换到完全", cancelText: "取消" },
      );
      if (!ok) return;
    }
    await apply(info.enabled, id);
  }

  async function resetToken() {
    const ok = await confirmDialog("重置后，所有已配置的 agent 都要换上新的 token 才能继续连接（走 stdio 转发器的不受影响）。\n\n确定重置吗？", { okText: "重置 token", cancelText: "取消" });
    if (!ok) return;
    info = await guard(api.mcpResetToken(), "已生成新的 token");
    revealToken = false;
    render();
  }

  // ---------- 渲染 ----------

  function connectionCard() {
    const url = info.url || "（服务启动中…）";
    return el("section", { class: "card mcp-card" }, [
      el("div", { class: "mcp-card-head" }, [
        el("h2", {}, "连接"),
        el("label", { class: "mcp-switch", title: "关闭后所有 agent 的调用都会被拒绝" }, [
          el("input", { type: "checkbox", ...(info.enabled ? { checked: "" } : {}), onchange: (e) => apply(e.target.checked, info.mode) }),
          el("span", {}, info.enabled ? "已开启" : "已关闭"),
        ]),
      ]),
      info.error ? el("p", { class: "mcp-error" }, `MCP 服务没能启动：${info.error}`) : null,
      el("p", { class: "dim" }, "DevBox 自带一个 MCP 服务，只监听本机（127.0.0.1）。支持 URL 连接的 agent 直接填下面的地址和 token 即可。"),
      el("div", { class: "mcp-kv" }, [
        el("span", { class: "mcp-k" }, "地址"),
        el("code", { class: "mcp-v" }, url),
        el("button", { class: "ghost-btn sm", disabled: info.url ? null : "", onclick: () => copy(info.url, "地址") }, "复制"),
      ]),
      el("div", { class: "mcp-kv" }, [
        el("span", { class: "mcp-k" }, "Token"),
        el("code", { class: "mcp-v" }, revealToken ? info.token : maskToken(info.token)),
        el("button", { class: "ghost-btn sm", onclick: () => { revealToken = !revealToken; render(); } }, revealToken ? "隐藏" : "显示"),
        el("button", { class: "ghost-btn sm", onclick: () => copy(info.token, " token") }, "复制"),
        el("button", { class: "ghost-btn sm", onclick: resetToken }, "重置"),
      ]),
      el("p", { class: "dim mcp-note" }, "token 保存在配置目录里（仅当前用户可读），重启 DevBox 后不变。别把它发给不信任的人或提交到代码仓库。"),
    ]);
  }

  function modeCard() {
    return el("section", { class: "card mcp-card" }, [
      el("h2", {}, "权限档位"),
      el("p", { class: "dim" }, "由 DevBox 在服务端强制执行：超出档位的工具不会出现在 agent 的工具列表里，直接调用也会被拒绝。随时可改，立即生效。"),
      el("div", { class: "mcp-modes" }, MODES.map((m) =>
        el("button", { class: `mcp-mode ${m.id}${info.mode === m.id ? " active" : ""}`, onclick: () => pickMode(m.id) }, [
          el("strong", {}, m.label),
          el("span", {}, m.hint),
        ]),
      )),
    ]);
  }

  function connectCard() {
    const shown = clientConfigs(info, { mask: !revealToken });
    const real = clientConfigs(info);
    if (shown.length === 0) return el("section", { class: "card mcp-card" }, [el("h2", {}, "接入方式"), el("p", { class: "dim" }, "服务还没有启动，稍后点右上角刷新。")]);
    const cur = shown.find((c) => c.id === activeClient) ?? shown[0];
    const curReal = real.find((c) => c.id === cur.id);
    return el("section", { class: "card mcp-card" }, [
      el("h2", {}, "接入方式"),
      el("div", { class: "mcp-tabs" }, shown.map((c) =>
        el("button", { class: `mcp-tab${c.id === cur.id ? " active" : ""}`, onclick: () => { activeClient = c.id; render(); } }, c.title),
      )),
      el("p", { class: "dim" }, cur.hint),
      el("pre", { class: "mcp-code" }, cur.text),
      el("div", { class: "mcp-actions" }, [
        el("button", { class: "run-btn sm", onclick: () => copy(curReal.text, "配置") }, "复制（含完整 token）"),
      ]),
    ]);
  }

  function toolsCard() {
    const groups = groupTools(info.tools);
    return el("section", { class: "card mcp-card" }, [
      el("h2", {}, `工具（当前档位可用 ${toolSummary(info.tools)}）`),
      el("p", { class: "dim" }, "agent 看到的是同样的列表。除了工具，还提供资源（项目日志、总览）和提示词（排查项目、体检等）。"),
      ...groups.map((g) =>
        el("details", { class: "mcp-group", ...(g.risk === "read" ? {} : { open: "" }) }, [
          el("summary", {}, [el("span", { class: `mcp-risk ${g.risk}` }, g.label), ` ${g.title}（${g.tools.length}）`]),
          el("div", { class: "mcp-tools" }, g.tools.map((t) =>
            el("div", { class: `mcp-tool${t.allowed ? "" : " off"}`, title: t.description }, [
              el("code", {}, t.name),
              el("span", {}, t.title),
              t.allowed ? null : el("em", {}, `需要「${RISK_LABEL[t.risk]}」档`),
            ]),
          )),
        ]),
      ),
    ]);
  }

  function activityCard() {
    const box = el("div", { id: "mcp-activity", class: "mcp-activity" });
    renderActivity(box);
    return el("section", { class: "card mcp-card" }, [
      el("div", { class: "mcp-card-head" }, [
        el("h2", {}, "最近的调用"),
        el("span", { id: "mcp-calls", class: "dim mcp-note" }, `累计 ${info.calls_total} 次调用`),
        el("button", { class: "ghost-btn sm", onclick: async () => { await api.mcpClearActivity().catch(() => {}); activity = []; renderActivity(page.querySelector("#mcp-activity")); } }, "清空记录"),
      ]),
      el("p", { class: "dim mcp-note" }, "agent 每次调用都会记在这里（被拒绝的也算），方便你知道它在做什么。"),
      box,
    ]);
  }

  function renderActivity(box) {
    if (!box) return;
    if (activity.length === 0) {
      box.replaceChildren(el("div", { class: "empty" }, "还没有调用。接入 agent 后，让它「看看 DevBox 里有哪些项目」试试。"));
      return;
    }
    box.replaceChildren(
      ...activity.slice(0, 60).map((a) =>
        el("div", { class: `mcp-act${a.ok ? "" : " bad"}`, title: a.error || "" }, [
          el("span", { class: "mcp-act-ts" }, a.ts),
          el("code", {}, a.tool),
          el("span", { class: "mcp-act-target" }, a.target || ""),
          el("span", { class: "mcp-act-ms" }, `${a.ms}ms`),
          el("span", { class: "mcp-act-client" }, a.client),
          a.ok ? null : el("span", { class: "mcp-act-err" }, a.error || "失败"),
        ]),
      ),
    );
  }

  function render() {
    if (!info) return;
    page.replaceChildren(
      el("div", { class: "view-header" }, [
        el("h1", {}, "MCP · 对 AI agent 开放"),
        el("span", { class: `mcp-pill ${info.enabled ? "on" : "off"}` }, info.enabled ? `运行中 · ${modeLabel(info.mode)}档` : "已关闭"),
      ]),
      connectionCard(),
      modeCard(),
      connectCard(),
      toolsCard(),
      activityCard(),
    );
  }

  (async () => {
    if (!(await loadInfo())) return;
    await refreshActivity().catch(() => {});
    if (destroyed) return;
    render();
    timer = setInterval(refreshActivity, 2500);
  })();

  return () => {
    destroyed = true;
    clearInterval(timer);
  };
}
