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
  }, 3200);
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

export function fmtUptime(secs) {
  if (secs == null) return "—";
  if (secs < 60) return `${secs}s`;
  const m = Math.floor(secs / 60);
  if (m < 60) return `${m}m${secs % 60}s`;
  const h = Math.floor(m / 60);
  return `${h}h${m % 60}m`;
}
