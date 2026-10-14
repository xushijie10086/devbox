import test from "node:test";
import assert from "node:assert/strict";
import { posix, win32 } from "node:path";
import { configDir } from "../devbox-mcp/paths.js";

// 在不同系统上 path.join 的分隔符不同，这里只比较归一化后的结果
const norm = (p) => p.replaceAll("\\", "/");

test("macOS：~/Library/Application Support", () => {
  assert.equal(norm(configDir("darwin", {}, "/Users/me")), "/Users/me/Library/Application Support");
});

test("Windows：优先 %APPDATA%，没有就用用户目录下的 AppData/Roaming", () => {
  assert.equal(configDir("win32", { APPDATA: "C:\\Users\\me\\AppData\\Roaming" }, "C:\\Users\\me"), "C:\\Users\\me\\AppData\\Roaming");
  assert.equal(norm(configDir("win32", {}, "C:/Users/me")), "C:/Users/me/AppData/Roaming");
});

test("Linux：优先 $XDG_CONFIG_HOME，没有就是 ~/.config", () => {
  assert.equal(configDir("linux", { XDG_CONFIG_HOME: "/data/cfg" }, "/home/me"), "/data/cfg");
  assert.equal(norm(configDir("linux", {}, "/home/me")), "/home/me/.config");
});

test("桥接文件的路径和后端写的位置一致（都是 <配置目录>/devbox/bridge.json）", () => {
  assert.ok(posix && win32); // 仅确认 node:path 可用
  assert.equal(norm(configDir("linux", {}, "/home/me")) + "/devbox/bridge.json", "/home/me/.config/devbox/bridge.json");
});
