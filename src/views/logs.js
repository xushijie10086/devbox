import { api } from "../api.js";
import { el, guard } from "../ui.js";
import { createLogView } from "../logview.js";

// 「日志」页：选一个项目，查看它的实时日志（与项目行的日志弹窗共用同一个查看器）
export function mount(root) {
  let current = null;

  const select = el("select", { class: "log-select", onchange: (e) => switchTo(e.target.value) });
  const header = el("div", { class: "view-header" }, [
    el("h1", {}, "日志"),
    el("div", { class: "header-tools" }, [select]),
  ]);
  const view = createLogView({
    fetchChunk: (after, epoch) => api.getLogs(current, after, epoch),
    onClear: () => api.clearLogs(current),
    onRevealFile: async () => guard(api.revealInFinder(await api.logFilePath(current))),
  });
  const page = el("div", { class: "logs-page" }, [view.root]);
  root.append(header, page);

  function switchTo(id) {
    current = id;
    view.setSource((after, epoch) => api.getLogs(id, after, epoch));
  }

  async function init() {
    const projects = await api.listProjects();
    select.innerHTML = "";
    if (projects.length === 0) {
      select.append(el("option", {}, "（暂无项目）"));
      page.replaceChildren(el("div", { class: "empty" }, "还没有项目，无法查看日志。"));
      return;
    }
    for (const p of projects) select.append(el("option", { value: p.id }, p.name));
    current = projects[0].id;
    view.start();
  }

  init();
  return () => view.stop();
}
