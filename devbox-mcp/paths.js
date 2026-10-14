// 各系统的配置目录，规则与 DevBox 后端（Rust 的 dirs::config_dir）保持一致。
// 单独放一个文件，没有任何副作用，方便测试。
import { homedir } from "node:os";
import { join } from "node:path";

export function configDir(platform = process.platform, env = process.env, home = homedir()) {
  if (platform === "darwin") return join(home, "Library", "Application Support");
  if (platform === "win32") return env.APPDATA || join(home, "AppData", "Roaming");
  return env.XDG_CONFIG_HOME || join(home, ".config");
}
