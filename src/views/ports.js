import { api } from "../api.js";
import { el, guard, toast, confirmDialog } from "../ui.js";

export function mount(root) {
  let ports = [];
  let groups = []; // 项目组，顺序与项目库的 tab 一致
  let activeTab = "__all__"; // "__all__" | "__none__" | 组名
  let filter = "";
  let timer = null;

  const search = el("input", { class: "search", placeholder: "过滤 端口 / 项目 / 组 / 进程…", oninput: (e) => { filter = e.target.value.toLowerCase(); render(); } });
  const header = el("div", { class: "view-header" }, [
    el("h1", {}, "项目端口"),
    el("div", { class: "header-tools" }, [search, el("button", { class: "ghost-btn", onclick: refresh }, "↻ 刷新")]),
  ]);
  const tabsEl = el("div", { class: "tabs" });
  const table = el("div", { class: "table" });
  root.append(header, el("div", { class: "tabs-bar port-tabs" }, [tabsEl]), table);

  const TAB_ALL = "__all__";
  const TAB_NONE = "__none__";

  async function refresh() {
    try {
      [ports, groups] = await Promise.all([api.listProjectPorts(), api.listProjectGroups()]); // 端口只含与已登记项目相关的
    } catch (e) {
      toast(String(e), "error");
    }
    render();
  }

  const inTab = (p, tab) => (tab === TAB_ALL ? true : tab === TAB_NONE ? !p.group : p.group === tab);

  function renderTabs() {
    const hasNone = ports.some((p) => !p.group);
    // 当前 tab 对应的分组已被删（或未分组已空）时回到「全部」
    if (activeTab !== TAB_ALL && !(activeTab === TAB_NONE && hasNone) && !groups.includes(activeTab)) activeTab = TAB_ALL;
    tabsEl.innerHTML = "";
    const add = (id, label) =>
      tabsEl.append(el("button", { class: `tab${id === activeTab ? " active" : ""}`, "data-tab": id, onclick: () => { activeTab = id; render(); } }, [
        el("span", {}, label),
        el("span", { class: "tab-count" }, String(ports.filter((p) => inTab(p, id)).length)),
      ]));
    add(TAB_ALL, "全部");
    for (const g of groups) add(g, g);
    if (hasNone && groups.length > 0) add(TAB_NONE, "未分组");
  }

  function render() {
    renderTabs();
    // 同组的排在一起：按项目库里项目组的顺序，未分组放最后，组内按端口号
    const order = (p) => (p.group && groups.includes(p.group) ? groups.indexOf(p.group) : groups.length);
    const rows = ports
      .filter((p) => inTab(p, activeTab))
      .filter((p) =>
        !filter ||
        String(p.port).includes(filter) ||
        (p.project || "").toLowerCase().includes(filter) ||
        (p.group || "").toLowerCase().includes(filter) ||
        p.process.toLowerCase().includes(filter) ||
        p.address.toLowerCase().includes(filter)
      )
      .sort((a, b) => order(a) - order(b) || a.port - b.port);
    table.innerHTML = "";
    table.append(el("div", { class: "trow ports thead" }, [
      el("span", {}, "端口"), el("span", {}, "所属项目"), el("span", {}, "项目组"), el("span", {}, "进程"), el("span", {}, "PID"),
      el("span", {}, "地址"), el("span", {}, "协议"), el("span", {}, ""),
    ]));
    if (rows.length === 0) {
      table.append(el("div", { class: "empty" }, ports.length === 0
        ? "没有与项目相关的监听端口。启动项目，或为项目填写端口后会显示在这里。"
        : !filter && activeTab !== TAB_ALL
          ? `${activeTab === TAB_NONE ? "未分组" : `项目组「${activeTab}」`}下的项目目前没有监听端口。`
          : "没有匹配的监听端口。"));
      return;
    }
    for (const p of rows) {
      table.append(el("div", { class: "trow ports" }, [
        el("span", { class: "mono strong" }, String(p.port)),
        el("span", { class: "strong" }, p.project),
        el("span", { class: p.group ? "" : "dim" }, p.group || "未分组"),
        el("span", {}, p.process),
        el("span", { class: "mono" }, String(p.pid)),
        el("span", { class: "mono dim" }, p.address),
        el("span", { class: "dim" }, p.protocol),
        el("span", {}, el("button", { class: "danger-btn sm", onclick: () => kill(p) }, "结束")),
      ]));
    }
  }

  async function kill(p) {
    if (!(await confirmDialog(`结束进程 ${p.process} (PID ${p.pid})，释放端口 ${p.port}？`))) return;
    await guard(api.killProcess(p.pid, p.port), `已释放端口 ${p.port}`);
    setTimeout(refresh, 400);
  }

  refresh();
  timer = setInterval(refresh, 4000);
  return () => clearInterval(timer);
}
