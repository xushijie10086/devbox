import { api } from "../api.js";
import { el, guard, toast } from "../ui.js";

export function mount(root) {
  let entries = [];

  const header = el("div", { class: "view-header" }, [
    el("h1", {}, "Hosts 管理"),
    el("div", { class: "header-tools" }, [
      el("button", { class: "ghost-btn", onclick: addRow }, "+ 新增记录"),
      el("button", { class: "primary-btn", onclick: save }, "保存到 /etc/hosts"),
    ]),
  ]);
  const note = el("div", { class: "note" },
    "DevBox 只维护 /etc/hosts 中它自己的托管区块（不会动其它条目）。保存时会弹出系统管理员授权。");
  const listBox = el("div", { class: "hosts-list" });
  const rawTitle = el("h3", { class: "sub" }, "当前 /etc/hosts 全文（只读）");
  const rawBox = el("pre", { class: "raw" });
  root.append(header, note, listBox, rawTitle, rawBox);

  async function refresh() {
    try {
      entries = await api.getHosts();
      rawBox.textContent = await api.getHostsRaw();
    } catch (e) {
      toast(String(e), "error");
    }
    render();
  }

  function render() {
    listBox.innerHTML = "";
    if (entries.length === 0) {
      listBox.append(el("div", { class: "empty" }, "托管区块暂无记录。点击「新增记录」添加本地域名映射。"));
      return;
    }
    entries.forEach((e, i) => {
      const enabled = el("input", { type: "checkbox", title: "启用/停用" });
      enabled.checked = e.enabled;
      enabled.onchange = () => { entries[i].enabled = enabled.checked; };

      const ip = el("input", { class: "mono", value: e.ip, placeholder: "127.0.0.1" });
      ip.oninput = () => { entries[i].ip = ip.value.trim(); };

      const hosts = el("input", { class: "grow", value: e.hostnames.join(" "), placeholder: "app.local api.local" });
      hosts.oninput = () => { entries[i].hostnames = hosts.value.split(/\s+/).filter(Boolean); };

      const comment = el("input", { class: "dim", value: e.comment || "", placeholder: "备注(可选)" });
      comment.oninput = () => { entries[i].comment = comment.value; };

      const del = el("button", { class: "icon-btn", title: "删除", onclick: () => { entries.splice(i, 1); render(); } }, "🗑️");

      listBox.append(el("div", { class: `host-row ${e.enabled ? "" : "disabled"}` }, [enabled, ip, hosts, comment, del]));
    });
  }

  function addRow() {
    entries.push({ ip: "127.0.0.1", hostnames: [""], enabled: true, comment: null });
    render();
  }

  async function save() {
    const cleaned = entries
      .map((e) => ({ ...e, hostnames: e.hostnames.filter(Boolean), comment: e.comment || null }))
      .filter((e) => e.ip && e.hostnames.length > 0);
    await guard(api.saveHosts(cleaned), "已写入 /etc/hosts 并刷新 DNS");
    refresh();
  }

  refresh();
  return () => {};
}
