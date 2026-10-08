// 轻量 DOM 与提示辅助

/** 创建元素：el("div", {class:"x", onclick:fn}, [children|string]) */
export function el(tag, attrs = {}, children = []) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") node.className = v;
    else if (k === "html") node.innerHTML = v;
    else if (k.startsWith("on") && typeof v === "function") {
      node.addEventListener(k.slice(2).toLowerCase(), v);
    } else if (v !== null && v !== undefined && v !== false) {
      node.setAttribute(k, v);
    }
  }
  const kids = Array.isArray(children) ? children : [children];
  for (const c of kids) {
    if (c === null || c === undefined || c === false) continue;
    node.append(c.nodeType ? c : document.createTextNode(String(c)));
  }
  return node;
}

let toastTimer = null;
// 失败 / 警告信息里有原因和日志片段，需要更长的阅读时间
const TOAST_MS = { info: 3200, success: 3200, warning: 8000, error: 10000 };

export function toast(message, kind = "info") {
  const root = document.getElementById("toast-root");
  root.innerHTML = "";
  const node = el("div", { class: `toast toast-${kind}` }, message);
  root.append(node);
  requestAnimationFrame(() => node.classList.add("show"));
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    node.classList.remove("show");
    setTimeout(() => node.remove(), 300);
  }, TOAST_MS[kind] ?? 3200);
}

/** 包装异步操作，自动处理错误提示 */
export async function guard(promise, okMsg) {
  try {
    const r = await promise;
    if (okMsg) toast(okMsg, "success");
    return r;
  } catch (e) {
    toast(String(e), "error");
    throw e;
  }
}

/** 应用内确认弹窗，替代 Tauri WKWebView 里失效的原生 confirm()。返回 Promise<boolean> */
export function confirmDialog(message, { okText = "确定", cancelText = "取消", danger = true } = {}) {
  return new Promise((resolve) => {
    let done = false;
    const finish = (v) => {
      if (done) return;
      done = true;
      document.removeEventListener("keydown", onKey);
      overlay.remove();
      resolve(v);
    };
    const onKey = (e) => {
      if (e.key === "Escape") finish(false);
      else if (e.key === "Enter") finish(true);
    };
    const box = el("div", { class: "confirm-box" }, [
      el("div", { class: "confirm-msg" }, message),
      el("div", { class: "confirm-actions" }, [
        el("button", { class: "ghost-btn", onclick: () => finish(false) }, cancelText),
        el("button", { class: danger ? "danger-btn" : "primary-btn", onclick: () => finish(true) }, okText),
      ]),
    ]);
    const overlay = el("div", { class: "confirm-overlay", onclick: (e) => { if (e.target === overlay) finish(false); } }, [box]);
    document.addEventListener("keydown", onKey);
    document.body.append(overlay);
  });
}

/**
 * 多选一对话框：choices 为 [{ label, value, kind }]（kind: "danger" | "primary" | "ghost"），
 * 返回被点中的 value；按 Esc / 点遮罩视为取消，返回 null。
 */
export function choiceDialog(message, choices) {
  return new Promise((resolve) => {
    let done = false;
    const finish = (v) => {
      if (done) return;
      done = true;
      document.removeEventListener("keydown", onKey);
      overlay.remove();
      resolve(v);
    };
    const onKey = (e) => {
      if (e.key === "Escape") finish(null);
    };
    const buttons = choices.map((c) =>
      el("button", { class: `${c.kind === "danger" ? "danger-btn" : c.kind === "primary" ? "primary-btn" : "ghost-btn"}`, onclick: () => finish(c.value) }, c.label),
    );
    const box = el("div", { class: "confirm-box" }, [
      el("div", { class: "confirm-msg" }, message),
      el("div", { class: "confirm-actions" }, buttons),
    ]);
    const overlay = el("div", { class: "confirm-overlay", onclick: (e) => { if (e.target === overlay) finish(null); } }, [box]);
    document.addEventListener("keydown", onKey);
    document.body.append(overlay);
  });
}

/** 汇总一批启动结果（Promise.allSettled 的返回）：几个成功、哪些失败及原因，合成一条提示 */
export function summarizeStart(items, results) {
  const failed = [];
  let warned = 0;
  results.forEach((r, i) => {
    if (r.status === "rejected") failed.push(`${items[i].name}：${String(r.reason).split("\n")[0]}`);
    else if (r.value.level === "warning") warned++;
  });
  const ok = items.length - failed.length;
  if (failed.length === 0) {
    return warned
      ? { text: `已启动 ${ok} 个项目，其中 ${warned} 个需留意（见各项目提示）`, kind: "warning" }
      : { text: `已启动 ${ok} 个项目`, kind: "success" };
  }
  return { text: `成功 ${ok} 个，失败 ${failed.length} 个：\n${failed.join("\n")}`, kind: "error" };
}

/** 汇总批量拉取的结果：几个有更新（改动了什么）、几个已是最新、哪些失败及原因 */
export function summarizePull(items, results, isRunning = () => false) {
  const updated = [];
  const same = [];
  const failed = [];
  results.forEach((r, i) => {
    const name = items[i].name;
    if (!r.ok) failed.push(`${name}：${String(r.err).split("\n")[0]}`);
    else if (r.out.message.includes("已是最新")) same.push(name);
    else {
      const first = r.out.message.split("\n")[0].replace(/^分支 \S+ /, "");
      updated.push(`${name}：${first}${isRunning(items[i]) ? "（运行中，需重启）" : ""}`);
    }
  });
  const head = `拉取完成：${updated.length} 个有更新、${same.length} 个已是最新${failed.length ? `、${failed.length} 个失败` : ""}`;
  return {
    text: [head, ...updated.map((x) => `↓ ${x}`), ...failed.map((x) => `✖ ${x}`)].join("\n"),
    kind: failed.length ? "error" : "success",
  };
}

export function fmtUptime(secs) {
  if (secs == null) return "—";
  if (secs < 60) return `${secs}s`;
  const m = Math.floor(secs / 60);
  if (m < 60) return `${m}m${secs % 60}s`;
  const h = Math.floor(m / 60);
  return `${h}h${m % 60}m`;
}
