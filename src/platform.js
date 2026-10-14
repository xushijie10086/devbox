// 当前系统的信息和与系统相关的文案。后端启动后通过 get_platform 告诉我们真实系统。
import { api } from "./api.js";

// 在读到真实系统之前先按 macOS（最早支持的平台）显示
let info = { os: "macos", services: true };

const FILE_MANAGER = { macos: "访达", windows: "资源管理器", linux: "文件管理器" };

/** 文件管理器的名字：访达 / 资源管理器 / 文件管理器 */
export const fileManagerName = (os = info.os) => FILE_MANAGER[os] ?? "文件管理器";

/** 快捷键修饰键的写法：macOS 是 ⌘，其它是 Ctrl+ */
export const modKeyLabel = (os = info.os) => (os === "macos" ? "⌘" : "Ctrl+");

/** 能检测到哪些来源的 Node / JDK，用在「没有检测到版本」的提示里 */
export function runtimeSources(kind, os = info.os) {
  const table = {
    node: {
      macos: "nvm / fnm / Volta / asdf / Homebrew",
      windows: "nvm-windows / fnm / Volta / Scoop / 官方安装包",
      linux: "nvm / fnm / Volta / asdf / n",
    },
    java: {
      macos: "系统 JVM / SDKMAN / asdf / Homebrew",
      windows: "Oracle / Adoptium / Microsoft / Zulu / Corretto 等安装包、Scoop、IntelliJ 下载的 JDK",
      linux: "系统 JVM（/usr/lib/jvm）/ SDKMAN / asdf / IntelliJ 下载的 JDK",
    },
  };
  return table[kind]?.[os] ?? "";
}

export const platform = () => info;

/** 启动时调用一次；读不到就保持默认，不影响使用 */
export async function loadPlatform() {
  try {
    info = await api.getPlatform();
  } catch (_) {}
  document.documentElement.dataset.os = info.os;
  return info;
}
