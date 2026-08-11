import { api } from "../api.js";
import { el, guard, toast } from "../ui.js";

export function mount(root) {
  let services = [];
  let timer = null;

  const header = el("div", { class: "view-header" }, [
    el("h1", {}, "服务 (Homebrew)"),
    el("button", { class: "ghost-btn", onclick: refresh }, "↻ 刷新"),
  ]);
  const table = el("div", { class: "table" });
  root.append(header, table);

  async function refresh() {
    try {
      services = await api.listServices();
    } catch (e) {
      services = [];
      table.innerHTML = "";
      table.append(el("div", { class: "empty" }, `无法读取 brew 服务：${e}`));
      return;
    }
    render();
  }

  function render() {
    table.innerHTML = "";
    table.append(el("div", { class: "trow thead services" }, [
      el("span", {}, "服务"), el("span", {}, "状态"), el("span", {}, "用户"), el("span", {}, "操作"),
    ]));
    if (services.length === 0) {
      table.append(el("div", { class: "empty" }, "没有检测到 brew 服务。"));
      return;
    }
    for (const s of services) {
      const running = s.status === "started";
      table.append(el("div", { class: "trow services" }, [
        el("span", { class: "strong" }, [el("span", { class: `dot ${running ? "on" : "off"}` }), s.name]),
        el("span", { class: `svc-status ${s.status}` }, statusLabel(s.status)),
        el("span", { class: "dim" }, s.user || "—"),
        el("span", { class: "svc-actions" }, [
          running
            ? el("button", { class: "danger-btn sm", onclick: () => act(api.stopService(s.name), `已停止 ${s.name}`) }, "停止")
            : el("button", { class: "run-btn sm", onclick: () => act(api.startService(s.name), `已启动 ${s.name}`) }, "启动"),
          el("button", { class: "ghost-btn sm", onclick: () => act(api.restartService(s.name), `已重启 ${s.name}`) }, "重启"),
        ]),
      ]));
    }
  }

  async function act(promise, okMsg) {
    await guard(promise, okMsg);
    setTimeout(refresh, 600);
  }

  refresh();
  timer = setInterval(refresh, 6000);
  return () => clearInterval(timer);
}

function statusLabel(s) {
  return { started: "运行中", stopped: "已停止", none: "未启动", error: "错误" }[s] || s;
}
