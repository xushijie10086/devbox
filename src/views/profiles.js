import { api } from "../api.js";
import { el, guard, toast } from "../ui.js";
import { showModal } from "./projects.js";

export function mount(root) {
  let profiles = [];
  let projects = [];
  let services = [];

  const header = el("div", { class: "view-header" }, [
    el("h1", {}, "启动组"),
    el("button", { class: "primary-btn", onclick: () => openEditor() }, "+ 新增启动组"),
  ]);
  const note = el("div", { class: "note" }, "把常用的项目 + brew 服务打包，一键拉起或关闭一整套开发环境。");
  const list = el("div", { class: "card-grid" });
  root.append(header, note, list);

  async function refresh() {
    [profiles, projects, services] = await Promise.all([
      api.listProfiles(),
      api.listProjects(),
      api.listServices().catch(() => []),
    ]);
    render();
  }

  function render() {
    list.innerHTML = "";
    if (profiles.length === 0) {
      list.append(el("div", { class: "empty" }, "还没有启动组。"));
      return;
    }
    const nameOf = (id) => projects.find((p) => p.id === id)?.name || "(已删除)";
    for (const pf of profiles) {
      const items = [
        ...pf.project_ids.map((id) => el("span", { class: "chip" }, nameOf(id))),
        ...pf.service_names.map((s) => el("span", { class: "chip chip-svc" }, s)),
      ];
      list.append(el("div", { class: "card" }, [
        el("div", { class: "card-top" }, [el("span", { class: "card-title" }, pf.name)]),
        el("div", { class: "chips" }, items.length ? items : [el("span", { class: "dim" }, "（空）")]),
        el("div", { class: "card-actions" }, [
          el("button", { class: "run-btn", onclick: () => runProfile(pf, true) }, "▶ 全部启动"),
          el("button", { class: "danger-btn", onclick: () => runProfile(pf, false) }, "■ 全部停止"),
          el("button", { class: "icon-btn", title: "编辑", onclick: () => openEditor(pf) }, "✎"),
          el("button", { class: "icon-btn", title: "删除", onclick: () => remove(pf) }, "🗑️"),
        ]),
      ]));
    }
  }

  async function runProfile(pf, start) {
    const errs = await guard(start ? api.startProfile(pf.id) : api.stopProfile(pf.id));
    if (errs && errs.length) toast(errs.join("；"), "error");
    else toast(start ? "已全部启动" : "已全部停止", "success");
  }

  async function remove(pf) {
    if (!confirm(`删除启动组「${pf.name}」？`)) return;
    await guard(api.deleteProfile(pf.id), "已删除");
    refresh();
  }

  function openEditor(pf) {
    const data = pf || { id: "", name: "", project_ids: [], service_names: [] };

    const nameInput = el("input", { name: "name", value: data.name, placeholder: "全栈开发环境" });

    const projBox = el("div", { class: "check-list" }, projects.map((p) => {
      const c = el("input", { type: "checkbox" });
      if (data.project_ids.includes(p.id)) c.checked = true;
      c.dataset.id = p.id;
      return el("label", { class: "check-item" }, [c, el("span", {}, p.name)]);
    }));

    const svcBox = el("div", { class: "check-list" }, services.map((s) => {
      const c = el("input", { type: "checkbox" });
      if (data.service_names.includes(s.name)) c.checked = true;
      c.dataset.name = s.name;
      return el("label", { class: "check-item" }, [c, el("span", {}, s.name)]);
    }));

    const form = el("div", { class: "form" }, [
      el("label", { class: "form-row" }, [el("span", {}, "名称"), nameInput]),
      el("div", { class: "form-row" }, [el("span", {}, "包含项目"), projBox]),
      el("div", { class: "form-row" }, [el("span", {}, "包含服务"), svcBox]),
    ]);

    showModal(pf ? "编辑启动组" : "新增启动组", form, async () => {
      const profile = {
        id: data.id || "",
        name: nameInput.value.trim(),
        project_ids: [...projBox.querySelectorAll("input:checked")].map((c) => c.dataset.id),
        service_names: [...svcBox.querySelectorAll("input:checked")].map((c) => c.dataset.name),
      };
      if (!profile.name) { toast("请填写名称", "error"); return false; }
      await guard(api.saveProfile(profile), "已保存");
      refresh();
      return true;
    });
  }

  refresh();
  return () => {};
}
