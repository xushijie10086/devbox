import { api } from "../api.js";
import { el, toast, guard, fmtUptime, confirmDialog } from "../ui.js";
import { icon } from "../icons.js";

// 固定 tab 的内部标识；真实项目组用它自己的名字，保留名在后端已禁止使用
const TAB_ALL = "__all__";
const TAB_NONE = "__none__";

// 记住上次停留的 tab（视图重新挂载、点「刷新」后仍停在原处）
let activeTab = TAB_ALL;

// 生成一个带 SVG 图标的操作按钮
function iconBtn(name, title, onclick, extraClass = "") {
  const btn = el("button", { class: `icon-btn ${extraClass}`.trim(), title, onclick });
  btn.append(icon(name, 17));
  return btn;
}

export function mount(root) {
  let projects = [];
  let groups = [];
  let statuses = {};
  let timer = null;
  let drag = null; // 拖拽上下文，见 startDrag
  const starting = new Set(); // 正在等待启动结果的项目 id
  const pulling = new Set(); // 正在拉取代码的项目 id

  const tabsEl = el("div", { class: "tabs" });
  const groupTools = el("div", { class: "group-tools" });
  const header = el("div", { class: "sticky-header" }, [
    el("div", { class: "view-header" }, [
      el("h1", {}, "项目库"),
      el("button", { class: "primary-btn", onclick: () => openEditor() }, "+ 新增项目"),
    ]),
    el("div", { class: "tabs-bar" }, [tabsEl, groupTools]),
  ]);
  const thead = el("div", { class: "prow prow-head" }, [
    el("span", {}), el("span", {}, "项目"), el("span", {}, "启动命令"),
    el("span", {}, "端口"), el("span", { class: "meta-cell" }, "运行"), el("span", { class: "head-actions" }, "操作"),
  ]);
  const list = el("div", { class: "plist" });
  const table = el("div", { class: "table ptable" }, [thead, list]);
  root.append(header, table);

  // ---------- 分组 / tab ----------

  const ungrouped = () => projects.filter((p) => !p.group);
  const inGroup = (g) => projects.filter((p) => p.group === g);

  /** 当前 tab 下要显示的项目（保持全局顺序） */
  function visibleProjects() {
    if (activeTab === TAB_ALL) return projects;
    if (activeTab === TAB_NONE) return ungrouped();
    return inGroup(activeTab);
  }

  /** 当前 tab 的名称，用于提示语 */
  const tabLabel = () => (activeTab === TAB_ALL ? "全部项目" : activeTab === TAB_NONE ? "未分组" : activeTab);

  function renderTabs() {
    // 当前 tab 已不存在（分组被删 / 未分组已空）时回到「全部」
    const exists =
      activeTab === TAB_ALL ||
      (activeTab === TAB_NONE && ungrouped().length > 0) ||
      groups.includes(activeTab);
    if (!exists) activeTab = TAB_ALL;

    tabsEl.innerHTML = "";
    const addTab = (id, label, count) => {
      const t = el("button", { class: `tab${id === activeTab ? " active" : ""}`, onclick: () => selectTab(id) }, [
        el("span", {}, label),
        el("span", { class: "tab-count" }, String(count)),
      ]);
      tabsEl.append(t);
    };
    addTab(TAB_ALL, "全部", projects.length);
    for (const g of groups) addTab(g, g, inGroup(g).length);
    if (ungrouped().length > 0 && groups.length > 0) addTab(TAB_NONE, "未分组", ungrouped().length);
    tabsEl.append(el("button", { class: "tab tab-add", title: "新增项目组", onclick: () => openGroupDialog() }, "＋"));

    renderGroupTools();
  }

  function selectTab(id) {
    if (drag) return;
    activeTab = id;
    renderTabs();
    render();
  }

  /** 右侧：当前 tab 的批量操作 + 分组管理 */
  function renderGroupTools() {
    groupTools.innerHTML = "";
    const rows = visibleProjects();
    const runningN = rows.filter((p) => statuses[p.id]?.running).length;
    const idle = rows.filter((p) => !statuses[p.id]?.running && !starting.has(p.id));
    const scope = activeTab === TAB_ALL ? "全部" : "本组";

    const startAll = el("button", {
      class: "run-btn sm", disabled: idle.length === 0 ? "disabled" : false,
      title: idle.length ? `启动${tabLabel()}里未运行的 ${idle.length} 个项目` : "没有可启动的项目",
      onclick: () => startMany(idle),
    }, `▶ ${scope}启动`);
    const stopAll = el("button", {
      class: "danger-btn sm", disabled: runningN === 0 ? "disabled" : false,
      title: runningN ? `停止${tabLabel()}里运行中的 ${runningN} 个项目` : "没有运行中的项目",
      onclick: () => stopMany(rows.filter((p) => statuses[p.id]?.running)),
    }, `■ ${scope}停止`);
    groupTools.append(startAll, stopAll);

    if (groups.includes(activeTab)) {
      groupTools.append(
        el("span", { class: "tools-sep" }),
        iconBtn("edit", "重命名分组", () => openGroupDialog(activeTab)),
        iconBtn("trash", "删除分组（项目保留，回到未分组）", () => removeGroup(activeTab), "danger"),
      );
    }
  }

  /** 新增 / 重命名项目组 */
  function openGroupDialog(oldName) {
    const input = el("input", { type: "text", placeholder: "例如：后端服务", value: oldName ?? "", maxlength: "30" });
    const form = el("div", { class: "form" }, [el("label", { class: "form-row" }, [el("span", {}, "分组名称"), input])]);
    showModal(oldName ? "重命名分组" : "新增项目组", form, async () => {
      const name = input.value.trim();
      if (!name) {
        toast("分组名称不能为空", "error");
        return false;
      }
      try {
        const saved = oldName
          ? await api.renameProjectGroup(oldName, name)
          : await api.addProjectGroup(name);
        activeTab = saved; // 新建 / 改名后直接切到该分组
        toast(oldName ? "已重命名" : "已创建分组", "success");
      } catch (e) {
        toast(String(e), "error");
        return false;
      }
      await refresh();
      return true;
    });
    setTimeout(() => input.focus(), 0);
  }

  async function removeGroup(name) {
    const n = inGroup(name).length;
    const tip = n ? `删除分组「${name}」？其中 ${n} 个项目不会被删除，会回到「未分组」。` : `删除空分组「${name}」？`;
    if (!(await confirmDialog(tip))) return;
    try {
      await api.deleteProjectGroup(name);
      toast("已删除分组", "success");
    } catch (e) {
      toast(String(e), "error");
    }
    await refresh();
  }

  // ---------- 批量操作 ----------

  /** 汇总一批项目的结果，只在一条提示里说清：几个成功、哪些失败、为什么 */
  async function startMany(items) {
    if (items.length === 0) return;
    items.forEach((p) => starting.add(p.id));
    render();
    renderGroupTools();
    const results = await Promise.allSettled(items.map((p) => api.startProject(p.id)));
    items.forEach((p) => starting.delete(p.id));

    const failed = [];
    let warned = 0;
    results.forEach((r, i) => {
      if (r.status === "rejected") failed.push(`${items[i].name}：${String(r.reason).split("\n")[0]}`);
      else if (r.value.level === "warning") warned++;
    });
    const ok = items.length - failed.length;
    if (failed.length === 0) {
      toast(warned ? `已启动 ${ok} 个项目，其中 ${warned} 个需留意（见各项目提示）` : `已启动 ${ok} 个项目`, warned ? "warning" : "success");
    } else {
      toast(`成功 ${ok} 个，失败 ${failed.length} 个：\n${failed.join("\n")}`, "error");
    }
    await refresh();
  }

  async function stopMany(items) {
    if (items.length === 0) return;
    if (!(await confirmDialog(`停止${tabLabel()}里 ${items.length} 个运行中的项目？`))) return;
    const results = await Promise.allSettled(items.map((p) => api.stopProject(p.id)));
    const failed = results.filter((r) => r.status === "rejected").length;
    toast(failed ? `已停止 ${items.length - failed} 个，${failed} 个失败` : `已停止 ${items.length} 个项目`, failed ? "error" : "success");
    await refresh();
  }

  // ---------- 拖拽排序 ----------

  // 用 Pointer 事件手写拖拽：macOS WKWebView 对 HTML5 drag-and-drop 支持不完整，
  // 拖拽途中移动源节点会被忽略，导致「能拖但插不进去」。
  function startDrag(row, e) {
    if (e.button !== 0 || drag || row.parentNode !== list) return;
    e.preventDefault();

    const rect = row.getBoundingClientRect();
    const placeholder = el("div", { class: "row-placeholder" });
    placeholder.style.height = `${rect.height}px`;

    drag = { row, placeholder, dx: e.clientX - rect.left, dy: e.clientY - rect.top };

    // 占位块留在原位撑住列表，行移到 body 上用 fixed 跟随指针
    list.insertBefore(placeholder, row);
    row.style.width = `${rect.width}px`;
    row.style.height = `${rect.height}px`;
    row.style.left = `${rect.left}px`;
    row.style.top = `${rect.top}px`;
    row.classList.add("dragging");
    document.body.append(row);
    document.body.classList.add("dragging-active");

    document.addEventListener("pointermove", onMove);
    document.addEventListener("pointerup", endDrag);
    document.addEventListener("pointercancel", endDrag);
  }

  // 行空白处按下后拖动超过阈值才进入拖拽，避免点击、选中文本被误判；
  // 按钮、输入框与命令列保持原有交互，不触发拖拽
  function armDrag(row, down) {
    if (down.button !== 0 || drag || down.target.closest("button, input, textarea, select, a, .cmd-cell")) return;
    const disarm = () => {
      document.removeEventListener("pointermove", probe);
      document.removeEventListener("pointerup", disarm);
      document.removeEventListener("pointercancel", disarm);
    };
    const probe = (e) => {
      if (Math.hypot(e.clientX - down.clientX, e.clientY - down.clientY) < 5) return;
      disarm();
      startDrag(row, down);
    };
    document.addEventListener("pointermove", probe);
    document.addEventListener("pointerup", disarm);
    document.addEventListener("pointercancel", disarm);
  }

  function onMove(e) {
    if (!drag) return;
    const { row, placeholder } = drag;
    row.style.left = `${e.clientX - drag.dx}px`;
    row.style.top = `${e.clientY - drag.dy}px`;

    const after = rowAfterPoint(e.clientY);
    if (after) {
      if (after !== placeholder.nextElementSibling) list.insertBefore(placeholder, after);
    } else if (list.lastElementChild !== placeholder) {
      list.append(placeholder);
    }
    autoScroll(e.clientY);
  }

  async function endDrag() {
    if (!drag) return;
    const { row, placeholder } = drag;
    drag = null;
    document.removeEventListener("pointermove", onMove);
    document.removeEventListener("pointerup", endDrag);
    document.removeEventListener("pointercancel", endDrag);
    document.body.classList.remove("dragging-active");

    // 行落回占位块的位置
    row.classList.remove("dragging");
    row.removeAttribute("style");
    list.insertBefore(row, placeholder);
    placeholder.remove();

    await commitOrder();
    render(); // 补上拖拽期间被跳过的状态刷新
  }

  /** 拖到视口上下边缘时滚动内容区；上沿要避开固定在顶部的标题 + tab 栏 */
  function autoScroll(y) {
    const scroller = root.closest(".content") || root.parentElement;
    if (!scroller) return;
    const r = scroller.getBoundingClientRect();
    const top = r.top + header.offsetHeight;
    if (y < top + 40) scroller.scrollTop -= 12;
    else if (y > r.bottom - 60) scroller.scrollTop += 12;
  }

  /** 找出应排在指针位置之后的那一行：指针在某行上半部分之上即插到它前面 */
  function rowAfterPoint(y) {
    for (const r of list.querySelectorAll(".prow")) {
      const b = r.getBoundingClientRect();
      if (y < b.top + b.height / 2) return r;
    }
    return null;
  }

  // 把当前 DOM 顺序同步回内存并持久化。
  // 列表可能只是某个 tab 的子集：让这些项目依次回填它们原来占的位置，其余项目纹丝不动。
  async function commitOrder() {
    const ids = [...list.querySelectorAll(".prow")].map((r) => r.dataset.id);
    const byId = new Map(projects.map((p) => [p.id, p]));
    const visible = new Set(ids);
    const queue = ids.map((id) => byId.get(id));
    if (queue.some((p) => !p)) return; // 顺序异常时放弃，等下次刷新纠正
    const next = projects.map((p) => (visible.has(p.id) ? queue.shift() : p));
    if (next.every((p, i) => p.id === projects[i].id)) return; // 顺序没变
    projects = next;
    try {
      await api.reorderProjects(next.map((p) => p.id));
    } catch (e) {
      toast(String(e), "error");
      await refresh();
    }
  }

  // ---------- 数据与渲染 ----------

  async function refresh() {
    [projects, groups] = await Promise.all([api.listProjects(), api.listProjectGroups()]);
    try {
      const arr = await api.projectStatuses();
      statuses = Object.fromEntries(arr.map((s) => [s.id, s]));
    } catch (e) {
      statuses = {};
    }
    renderTabs();
    render();
  }

  function render() {
    if (drag) return; // 拖拽过程中不重建 DOM，避免打断
    list.innerHTML = "";
    const rows = visibleProjects();
    thead.style.display = rows.length ? "" : "none";
    if (rows.length === 0) {
      list.append(el("div", { class: "empty" }, projects.length === 0
        ? "还没有项目。点击右上角「新增项目」注册你的第一个项目。"
        : "这个分组下还没有项目。点右上角「新增项目」，或编辑已有项目并把它移到这里。"));
      return;
    }
    for (const p of rows) {
      list.append(projectRow(p, statuses[p.id]));
    }
  }

  function projectRow(p, st) {
    const running = st?.running;
    const dot = el("span", { class: `dot ${running ? "on" : "off"}` });
    const kindBadge = el("span", { class: `badge badge-${p.kind}` }, kindLabel(p.kind));

    // 「全部」tab 里额外标出所属分组，方便辨认
    const groupBadge = activeTab === TAB_ALL && p.group ? el("span", { class: "badge badge-group", title: "所属项目组" }, `# ${p.group}`) : null;

    const nameCell = el("div", { class: "name-cell" }, [
      el("div", { class: "name-line" }, [dot, el("span", { class: "row-title", title: p.name }, p.name), kindBadge]),
      el("div", { class: "sub-line" }, [groupBadge, ...runtimeChips(p), el("span", { class: "row-path", title: p.path }, p.path)]),
    ]);

    const cmdCell = el("div", { class: "cmd-cell", title: p.start_command }, [
      el("span", { class: "cmd-prompt" }, "$"),
      el("code", {}, p.start_command),
    ]);

    const portCell = el("div", { class: "port-cell" },
      p.port != null ? `:${p.port} ${st?.port_up ? "🟢" : "⚪"}` : "—");

    const metaCell = el("div", { class: "meta-cell" }, running
      ? [
          el("span", { title: `PID ${st.pid}` }, `PID ${st.pid} · ${fmtUptime(st.uptime_secs)}`),
          el("span", {}, `CPU ${(st.cpu ?? 0).toFixed(0)}% · ${st.memory_mb ?? 0}MB`),
        ]
      : el("span", { class: "dim" }, "未运行"));

    const runBtns = starting.has(p.id)
      ? [el("button", { class: "run-btn sm", disabled: "disabled" }, "⏳ 启动中…")]
      : running
      ? [
          el("button", { class: "danger-btn sm", onclick: () => act(api.stopProject(p.id), "已停止") }, "■ 停止"),
          el("button", { class: "ghost-btn sm", onclick: () => launch(p, api.restartProject) }, "↻ 重启"),
        ]
      : [el("button", { class: "run-btn sm", onclick: () => launch(p, api.startProject) }, "▶ 启动")];

    // 操作列：生命周期按钮 + 快捷入口 + 编辑 / 删除
    const actions = el("div", { class: "actions-cell" }, [
      el("div", { class: "run-group" }, runBtns),
      el("div", { class: "quick-group" }, [
        iconBtn("code", "在编辑器打开", () => guard(api.openInEditor(p.path, p.editor))),
        p.url && iconBtn("browser", "在浏览器打开", () => guard(api.openUrl(p.url))),
        iconBtn("terminal", "在终端打开", () => guard(api.openTerminal(p.path))),
        iconBtn("folder", "在访达显示", () => guard(api.revealInFinder(p.path))),
        iconBtn("pull", pulling.has(p.id) ? "正在拉取代码…" : "拉取最新代码（git pull）", () => pull(p), pulling.has(p.id) ? "busy" : ""),
        iconBtn("logsView", "查看日志", () => openLogModal(p)), // 进程退出后日志仍保留，便于排查启动失败
        iconBtn("edit", "编辑", () => openEditor(p)),
        iconBtn("trash", "删除", () => removeProject(p), "danger"),
      ]),
    ]);

    // 拖拽手柄：按下即拖；行上其它空白处也可拖，见 armDrag
    const handle = el("span", { class: "drag-handle", title: "拖动调整排序" });
    handle.append(icon("grip", 16));

    const row = el("div", { class: "prow", "data-id": p.id }, [handle, nameCell, cmdCell, portCell, metaCell, actions]);

    handle.addEventListener("pointerdown", (e) => startDrag(row, e));
    row.addEventListener("pointerdown", (e) => {
      if (!handle.contains(e.target)) armDrag(row, e);
    });

    return row;
  }

  // 一键拉取代码：git pull --ff-only，结果（已是最新 / 更新了几个提交 / 失败原因）用提示告知
  async function pull(p) {
    if (pulling.has(p.id)) return;
    pulling.add(p.id);
    render();
    toast(`${p.name}：正在拉取代码…`, "info");
    try {
      const out = await api.gitPull(p.id);
      toast(`${p.name}：${out.message}`, out.level === "warning" ? "warning" : "success");
    } catch (e) {
      toast(`${p.name}：${String(e)}`, "error");
    } finally {
      pulling.delete(p.id);
      render();
    }
  }

  // 运行时版本标签：前端项目显示 Node、后端项目显示 JDK；「其它」类型只在设置过时显示。点击可切换。
  function runtimeChips(p) {
    const kinds = [];
    if (p.kind === "frontend" || p.node) kinds.push("node");
    if (p.kind === "backend" || p.java) kinds.push("java");
    return kinds.map((k) => {
      const cur = p[k];
      const label = k === "node" ? "Node" : "JDK";
      return el("button", {
        class: `rt-chip${cur ? " set" : ""}`,
        title: cur ? `${label} ${cur.version}（点击切换）` : `使用系统默认 ${label}（点击切换版本）`,
        onclick: () => openRuntimeDialog(p, k),
      }, cur ? `${label} ${cur.version}` : `${label} 默认`);
    });
  }

  /** 构造某种运行时的版本下拉；已选版本若本机检测不到（被卸载）也保留并标出 */
  function runtimeSelect(kind, list, current) {
    const label = kind === "node" ? "Node" : "JDK";
    const options = [el("option", { value: "" }, `系统默认（不指定）`)];
    const known = new Set();
    for (const r of list) {
      known.add(r.path);
      options.push(el("option", {
        value: r.path,
        selected: current?.path === r.path ? "selected" : false,
      }, `${label} ${r.version} · ${r.source}`));
    }
    if (current && !known.has(current.path)) {
      options.push(el("option", { value: current.path, selected: "selected" }, `⚠ ${label} ${current.version}（本机未检测到，可能已卸载）`));
    }
    const sel = el("select", {}, options);
    const emptyHint = list.length === 0 ? `没有检测到本机安装的 ${label}（支持 ${kind === "node" ? "nvm / fnm / Volta / asdf / Homebrew" : "系统 JVM / SDKMAN / asdf / Homebrew"}）` : null;
    return {
      sel, emptyHint,
      /** 当前选择对应的 {version, path}；选「系统默认」为 null */
      value() {
        if (!sel.value) return null;
        if (current?.path === sel.value) return current;
        const r = list.find((x) => x.path === sel.value);
        return r ? { version: r.version, path: r.path } : null;
      },
    };
  }

  async function loadRuntimes() {
    try {
      return await api.listRuntimes();
    } catch (e) {
      toast(`检测 Node / JDK 版本失败：${String(e)}`, "error");
      return { node: [], java: [] };
    }
  }

  // 在列表里直接切换某个项目的 Node / JDK 版本
  async function openRuntimeDialog(p, kind) {
    const rts = await loadRuntimes();
    const label = kind === "node" ? "Node" : "JDK";
    const rs = runtimeSelect(kind, rts[kind], p[kind]);
    const body = el("div", { class: "form" }, [
      el("label", { class: "form-row" }, [el("span", {}, `${label} 版本`), rs.sel]),
      el("div", { class: "note" }, rs.emptyHint || `选定后，启动「${p.name}」时会优先使用该版本${kind === "java" ? "（同时设置 JAVA_HOME）" : ""}。`),
    ]);
    showModal(`切换 ${label} 版本 · ${p.name}`, body, async () => {
      const next = rs.value();
      try {
        await api.saveProject({ ...p, [kind]: next });
      } catch (e) {
        toast(String(e), "error");
        return false;
      }
      const running = statuses[p.id]?.running;
      const what = next ? `${label} ${next.version}` : `系统默认 ${label}`;
      toast(running ? `已切换为 ${what}；项目正在运行，需点「重启」才会生效` : `已切换为 ${what}`, running ? "warning" : "success");
      await refresh();
      return true;
    });
  }

  // 启动 / 重启并反馈结果：成功、成功但有提醒、失败（含原因）。
  // 后端会等到端口就绪或进程提前退出才返回，期间行内显示「启动中…」。
  async function launch(p, fn) {
    starting.add(p.id);
    render();
    renderGroupTools();
    try {
      const out = await fn(p.id);
      toast(`${p.name}：${out.message}`, out.level === "warning" ? "warning" : "success");
    } catch (e) {
      toast(`${p.name}：${String(e)}`, "error");
    } finally {
      starting.delete(p.id);
      await refresh();
    }
  }

  async function act(promise, okMsg) {
    await guard(promise, okMsg);
    await refresh();
  }

  async function removeProject(p) {
    if (!(await confirmDialog(`确定删除项目「${p.name}」？（会先停止其进程）`))) return;
    await guard(api.deleteProject(p.id), "已删除");
    await refresh();
  }

  // 单独查看某个项目的实时日志
  function openLogModal(p) {
    const body = el("pre", { class: "term-log" }, "加载日志中…");
    let alive = true;
    let interval = null;

    async function pull() {
      try {
        const lines = await api.getLogs(p.id);
        if (!alive) return;
        if (!lines || lines.length === 0) {
          body.textContent = "暂无日志输出";
          return;
        }
        const atBottom = body.scrollTop + body.clientHeight >= body.scrollHeight - 20;
        body.innerHTML = "";
        for (const l of lines) {
          const line = el("div", { class: `log-line ${l.stream}` }, [
            el("span", { class: "log-ts" }, l.ts),
            el("span", { class: "log-text" }, l.text),
          ]);
          body.append(line);
        }
        if (atBottom) body.scrollTop = body.scrollHeight;
      } catch (_) {}
    }

    showTermModal(`日志 · ${p.name}`, body, () => {
      alive = false;
      if (interval) clearInterval(interval);
    });
    pull();
    interval = setInterval(pull, 1000);
  }

  async function openEditor(p) {
    const isNew = !p;
    const rts = await loadRuntimes();
    const data = p
      ? { ...p, env: p.env || {} }
      : { id: "", name: "", path: "", kind: "frontend", start_command: "", stop_command: "", port: "", url: "", editor: "code", auto_restart: false, env: {},
        // 在某个分组 tab 里新增时，默认放进该分组
        group: groups.includes(activeTab) ? activeTab : null };

    const f = (name, label, placeholder = "", type = "text") =>
      el("label", { class: "form-row" }, [
        el("span", {}, label),
        el("input", { name, type, placeholder, value: data[name] ?? "" }),
      ]);

    const kindSel = el("select", { name: "kind" }, ["frontend", "backend", "other"].map((k) =>
      el("option", { value: k, selected: data.kind === k ? "selected" : false }, kindLabel(k))
    ));

    const groupSel = el("select", { name: "group" }, [
      el("option", { value: "" }, "未分组"),
      ...groups.map((g) => el("option", { value: g, selected: data.group === g ? "selected" : false }, g)),
    ]);

    // 前端项目选 Node，后端项目选 JDK，「其它」两者都可选
    const nodeRs = runtimeSelect("node", rts.node, data.node);
    const javaRs = runtimeSelect("java", rts.java, data.java);
    const rtRow = (label, rs) =>
      el("label", { class: "form-row" }, [
        el("span", {}, label),
        rs.sel,
        rs.emptyHint && el("small", { class: "dim" }, rs.emptyHint),
      ]);
    const nodeRow = rtRow("Node 版本", nodeRs);
    const javaRow = rtRow("JDK 版本", javaRs);
    const syncRuntimeRows = () => {
      nodeRow.style.display = kindSel.value === "backend" ? "none" : "";
      javaRow.style.display = kindSel.value === "frontend" ? "none" : "";
    };
    kindSel.addEventListener("change", syncRuntimeRows);
    syncRuntimeRows();

    const autoRestart = el("input", { type: "checkbox", name: "auto_restart" });
    if (data.auto_restart) autoRestart.checked = true;

    const envText = Object.entries(data.env || {}).map(([k, v]) => `${k}=${v}`).join("\n");
    const envArea = el("textarea", { name: "env", rows: "3", placeholder: "KEY=VALUE，每行一个" }, envText);

    // 工作目录行：输入框 + 「浏览选择目录」 + 「自动获取项目信息」按钮
    const pathInput = el("input", { name: "path", type: "text", placeholder: "/Users/you/projects/app", value: data.path ?? "" });
    const browseBtn = el("button", { class: "browse-btn", type: "button", title: "选择本地目录" });
    browseBtn.append(icon("folder", 15), el("span", {}, "浏览"));
    const detectBtn = el("button", { class: "detect-btn", type: "button" });
    detectBtn.append(icon("magic", 15), el("span", {}, "自动获取"));
    const pathRow = el("label", { class: "form-row" }, [
      el("span", {}, "工作目录"),
      el("div", { class: "path-row" }, [pathInput, browseBtn, detectBtn]),
    ]);

    // 「浏览」：弹出 macOS 原生选择文件夹对话框
    browseBtn.onclick = async () => {
      browseBtn.disabled = true;
      try {
        const picked = await api.pickDirectory();
        if (picked) {
          pathInput.value = picked.replace(/\/$/, "");
          const nameInput = form.querySelector('[name="name"]');
          if (nameInput && !nameInput.value.trim()) {
            nameInput.value = picked.replace(/\/$/, "").split("/").pop();
          }
        }
      } catch (e) {
        toast(String(e), "error");
      } finally {
        browseBtn.disabled = false;
      }
    };

    // 启动命令：终端命令行风格，可多行换行
    const cmdInput = el("textarea", { name: "start_command", class: "cmd-field", rows: "2", placeholder: "npm run dev" }, data.start_command ?? "");
    const cmdRow = el("label", { class: "form-row" }, [
      el("span", {}, "启动命令"),
      el("div", { class: "cmd-input" }, [el("span", { class: "cmd-prompt" }, "$"), cmdInput]),
    ]);

    const form = el("div", { class: "form" }, [
      f("name", "名称", "我的前端"),
      pathRow,
      el("label", { class: "form-row" }, [el("span", {}, "类型"), kindSel]),
      el("label", { class: "form-row" }, [el("span", {}, "所属项目组"), groupSel]),
      cmdRow,
      nodeRow,
      javaRow,
      f("stop_command", "停止命令(可选)", "留空则由 DevBox 结束进程树"),
      f("port", "端口(可选)", "3000", "number"),
      f("url", "打开地址(可选)", "http://localhost:3000"),
      f("editor", "编辑器命令", "code"),
      el("label", { class: "form-row" }, [el("span", {}, "环境变量"), envArea]),
      el("label", { class: "form-row checkbox" }, [autoRestart, el("span", {}, "崩溃后自动重启")]),
    ]);

    // 「自动获取项目信息」：读取目录里的配置文件回填表单
    detectBtn.onclick = async () => {
      const path = pathInput.value.trim();
      if (!path) {
        toast("请先填写工作目录", "error");
        return;
      }
      detectBtn.disabled = true;
      try {
        const d = await api.detectProject(path);
        const nameInput = form.querySelector('[name="name"]');
        if (d.name && !nameInput.value.trim()) nameInput.value = d.name;
        if (d.kind) kindSel.value = d.kind;
        if (d.start_command) cmdInput.value = d.start_command;
        if (d.port != null) {
          const portInput = form.querySelector('[name="port"]');
          if (portInput) portInput.value = d.port;
        }
        if (d.url) {
          const urlInput = form.querySelector('[name="url"]');
          if (urlInput && !urlInput.value.trim()) urlInput.value = d.url;
        }
        toast(d.summary || "已自动填充", "success");
      } catch (e) {
        toast(String(e), "error");
      } finally {
        detectBtn.disabled = false;
      }
    };

    showModal(isNew ? "新增项目" : "编辑项目", form, async () => {
      const get = (n) => form.querySelector(`[name="${n}"]`).value.trim();
      const env = {};
      envArea.value.split("\n").map((l) => l.trim()).filter(Boolean).forEach((line) => {
        const i = line.indexOf("=");
        if (i > 0) env[line.slice(0, i).trim()] = line.slice(i + 1).trim();
      });
      const project = {
        id: data.id || "",
        name: get("name"),
        path: get("path"),
        kind: kindSel.value,
        group: groupSel.value || null,
        // 当前类型下看不到的那一项视为不指定，避免残留的旧选择悄悄生效
        node: nodeRow.style.display === "none" ? null : nodeRs.value(),
        java: javaRow.style.display === "none" ? null : javaRs.value(),
        start_command: get("start_command"),
        stop_command: get("stop_command") || null,
        port: get("port") ? Number(get("port")) : null,
        url: get("url") || null,
        editor: get("editor") || null,
        auto_restart: autoRestart.checked,
        env,
      };
      if (!project.name || !project.path || !project.start_command) {
        toast("名称、工作目录、启动命令为必填", "error");
        return false;
      }
      await guard(api.saveProject(project), "已保存");
      await refresh();
      return true;
    });
  }

  refresh();
  timer = setInterval(async () => {
    try {
      const arr = await api.projectStatuses();
      statuses = Object.fromEntries(arr.map((s) => [s.id, s]));
      render();
      renderGroupTools(); // 批量按钮的可用状态随运行状态变化
      await api.healthTick();
    } catch (_) {}
  }, 2500);

  return () => {
    clearInterval(timer);
    if (drag) endDrag(); // 切换视图时收尾，避免卡片残留在 body 上
  };
}

function kindLabel(k) {
  return { frontend: "前端", backend: "后端", other: "其它" }[k] || k;
}

// 简易模态框
function showModal(title, body, onOk) {
  const overlay = el("div", { class: "modal-overlay" });
  const okBtn = el("button", { class: "primary-btn" }, "保存");
  const cancelBtn = el("button", { class: "ghost-btn" }, "取消");
  const modal = el("div", { class: "modal" }, [
    el("div", { class: "modal-header" }, title),
    el("div", { class: "modal-body" }, body),
    el("div", { class: "modal-footer" }, [cancelBtn, okBtn]),
  ]);
  overlay.append(modal);
  document.body.append(overlay);
  const close = () => overlay.remove();
  cancelBtn.onclick = close;
  overlay.onclick = (e) => { if (e.target === overlay) close(); };
  okBtn.onclick = async () => {
    const ok = await onOk();
    if (ok !== false) close();
  };
}

// 只读的终端风格日志模态框（带关闭回调用于清理定时器）
function showTermModal(title, body, onClose) {
  const overlay = el("div", { class: "modal-overlay" });
  const closeBtn = el("button", { class: "ghost-btn" }, "关闭");
  const modal = el("div", { class: "modal modal-lg term-modal" }, [
    el("div", { class: "modal-header" }, title),
    el("div", { class: "modal-body" }, body),
    el("div", { class: "modal-footer" }, [closeBtn]),
  ]);
  overlay.append(modal);
  document.body.append(overlay);
  const close = () => {
    try { onClose && onClose(); } catch (_) {}
    overlay.remove();
  };
  closeBtn.onclick = close;
  overlay.onclick = (e) => { if (e.target === overlay) close(); };
}
