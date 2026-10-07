// 守卫：Rust 的 tauri crate 与前端 @tauri-apps/api 必须是同一个「主.次」版本，
// 否则 `tauri dev` 启动就会报 "version mismatched Tauri packages"。
// 任何一边升级（含 cargo 因为新增依赖而顺带升级 tauri）导致漂移时，这里会直接失败并告诉你怎么改。
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

/** 从 Cargo.lock 文本里取某个 crate 的版本 */
export function crateVersion(lockText, name) {
  const m = lockText.match(new RegExp(`\\[\\[package\\]\\]\\nname = "${name}"\\nversion = "([^"]+)"`));
  return m ? m[1] : null;
}

const majorMinor = (v) => v.split(".").slice(0, 2).join(".");

test("解析 Cargo.lock 里的版本（含名字相近的 crate 不会串）", () => {
  const lock = '[[package]]\nname = "tauri-build"\nversion = "2.7.1"\n\n[[package]]\nname = "tauri"\nversion = "2.12.1"\n';
  assert.equal(crateVersion(lock, "tauri"), "2.12.1");
  assert.equal(crateVersion(lock, "tauri-build"), "2.7.1");
  assert.equal(crateVersion(lock, "nope"), null);
});

test("tauri crate 与 @tauri-apps/api、@tauri-apps/cli 的主.次版本一致", () => {
  const rust = crateVersion(readFileSync(join(root, "src-tauri/Cargo.lock"), "utf8"), "tauri");
  const npmLock = JSON.parse(readFileSync(join(root, "package-lock.json"), "utf8")).packages;
  const api = npmLock["node_modules/@tauri-apps/api"].version;
  const cli = npmLock["node_modules/@tauri-apps/cli"].version;
  const hint = `Rust tauri ${rust} / @tauri-apps/api ${api} / @tauri-apps/cli ${cli}。` +
    `把前端对齐到 Rust：npm install @tauri-apps/api@~${majorMinor(rust)} @tauri-apps/cli@~${majorMinor(rust)}`;
  assert.ok(rust, "Cargo.lock 里找不到 tauri");
  assert.equal(majorMinor(api), majorMinor(rust), `@tauri-apps/api 与 tauri crate 版本不一致：${hint}`);
  assert.equal(majorMinor(cli), majorMinor(rust), `@tauri-apps/cli 与 tauri crate 版本不一致：${hint}`);
});
