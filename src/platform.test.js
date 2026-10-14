import test from "node:test";
import assert from "node:assert/strict";
import { fileManagerName, modKeyLabel, runtimeSources } from "./platform.js";

test("文件管理器的叫法随系统变化", () => {
  assert.equal(fileManagerName("macos"), "访达");
  assert.equal(fileManagerName("windows"), "资源管理器");
  assert.equal(fileManagerName("linux"), "文件管理器");
  assert.equal(fileManagerName("freebsd"), "文件管理器", "未知系统用通用叫法");
});

test("快捷键修饰键：macOS 是 ⌘，其它是 Ctrl+", () => {
  assert.equal(modKeyLabel("macos"), "⌘");
  assert.equal(modKeyLabel("windows"), "Ctrl+");
  assert.equal(modKeyLabel("linux"), "Ctrl+");
});

test("Node / JDK 的检测来源：每个系统都有说明，且不会把别的系统的工具写进去", () => {
  for (const os of ["macos", "windows", "linux"]) {
    assert.ok(runtimeSources("node", os), `node ${os}`);
    assert.ok(runtimeSources("java", os), `java ${os}`);
  }
  assert.ok(runtimeSources("node", "macos").includes("Homebrew"));
  assert.ok(!runtimeSources("node", "windows").includes("Homebrew"));
  assert.ok(runtimeSources("node", "windows").includes("nvm-windows"));
  assert.equal(runtimeSources("python", "macos"), "");
});
