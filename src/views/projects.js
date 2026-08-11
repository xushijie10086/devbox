import { api } from "../api.js";
import { el, toast, guard, fmtUptime, confirmDialog } from "../ui.js";
import { icon } from "../icons.js";

// 生成一个带 SVG 图标的操作按钮
function iconBtn(name, title, onclick, extraClass = "") {
  const btn = el("button", { class: `icon-btn ${extraClass}`.trim(), title, onclick });
  btn.append(icon(name, 17));
  return btn;
}

export function mount(root) {
  let projects = [];
  let statuses = {};
  let timer = null;

  const list = el("div", { class: "card-grid" });
  const header = el("div", { class: "view-header" }, [
    el("h1", {}, "项目"),
    el("button", { class: "primary-btn", onclick: () => openEditor() }, "+ 新增项目"),
  ]);
  root.append(header, list);

  async function refresh() {
    projects = await api.listProjects();
    try {
      const arr = await api.projectStatuses();
      statuses = Object.fromEntries(arr.map((s) => [s.id, s]));
    } catch (e) {
      statuses = {};
    }
    render();
  }

  function render() {
    list.innerHTML = "";
    if (projects.length === 0) {
      list.append(el("div", { class: "empty" }, "还没有项目。点击右上角「新增项目」注册你的第一个项目。"));
      return;
    }
    for (const p of projects) {
      list.append(projectCard(p, statuses[p.id]));
    }
  }

  function projectCard(p, st) {
    const running = st?.running;
    const dot = el("span", { class: `dot ${running ? "on" : "off"}` });
    const kindBadge = el("span", { class: `badge badge-${p.kind}` }, kindLabel(p.kind));

    const meta = [];
    if (p.port != null) {
      const up = st?.port_up;
      meta.push(el("span", { class: "meta" }, `:${p.port} ${up ? "🟢" : "⚪"}`));
    }
    if (running) {
      meta.push(el("span", { class: "meta" }, `PID ${st.pid}`));
      meta.push(el("span", { class: "meta" }, `CPU ${(st.cpu ?? 0).toFixed(0)}%`));
      meta.push(el("span", { class: "meta" }, `${st.memory_mb ?? 0}MB`));
      meta.push(el("span", { class: "meta" }, fmtUptime(st.uptime_secs)));
    }

    const runBtns = running
      ? [
          el("button", { class: "danger-btn", onclick: () => act(api.stopProject(p.id), "已停止") }, "■ 停止"),
          el("button", { class: "ghost-btn", onclick: () => act(api.restartProject(p.id), "已重启") }, "↻ 重启"),
        ]
      : [el("button", { class: "run-btn", onclick: () => act(api.startProject(p.id), "已启动") }, "▶ 启动")];

    const quick = el("div", { class: "quick-actions" }, [
      iconBtn("code", "在编辑器打开", () => guard(api.openInEditor(p.path, p.editor))),
      p.url && iconBtn("browser", "在浏览器打开", () => guard(api.openUrl(p.url))),
      iconBtn("terminal", "在终端打开", () => guard(api.openTerminal(p.path))),
      iconBtn("folder", "在访达显示", () => guard(api.revealInFinder(p.path))),
      running && iconBtn("logsView", "查看日志", () => openLogModal(p)),
      el("span", { class: "quick-spacer" }),
      iconBtn("edit", "编辑", () => openEditor(p)),
      iconBtn("trash", "删除", () => removeProject(p), "danger"),
    ]);

    const cmd = el("div", { class: "card-cmd" }, [
      el("span", { class: "cmd-prompt" }, "$"),
      el("code", {}, p.start_command),
    ]);

    return el("div", { class: "card" }, [
      el("div", { class: "card-top" }, [dot, el("span", { class: "card-title" }, p.name), kindBadge]),
      el("div", { class: "card-path", title: p.path }, p.path),
      cmd,
      el("div", { class: "card-meta" }, meta),
      el("div", { class: "card-actions" }, [...runBtns]),
      quick,
    ]);
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

  function openEditor(p) {
    const isNew = !p;
    const data = p
      ? { ...p, env: p.env || {} }
      : { id: "", name: "", path: "", kind: "frontend", start_command: "", stop_command: "", port: "", url: "", editor: "code", auto_restart: false, env: {} };

    const f = (name, label, placeholder = "", type = "text") =>
      el("label", { class: "form-row" }, [
        el("span", {}, label),
        el("input", { name, type, placeholder, value: data[name] ?? "" }),
      ]);

    const kindSel = el("select", { name: "kind" }, ["frontend", "backend", "other"].map((k) =>
      el("option", { value: k, selected: data.kind === k ? "selected" : false }, kindLabel(k))
    ));

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
      cmdRow,
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
      await api.healthTick();
    } catch (_) {}
  }, 2500);

  return () => clearInterval(timer);
}

function kindLabel(k) {
  return { frontend: "前端", backend: "后端", other: "其它" }[k] || k;
}

// 简易模态框
export function showModal(title, body, onOk) {
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
