// 守卫：shell 脚本 / CI 命令里，变量后面不能直接紧跟中文等非 ASCII 字符。
// macOS 自带的 bash 是 3.2，会把非 ASCII 字符的第一个字节当成变量名的一部分，
// 配合 `set -u` 就报「NAME�: unbound variable」（Linux 上的新版 bash 不会，所以本地测不出来）。
// 写成 ${NAME}「…」 即可。
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

/** 找出「$name 后面直接跟非 ASCII 字符」的位置 */
export function unbracedBeforeNonAscii(text) {
  const bad = [];
  text.split("\n").forEach((line, i) => {
    // 跳过注释行；$name（不含花括号）后面紧跟非 ASCII 字符
    if (/^\s*#/.test(line)) return;
    const m = line.match(/\$[A-Za-z_][A-Za-z0-9_]*(?=[^\x00-\x7F])/g);
    if (m) bad.push({ line: i + 1, vars: m, text: line.trim() });
  });
  return bad;
}

test("检查函数本身：抓得到，也不误报", () => {
  assert.equal(unbracedBeforeNonAscii('echo "「$NAME」"').length, 1);
  assert.equal(unbracedBeforeNonAscii('echo "「${NAME}」"').length, 0);
  assert.equal(unbracedBeforeNonAscii('echo "$NAME 后面有空格"').length, 0);
  assert.equal(unbracedBeforeNonAscii('echo "价格：$5"').length, 0);
  assert.equal(unbracedBeforeNonAscii('# 注释里的 $NAME」不算').length, 0);
});

test("scripts/*.sh 和 .github/workflows/*.yml 里没有「$变量」紧跟非 ASCII 字符的写法", () => {
  const files = [
    ...readdirSync(join(root, "scripts")).filter((f) => f.endsWith(".sh")).map((f) => `scripts/${f}`),
    ...readdirSync(join(root, ".github/workflows")).filter((f) => /\.ya?ml$/.test(f)).map((f) => `.github/workflows/${f}`),
  ];
  assert.ok(files.length >= 3, "应当扫到脚本和工作流");
  const problems = files.flatMap((f) =>
    unbracedBeforeNonAscii(readFileSync(join(root, f), "utf8")).map((b) => `${f}:${b.line} ${b.vars.join(" ")}  → ${b.text}`)
  );
  assert.deepEqual(problems, [], `这些行在 macOS 自带的 bash 3.2 里会出错，请把变量写成 \${NAME}：\n${problems.join("\n")}`);
});
