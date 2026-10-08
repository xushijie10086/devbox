// 日志的纯逻辑：ANSI 颜色解析、错误 / 警告识别、搜索匹配、过滤、复制格式。
// 不依赖 DOM，可以直接用 node 跑单元测试（见 logfmt.test.js）。

// 16 色调色板（适配深色背景）：0-7 普通色，8-15 亮色
const PALETTE = [
  "#6b7280", "#ef5b6b", "#37c871", "#f0b429", "#4f8cff", "#c678dd", "#56b6c2", "#d7dbe3",
  "#9ca3af", "#ff7b86", "#5fe39a", "#ffd166", "#7fb0ff", "#e099f2", "#7fd6e0", "#ffffff",
];

/** 256 色表：前 16 个同上，16-231 是 6x6x6 色立方，232-255 是灰阶 */
function color256(n) {
  if (n < 16) return PALETTE[n];
  if (n >= 232) {
    const v = 8 + (n - 232) * 10;
    return `rgb(${v},${v},${v})`;
  }
  const i = n - 16;
  const level = (x) => (x === 0 ? 0 : 55 + x * 40);
  return `rgb(${level(Math.floor(i / 36))},${level(Math.floor(i / 6) % 6)},${level(i % 6)})`;
}

// 除 SGR（以 m 结尾，用来上色）之外的转义序列：光标移动、清行、OSC 标题等，一律丢弃
const OTHER_ESCAPES = /\x1b\[[0-9;?]*[A-Za-ln-z]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[()][A-Za-z0-9]/g;
const SGR = /\x1b\[([0-9;]*)m/g;

/** 进度条类输出用 \r 在原地覆盖：只保留最后一次写入的内容 */
function afterCarriageReturn(text) {
  if (!text.includes("\r")) return text;
  const parts = text.split("\r").filter((p) => p.length > 0);
  return parts.length ? parts[parts.length - 1] : "";
}

function applySgr(style, codes) {
  const s = { ...style };
  const list = codes.length ? codes : [0];
  for (let i = 0; i < list.length; i++) {
    const c = list[i];
    if (c === 0) { delete s.fg; delete s.bg; delete s.bold; delete s.dim; delete s.italic; delete s.underline; }
    else if (c === 1) s.bold = true;
    else if (c === 2) s.dim = true;
    else if (c === 3) s.italic = true;
    else if (c === 4) s.underline = true;
    else if (c === 22) { delete s.bold; delete s.dim; }
    else if (c === 23) delete s.italic;
    else if (c === 24) delete s.underline;
    else if (c >= 30 && c <= 37) s.fg = PALETTE[c - 30];
    else if (c >= 90 && c <= 97) s.fg = PALETTE[c - 90 + 8];
    else if (c === 39) delete s.fg;
    else if (c >= 40 && c <= 47) s.bg = PALETTE[c - 40];
    else if (c >= 100 && c <= 107) s.bg = PALETTE[c - 100 + 8];
    else if (c === 49) delete s.bg;
    else if (c === 38 || c === 48) {
      const key = c === 38 ? "fg" : "bg";
      if (list[i + 1] === 5 && Number.isInteger(list[i + 2])) { s[key] = color256(list[i + 2]); i += 2; }
      else if (list[i + 1] === 2 && [2, 3, 4].every((k) => Number.isInteger(list[i + k]))) {
        s[key] = `rgb(${list[i + 2]},${list[i + 3]},${list[i + 4]})`;
        i += 4;
      }
    }
  }
  return s;
}

/** 把带 ANSI 转义的一行拆成 [{ text, style }]，style 里有 fg / bg / bold / dim / italic / underline */
export function parseAnsi(raw) {
  const text = afterCarriageReturn(raw).replace(OTHER_ESCAPES, "");
  const out = [];
  let style = {};
  let last = 0;
  let m;
  SGR.lastIndex = 0;
  while ((m = SGR.exec(text)) !== null) {
    if (m.index > last) out.push({ text: text.slice(last, m.index), style });
    style = applySgr(style, m[1] === "" ? [] : m[1].split(";").map((x) => parseInt(x, 10)).filter((x) => !Number.isNaN(x)));
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push({ text: text.slice(last).replace(/\x1b/g, ""), style });
  return out.length ? out : [{ text: "", style: {} }];
}

/** 去掉所有转义序列，得到纯文本（用于搜索、复制、级别识别） */
export function stripAnsi(raw) {
  return afterCarriageReturn(raw).replace(OTHER_ESCAPES, "").replace(SGR, "").replace(/\x1b/g, "");
}

const ERROR_PATTERNS = [
  /\b(ERROR|FATAL|SEVERE|CRITICAL)\b/, // 日志框架的大写级别
  /\b[A-Za-z0-9_$.]*(Exception|Error)\b(?=[:\s]|$)/, // java.lang.NullPointerException / TypeError: ...
  /\bCaused by:/,
  /Traceback \(most recent call last\)/,
  /\bnpm ERR!/,
  /^\s*error(\[\w+\])?:/i, // rustc / gcc / tsc 的 "error: ..." "error[E0308]: ..."
  /^\s*(panic|fatal):/i,
  /[✖✘]/,
];
const WARN_PATTERNS = [/\b(WARN|WARNING)\b/, /^\s*warning(\[\w+\])?:/i, /\bnpm WARN\b/, /⚠/];

/** 判断一行的级别："error" | "warn" | null。不看 stderr 本身：很多工具把普通信息也写到 stderr */
export function detectLevel(text) {
  const t = stripAnsi(text);
  if (ERROR_PATTERNS.some((r) => r.test(t))) return "error";
  if (WARN_PATTERNS.some((r) => r.test(t))) return "warn";
  return null;
}

/**
 * 一行日志的级别。系统消息（DevBox 自己写的「▶ 启动: <命令>」之类）不按内容判断——
 * 命令回显里可能恰好含有 [ERROR] 字样——只认它自己的前缀符号：✖ 失败，⚠ 警告。
 */
export function lineLevel(line) {
  if (line.stream === "system") {
    const t = stripAnsi(line.text);
    if (/^\s*[✖✘]/.test(t)) return "error";
    if (/^\s*⚠/.test(t)) return "warn";
    return null;
  }
  return detectLevel(line.text);
}

/** 堆栈续行（"    at com.foo.Bar(Bar.java:12)"）：显示得淡一些，不当成新的错误 */
export function isStackLine(text) {
  return /^\s+at\s+\S/.test(stripAnsi(text)) || /^\s+\.\.\. \d+ (more|common frames omitted)/.test(stripAnsi(text));
}

/** 在纯文本里查找所有匹配（不区分大小写），返回 [[start, end), ...]。term 为空返回空数组 */
export function matchRanges(text, term) {
  if (!term) return [];
  const hay = text.toLowerCase();
  const needle = term.toLowerCase();
  const out = [];
  let from = 0;
  for (;;) {
    const i = hay.indexOf(needle, from);
    if (i < 0) break;
    out.push([i, i + needle.length]);
    from = i + needle.length;
  }
  return out;
}

/**
 * 过滤日志行。level: "all" | "warn"（警告及以上）| "error"（仅错误）；term 为搜索词。
 * 日志级别按单行判断，所以「仅错误」会隐藏堆栈续行——需要看完整堆栈就用搜索或选「全部」。
 */
export function filterLines(lines, { term = "", level = "all" } = {}) {
  const rank = { error: 2, warn: 1 };
  const need = level === "error" ? 2 : level === "warn" ? 1 : 0;
  return lines.filter((l) => {
    if (need > 0 && (rank[lineLevel(l)] || 0) < need) return false;
    if (term && matchRanges(stripAnsi(l.text), term).length === 0) return false;
    return true;
  });
}

/** 复制用的纯文本：每行「时间 内容」，去掉颜色转义 */
export function formatForCopy(lines) {
  return lines.map((l) => `${l.ts} ${stripAnsi(l.text)}`).join("\n");
}
