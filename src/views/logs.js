import { api } from "../api.js";
import { el, guard, toast } from "../ui.js";

export function mount(root) {
  let projects = [];
  let current = null;
  let timer = null;
  let autoscroll = true;

  const select = el("select", { class: "log-select", onchange: (e) => { current = e.target.value; pull(true); } });
  const clearBtn = el("button", { class: "ghost-btn", onclick: clear }, "清空");
  const scrollToggle = el("label", { class: "scroll-toggle" }, [
    (() => { const c = el("input", { type: "checkbox" }); c.checked = true; c.onchange = (e) => { autoscroll = e.target.checked; }; return c; })(),
    el("span", {}, "自动滚动"),
  ]);
  const header = el("div", { class: "view-header" }, [
    el("h1", {}, "日志"),
    el("div", { class: "header-tools" }, [select, scrollToggle, clearBtn]),
  ]);
  const term = el("div", { class: "terminal" });
  root.append(header, term);

  async function init() {
    projects = await api.listProjects();
    select.innerHTML = "";
    if (projects.length === 0) {
      select.append(el("option", {}, "（暂无项目）"));
      term.append(el("div", { class: "empty" }, "还没有项目，无法查看日志。"));
      return;
    }
    for (const p of projects) select.append(el("option", { value: p.id }, p.name));
    current = projects[0].id;
    pull(true);
  }

  async function pull(scroll) {
    if (!current) return;
    try {
      const lines = await api.getLogs(current);
      renderLines(lines, scroll);
    } catch (_) {}
  }

  function renderLines(lines, forceScroll) {
    term.innerHTML = "";
    if (lines.length === 0) {
      term.append(el("div", { class: "empty" }, "暂无输出。项目运行后这里会实时显示 stdout / stderr。"));
      return;
    }
    for (const l of lines) {
      term.append(el("div", { class: `log-line ${l.stream}` }, [
        el("span", { class: "log-ts" }, l.ts),
        el("span", { class: "log-text" }, l.text),
      ]));
    }
    if (autoscroll || forceScroll) term.scrollTop = term.scrollHeight;
  }

  async function clear() {
    if (!current) return;
    await guard(api.clearLogs(current));
    pull(true);
  }

  init();
  timer = setInterval(() => pull(false), 1000);
  return () => clearInterval(timer);
}
