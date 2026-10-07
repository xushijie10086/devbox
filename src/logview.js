// 日志查看器：项目行的日志弹窗和「日志」页共用。
// 功能：ANSI 颜色、错误 / 警告高亮、搜索（含高亮匹配）、级别过滤、复制、自动滚动、清空、打开日志文件。
// 通过序号增量拉取，只追加新行，不整体重绘。

import { el, toast } from "./ui.js";
import { parseAnsi, matchRanges, filterLines, formatForCopy, lineLevel, isStackLine } from "./logfmt.js";

const MAX_LINES = 5000; // 前端最多保留的行数
const MAX_RENDER = 3000; // 最多渲染到页面上的行数

/** 把一段文本按高亮区间切开，匹配到的用 <mark> 包起来 */
function appendHighlighted(parent, text, offset, ranges) {
  let pos = 0;
  for (const [s, e] of ranges) {
    const a = Math.max(s - offset, 0);
    const b = Math.min(e - offset, text.length);
    if (b <= a) continue;
    if (a > pos) parent.append(text.slice(pos, a));
    parent.append(el("mark", {}, text.slice(a, b)));
    pos = b;
  }
  if (pos < text.length) parent.append(text.slice(pos));
}

function buildRow(l, term) {
  const segs = parseAnsi(l.text);
  const plain = segs.map((s) => s.text).join("");
  const ranges = matchRanges(plain, term);
  const level = lineLevel(l);
  const textEl = el("span", { class: "log-text" });
  let offset = 0;
  for (const seg of segs) {
    const hasStyle = Object.keys(seg.style).length > 0;
    const host = hasStyle ? el("span") : textEl;
    if (hasStyle) {
      const st = seg.style;
      if (st.fg) host.style.color = st.fg;
      if (st.bg) { host.style.background = st.bg; host.style.borderRadius = "2px"; }
      if (st.bold) host.style.fontWeight = "700";
      if (st.dim) host.style.opacity = ".65";
      if (st.italic) host.style.fontStyle = "italic";
      if (st.underline) host.style.textDecoration = "underline";
      textEl.append(host);
    }
    appendHighlighted(host, seg.text, offset, ranges);
    offset += seg.text.length;
  }
  const cls = ["log-line", l.stream];
  if (level) cls.push(`lvl-${level}`);
  if (isStackLine(l.text)) cls.push("stack");
  return el("div", { class: cls.join(" ") }, [el("span", { class: "log-ts" }, l.ts), textEl]);
}

async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch (_) {
    // 个别 WebView 不允许直接写剪贴板：退回到选中一个临时文本框再复制
    const ta = el("textarea", { style: "position:fixed;left:-9999px;top:0" });
    ta.value = text;
    document.body.append(ta);
    ta.select();
    let ok = false;
    try { ok = document.execCommand("copy"); } catch (_) {}
    ta.remove();
    return ok;
  }
}

/**
 * fetchChunk(after, epoch) -> Promise<{ epoch, lines }>；onClear() 清空；onRevealFile() 打开日志文件位置。
 * 返回 { root, start, stop, setSource }。
 */
export function createLogView({ fetchChunk, onClear, onRevealFile, height }) {
  let fetchFn = fetchChunk;
  let lines = [];
  let lastSeq = 0;
  let epoch = null;
  let term = "";
  let level = "all";
  let autoscroll = true;
  let timer = null;
  let fetching = false;
  let alive = true;
  let programmatic = false;

  const search = el("input", { class: "search", type: "text", placeholder: "搜索日志…（⌘F）" });
  const levelSel = el("select", { class: "log-select" }, [
    el("option", { value: "all" }, "全部级别"),
    el("option", { value: "warn" }, "警告及以上"),
    el("option", { value: "error" }, "仅错误"),
  ]);
  const count = el("span", { class: "lv-count" });
  const auto = el("input", { type: "checkbox" });
  auto.checked = true;
  const body = el("div", { class: "logview-body" });
  if (height) body.style.height = height;
  const emptyEl = el("div", { class: "empty" });

  const bar = el("div", { class: "logview-bar" }, [
    search,
    levelSel,
    count,
    el("span", { class: "quick-spacer" }),
    el("label", { class: "scroll-toggle" }, [auto, el("span", {}, "自动滚动")]),
    el("button", { class: "ghost-btn sm", title: "复制当前显示的日志（已按搜索和级别过滤，不含颜色转义）", onclick: copyVisible }, "复制"),
    el("button", { class: "ghost-btn sm", title: "清空日志（连同磁盘上的日志文件）", onclick: clearAll }, "清空"),
    onRevealFile ? el("button", { class: "ghost-btn sm", title: "在访达里定位日志文件", onclick: () => onRevealFile() }, "日志文件") : null,
  ]);
  const root = el("div", { class: "logview" }, [bar, body]);

  // ---- 渲染 ----

  const passes = (l) => filterLines([l], { term, level }).length === 1;

  function rowCount() {
    return body.querySelectorAll(".log-line").length;
  }

  function refreshMeta() {
    const shown = rowCount();
    count.textContent = term || level !== "all" ? `匹配 ${shown} / 共 ${lines.length} 行` : `共 ${lines.length} 行`;
    const hasRows = shown > 0;
    if (!hasRows) {
      emptyEl.textContent = lines.length === 0 ? "暂无输出。项目运行后这里会实时显示 stdout / stderr。" : "没有匹配的日志行。";
      if (!emptyEl.isConnected) body.append(emptyEl);
    } else if (emptyEl.isConnected) {
      emptyEl.remove();
    }
  }

  function scrollToBottom() {
    programmatic = true;
    body.scrollTop = body.scrollHeight;
    requestAnimationFrame(() => { programmatic = false; });
  }

  /** 过滤条件变了：整体重绘 */
  function rerender() {
    body.replaceChildren();
    const rows = filterLines(lines, { term, level }).slice(-MAX_RENDER);
    const frag = document.createDocumentFragment();
    for (const l of rows) frag.append(buildRow(l, term));
    body.append(frag);
    refreshMeta();
    if (autoscroll) scrollToBottom();
  }

  /** 来了新行：只追加 */
  function appendLines(fresh) {
    lines.push(...fresh);
    if (lines.length > MAX_LINES) lines = lines.slice(-MAX_LINES);
    if (emptyEl.isConnected) emptyEl.remove();
    for (const l of fresh) {
      if (passes(l)) body.append(buildRow(l, term));
    }
    while (rowCount() > MAX_RENDER) body.querySelector(".log-line")?.remove();
    refreshMeta();
    if (autoscroll) scrollToBottom();
  }

  async function pull() {
    if (fetching || !alive) return;
    fetching = true;
    try {
      const chunk = await fetchFn(lastSeq, epoch);
      if (!alive) return;
      if (epoch !== null && chunk.epoch !== epoch) {
        // 刚被清空过（或在别处清空）：丢掉旧内容，从头开始
        lines = [];
        lastSeq = 0;
        body.replaceChildren();
      }
      epoch = chunk.epoch;
      const fresh = chunk.lines.filter((l) => l.seq > lastSeq);
      if (fresh.length > 0) {
        lastSeq = fresh[fresh.length - 1].seq;
        appendLines(fresh);
      } else {
        refreshMeta();
      }
    } catch (_) {
      // 拉取失败（后端暂时不可用等）：下一轮再试
    } finally {
      fetching = false;
    }
  }

  // ---- 交互 ----

  let searchTimer = null;
  search.addEventListener("input", () => {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      term = search.value.trim();
      rerender();
    }, 150);
  });
  levelSel.addEventListener("change", () => {
    level = levelSel.value;
    rerender();
  });
  auto.addEventListener("change", () => {
    autoscroll = auto.checked;
    if (autoscroll) scrollToBottom();
  });
  // 手动往上翻就暂停自动滚动，滚回底部再恢复
  body.addEventListener("scroll", () => {
    if (programmatic) return;
    const atBottom = body.scrollTop + body.clientHeight >= body.scrollHeight - 24;
    if (atBottom !== autoscroll) {
      autoscroll = atBottom;
      auto.checked = atBottom;
    }
  });
  root.addEventListener("keydown", (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "f") {
      e.preventDefault();
      search.focus();
      search.select();
    }
  });

  async function copyVisible() {
    const rows = filterLines(lines, { term, level });
    if (rows.length === 0) return toast("没有可复制的日志", "info");
    const ok = await copyText(formatForCopy(rows));
    toast(ok ? `已复制 ${rows.length} 行` : "复制失败，请手动选中后按 ⌘C", ok ? "success" : "error");
  }

  async function clearAll() {
    try {
      await onClear?.();
    } catch (e) {
      return toast(String(e), "error");
    }
    lines = [];
    lastSeq = 0;
    epoch = null;
    body.replaceChildren();
    refreshMeta();
    pull();
  }

  refreshMeta();

  return {
    root,
    start() {
      alive = true;
      pull();
      timer = setInterval(pull, 1000);
    },
    stop() {
      alive = false;
      clearInterval(timer);
      clearTimeout(searchTimer);
    },
    /** 换一个日志来源（「日志」页切换项目）：重置并重新拉取 */
    setSource(fn) {
      fetchFn = fn;
      lines = [];
      lastSeq = 0;
      epoch = null;
      body.replaceChildren();
      refreshMeta();
      pull();
    },
  };
}
