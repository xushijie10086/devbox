import { api } from "../api.js";
import { el, guard, toast, confirmDialog } from "../ui.js";

export function mount(root) {
  let ports = [];
  let filter = "";
  let timer = null;

  const search = el("input", { class: "search", placeholder: "过滤 端口 / 项目 / 进程 / 地址…", oninput: (e) => { filter = e.target.value.toLowerCase(); render(); } });
  const header = el("div", { class: "view-header" }, [
    el("h1", {}, "项目端口"),
    el("div", { class: "header-tools" }, [search, el("button", { class: "ghost-btn", onclick: refresh }, "↻ 刷新")]),
  ]);
  const table = el("div", { class: "table" });
  root.append(header, table);

  async function refresh() {
    try {
      ports = await api.listProjectPorts(); // 只含与已登记项目相关的端口
    } catch (e) {
      toast(String(e), "error");
    }
    render();
  }

  function render() {
    const rows = ports.filter((p) =>
      !filter ||
      String(p.port).includes(filter) ||
      (p.project || "").toLowerCase().includes(filter) ||
      p.process.toLowerCase().includes(filter) ||
      p.address.toLowerCase().includes(filter)
    );
    table.innerHTML = "";
    table.append(el("div", { class: "trow thead" }, [
      el("span", {}, "端口"), el("span", {}, "所属项目"), el("span", {}, "进程"), el("span", {}, "PID"),
      el("span", {}, "地址"), el("span", {}, "协议"), el("span", {}, ""),
    ]));
    if (rows.length === 0) {
      table.append(el("div", { class: "empty" }, ports.length === 0 ? "没有与项目相关的监听端口。启动项目，或为项目填写端口后会显示在这里。" : "没有匹配的监听端口。"));
      return;
    }
    for (const p of rows) {
      table.append(el("div", { class: "trow" }, [
        el("span", { class: "mono strong" }, String(p.port)),
        el("span", { class: "strong" }, p.project),
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
