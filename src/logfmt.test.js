import test from "node:test";
import assert from "node:assert/strict";
import { parseAnsi, stripAnsi, detectLevel, lineLevel, isStackLine, matchRanges, filterLines, formatForCopy } from "./logfmt.js";

const E = "\x1b";

test("ANSI：基础前景色、重置、粗体", () => {
  const segs = parseAnsi(`${E}[31m错误${E}[0m 正常 ${E}[1;32m成功${E}[0m`);
  assert.deepEqual(segs.map((s) => s.text), ["错误", " 正常 ", "成功"]);
  assert.equal(segs[0].style.fg, "#ef5b6b");
  assert.deepEqual(segs[1].style, {});
  assert.equal(segs[2].style.bold, true);
  assert.equal(segs[2].style.fg, "#37c871");
});

test("ANSI：亮色、背景色、256 色、真彩色", () => {
  assert.equal(parseAnsi(`${E}[91mx`)[0].style.fg, "#ff7b86");
  assert.equal(parseAnsi(`${E}[44mx`)[0].style.bg, "#4f8cff");
  assert.equal(parseAnsi(`${E}[38;5;2mx`)[0].style.fg, "#37c871", "256 色的前 16 个同基础色");
  assert.equal(parseAnsi(`${E}[38;5;196mx`)[0].style.fg, "rgb(255,0,0)");
  assert.equal(parseAnsi(`${E}[38;5;244mx`)[0].style.fg, "rgb(128,128,128)");
  assert.equal(parseAnsi(`${E}[38;2;10;20;30mx`)[0].style.fg, "rgb(10,20,30)");
  // 样式会延续到下一段，直到重置；39 只恢复前景色
  const segs = parseAnsi(`${E}[31;1mA${E}[39mB${E}[22mC`);
  assert.deepEqual(segs.map((s) => [s.text, s.style.fg, s.style.bold]), [["A", "#ef5b6b", true], ["B", undefined, true], ["C", undefined, undefined]]);
});

test("ANSI：光标 / 清行 / 标题等其它转义被丢弃，不留乱码", () => {
  assert.equal(stripAnsi(`${E}[2K${E}[1G进度 ${E}[32m100%${E}[0m${E}]0;标题\x07完成`), "进度 100%完成");
  assert.equal(stripAnsi(`${E}[?25lhello${E}[?25h`), "hello");
  assert.equal(stripAnsi("单独的 \x1b 残留"), "单独的  残留");
});

test("\\r 覆盖写（进度条）只保留最后一次", () => {
  assert.equal(stripAnsi("10%\r50%\r100% done"), "100% done");
  assert.equal(stripAnsi("abc\r"), "abc", "行尾的 \\r 不应让内容消失");
  assert.equal(parseAnsi("a\rb").map((s) => s.text).join(""), "b");
});

test("没有转义的普通文本原样返回", () => {
  assert.deepEqual(parseAnsi("普通 text"), [{ text: "普通 text", style: {} }]);
  assert.deepEqual(parseAnsi(""), [{ text: "", style: {} }]);
});

test("级别识别：错误", () => {
  for (const t of [
    "[ERROR] No plugin found for prefix 'spring-boot'",
    "2026-10-07 FATAL: out of memory",
    "java.lang.NullPointerException: x is null",
    "TypeError: Cannot read properties of undefined",
    "Caused by: java.sql.SQLException",
    "Traceback (most recent call last):",
    "npm ERR! code ELIFECYCLE",
    "error[E0308]: mismatched types",
    "error: could not compile",
    "✖ 脚本失败",
    `${E}[31mERROR${E}[0m something`,
  ]) assert.equal(detectLevel(t), "error", t);
});

test("级别识别：警告与普通行（避免误报）", () => {
  for (const t of ["[WARN] deprecated api", "warning: unused variable", "npm WARN deprecated foo", "⚠ 注意"]) assert.equal(detectLevel(t), "warn", t);
  for (const t of [
    "Server started on port 8080", "0 errors, 0 warnings", "errors: none", "handled the exception gracefully",
    "INFO  Starting app", "error_count=0", "编译完成",
  ]) assert.equal(detectLevel(t), null, t);
});

test("堆栈续行识别", () => {
  assert.ok(isStackLine("    at com.foo.Bar.baz(Bar.java:12)"));
  assert.ok(isStackLine("\t... 23 more"));
  assert.ok(!isStackLine("at the end of the day"));
  assert.ok(!isStackLine("Caused by: x"));
});

test("搜索：不区分大小写、多处匹配、不重叠", () => {
  assert.deepEqual(matchRanges("Error error ERROR", "error"), [[0, 5], [6, 11], [12, 17]]);
  assert.deepEqual(matchRanges("aaaa", "aa"), [[0, 2], [2, 4]]);
  assert.deepEqual(matchRanges("中文日志 中文", "中文"), [[0, 2], [5, 7]]);
  assert.deepEqual(matchRanges("abc", ""), []);
  assert.deepEqual(matchRanges("abc", "zzz"), []);
});

const L = (text, ts = "10:00:00") => ({ ts, stream: "stdout", text });

test("过滤：按搜索词、按级别，以及两者组合；识别和搜索都基于可见文本（不被颜色转义打断）", () => {
  const split = `${E}[31mERR${E}[0mOR red`; // 被颜色拆开的 "ERROR red"，可见文本是一个错误行
  const lines = [L("INFO started"), L("[WARN] slow query"), L("[ERROR] db down"), L("    at Foo.bar(Foo.java:1)"), L(split)];
  const texts = (r) => r.map((l) => l.text);
  assert.equal(filterLines(lines, {}).length, 5);
  assert.deepEqual(texts(filterLines(lines, { level: "error" })), ["[ERROR] db down", split]);
  assert.deepEqual(texts(filterLines(lines, { level: "warn" })), ["[WARN] slow query", "[ERROR] db down", split]);
  assert.deepEqual(texts(filterLines(lines, { term: "DB" })), ["[ERROR] db down"]);
  assert.deepEqual(texts(filterLines(lines, { term: "error", level: "error" })), ["[ERROR] db down", split]);
  assert.deepEqual(texts(filterLines(lines, { term: "ERROR red" })), [split], "带颜色转义的行按可见文本匹配");
  assert.deepEqual(texts(filterLines(lines, { term: "foo.bar" })), ["    at Foo.bar(Foo.java:1)"], "搜索能找到堆栈行");
  assert.equal(filterLines(lines, { term: "nope" }).length, 0);
});

test("复制：每行「时间 内容」，不含颜色转义", () => {
  const out = formatForCopy([L(`${E}[31mboom${E}[0m`, "10:00:01"), L("plain", "10:00:02")]);
  assert.equal(out, "10:00:01 boom\n10:00:02 plain");
  assert.equal(formatForCopy([]), "");
});

test("系统消息不按内容判断级别（启动命令里可能恰好含有 [ERROR] 字样），只认自己的前缀符号", () => {
  const sys = (text) => ({ ts: "t", stream: "system", text });
  assert.equal(lineLevel(sys("▶ 启动: echo '[ERROR] boom'; exit 1 (pid 1)")), null, "命令回显不是错误");
  assert.equal(lineLevel(sys("▶ 脚本「build」: ./gradlew build 2>&1 | grep ERROR")), null);
  assert.equal(lineLevel(sys("✖ 启动失败：进程异常退出（退出码 3）")), "error");
  assert.equal(lineLevel(sys("✖ 进程已退出：被信号 9 终止")), "error");
  assert.equal(lineLevel(sys("⚠ 端口被占用")), "warn");
  assert.equal(lineLevel(sys("✔ 脚本完成")), null);
  // 普通输出照旧按内容判断
  assert.equal(lineLevel({ ts: "t", stream: "stderr", text: "[ERROR] boom" }), "error");
  assert.equal(lineLevel({ ts: "t", stream: "stdout", text: "INFO ok" }), null);
  // 过滤也按这个规则：「仅错误」不会把含 [ERROR] 的启动命令回显算进去
  const lines = [sys("▶ 启动: echo '[ERROR] x'"), sys("✖ 启动失败"), { ts: "t", stream: "stderr", text: "[ERROR] real" }];
  assert.deepEqual(filterLines(lines, { level: "error" }).map((l) => l.text), ["✖ 启动失败", "[ERROR] real"]);
});
