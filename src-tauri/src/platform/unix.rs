//! macOS / Linux 的平台实现。macOS 专有的部分（lsof、open、osascript）放在 `cfg(target_os = "macos")` 里，
//! Linux 用对应的 xdg-open / pkexec 等。

use std::path::PathBuf;
use std::process::Command;

// ---------- 执行命令 ----------

/// 用登录 shell 执行一段脚本：GUI 应用启动时 PATH 很短，登录 shell 会读用户的 profile 补全
pub fn shell(script: &str) -> Command {
    let mut c = Command::new("/bin/sh");
    c.arg("-lc").arg(script);
    c
}

/// 可执行文件名（Windows 上要带 .exe）
pub fn exe(name: &str) -> String {
    name.to_string()
}

pub(crate) fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// 拼在命令前面的环境前缀：把 `path_front` 放到 PATH 最前，并设置 `vars`。
/// 必须写在命令里而不是设成进程环境变量：登录 shell 读 /etc/profile 时会重排 PATH，
/// 写在命令前缀里才能保证在它之后生效。没有要设置的就返回空串。
pub fn env_prelude(path_front: &[String], vars: &[(&str, &str)]) -> String {
    if path_front.is_empty() && vars.is_empty() {
        return String::new();
    }
    let mut s = format!("export PATH={}:\"$PATH\"; ", sh_quote(&path_front.join(":")));
    for (k, v) in vars {
        s.push_str(&format!("export {k}={}; ", sh_quote(v)));
    }
    s
}

/// 强制结束一个进程（sysinfo 偶尔投递失败时的兜底）
pub fn force_kill(pid: u32) {
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
}

/// 进程是被哪个信号杀死的（Unix 才有这个概念）
pub fn exit_signal(status: &std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

/// 非 UTF-8 的输出：Unix 上没有「系统代码页」一说，用替换字符
pub(super) fn decode_legacy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

// ---------- 构建工具 / Python 的平台差异 ----------

/// 系统里默认的 Python 命令
pub const PYTHON: &str = "python3";

/// 项目里的构建包装脚本（mvnw / gradlew）：存在就返回要在命令里写的形式，如 `./mvnw`
pub fn wrapper(dir: &std::path::Path, name: &str) -> Option<String> {
    dir.join(name).exists().then(|| format!("./{name}"))
}

/// 项目自己的 Python 虚拟环境里的解释器（相对项目目录）
pub fn venv_python(dir: &std::path::Path) -> Option<String> {
    [".venv", "venv"].iter().find(|v| dir.join(v).join("bin/python").exists()).map(|v| format!("{v}/bin/python"))
}

// ---------- 监听端口 ----------

#[cfg(target_os = "macos")]
pub fn listening_ports() -> Result<Vec<crate::models::PortInfo>, String> {
    use crate::models::PortInfo;
    let output = Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN"])
        .output()
        .map_err(|e| format!("执行 lsof 失败: {e}"))?;

    let text = String::from_utf8_lossy(&output.stdout);
    let mut result = Vec::new();

    for line in text.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 9 {
            continue;
        }
        let process = parts[0].to_string();
        let pid: u32 = match parts[1].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let protocol = parts[7].to_string();
        let address = parts[8].to_string();
        let port = match address.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()) {
            Some(p) => p,
            None => continue,
        };
        result.push(PortInfo { port, pid, process, protocol, address, project: None, group: None });
    }

    result.sort_by_key(|p| p.port);
    result.dedup_by_key(|p| (p.port, p.pid));
    Ok(result)
}

#[cfg(not(target_os = "macos"))]
pub fn listening_ports() -> Result<Vec<crate::models::PortInfo>, String> {
    super::listen::list()
}

/// 查询当前正在监听指定端口的所有进程 pid
#[cfg(target_os = "macos")]
pub fn pids_on_port(port: u16) -> Vec<u32> {
    let iarg = format!("-iTCP:{port}");
    let output = Command::new("lsof").args(["-nP", "-t", "-sTCP:LISTEN", iarg.as_str()]).output();
    match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| l.trim().parse::<u32>().ok()).collect(),
        Err(_) => Vec::new(),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn pids_on_port(port: u16) -> Vec<u32> {
    super::listen::pids_on_port(port)
}

// ---------- 打开 / 定位 ----------

/// 用编辑器打开项目目录（默认 VSCode: `code`）；走登录 shell，确保能取到 code / cursor / idea 等命令的 PATH
pub fn open_in_editor(editor: &str, path: &str) -> Result<(), String> {
    shell(&format!("{editor} {}", sh_quote(path)))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打开编辑器失败: {e}"))
}

/// 在默认浏览器打开地址
pub fn open_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("open");
    #[cfg(not(target_os = "macos"))]
    let mut cmd = Command::new("xdg-open");
    cmd.arg(url).spawn().map(|_| ()).map_err(|e| format!("打开浏览器失败: {e}"))
}

/// 在终端中打开目录
#[cfg(target_os = "macos")]
pub fn open_terminal(path: &str) -> Result<(), String> {
    Command::new("open")
        .args(["-a", "Terminal"])
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打开终端失败: {e}"))
}

/// Linux 的终端五花八门：依次尝试用户指定的 $TERMINAL 和常见终端，
/// 它们不带参数启动时都会在调用者的工作目录里打开，所以只要设置 current_dir
#[cfg(not(target_os = "macos"))]
pub fn open_terminal(path: &str) -> Result<(), String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(t) = std::env::var_os("TERMINAL").map(|t| t.to_string_lossy().into_owned()).filter(|t| !t.trim().is_empty()) {
        candidates.push(t);
    }
    candidates.extend(
        ["x-terminal-emulator", "gnome-terminal", "konsole", "xfce4-terminal", "tilix", "kitty", "alacritty", "wezterm", "xterm"]
            .iter()
            .map(|s| s.to_string()),
    );
    for t in &candidates {
        if Command::new(t).current_dir(path).spawn().is_ok() {
            return Ok(());
        }
    }
    Err("没有找到可用的终端程序（可以设置环境变量 TERMINAL 指定）".into())
}

/// 在文件管理器中显示：目录就打开它；文件则尽量定位并选中该文件
#[cfg(target_os = "macos")]
pub fn reveal(path: &str) -> Result<(), String> {
    let mut cmd = Command::new("open");
    if std::path::Path::new(path).is_file() {
        cmd.arg("-R");
    }
    cmd.arg(path).spawn().map(|_| ()).map_err(|e| format!("打开访达失败: {e}"))
}

#[cfg(not(target_os = "macos"))]
pub fn reveal(path: &str) -> Result<(), String> {
    let p = std::path::Path::new(path);
    if p.is_file() {
        // 标准的 FileManager1 接口能选中文件；没有（或失败）就退回打开它所在的目录
        let uri = format!("file://{}", percent_encode(path));
        let selected = Command::new("dbus-send")
            .args([
                "--session",
                "--print-reply",
                "--dest=org.freedesktop.FileManager1",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
            ])
            .arg(format!("array:string:{uri}"))
            .arg("string:")
            .output()
            .is_ok_and(|o| o.status.success());
        if selected {
            return Ok(());
        }
        let dir = p.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_else(|| ".".into());
        return Command::new("xdg-open").arg(dir).spawn().map(|_| ()).map_err(|e| format!("打开文件管理器失败: {e}"));
    }
    Command::new("xdg-open").arg(path).spawn().map(|_| ()).map_err(|e| format!("打开文件管理器失败: {e}"))
}

/// 文件路径转成 URI 里能用的形式（只保留非保留字符和 `/`）
#[cfg(not(target_os = "macos"))]
fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ---------- hosts ----------

pub fn hosts_path() -> PathBuf {
    PathBuf::from("/etc/hosts")
}

/// 以管理员权限把 `content` 写入 hosts，并刷新 DNS 缓存。用户取消授权会返回错误。
#[cfg(target_os = "macos")]
pub fn write_hosts_elevated(content: &str) -> Result<(), String> {
    let tmp = std::env::temp_dir().join("devbox_hosts.tmp");
    std::fs::write(&tmp, content).map_err(|e| e.to_string())?;

    let shell = format!(
        "cp '{}' '{}' && dscacheutil -flushcache && killall -HUP mDNSResponder",
        tmp.display(),
        hosts_path().display()
    );
    let script = format!("do shell script \"{}\" with administrator privileges", shell);

    let status = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .status()
        .map_err(|e| format!("调用 osascript 失败: {e}"))?;

    let _ = std::fs::remove_file(&tmp);

    if status.success() {
        Ok(())
    } else {
        Err("写入 /etc/hosts 被取消或失败（需要管理员授权）".into())
    }
}

/// Linux 用 pkexec（polkit）弹出授权框；刷新 DNS 缓存是尽力而为，失败不算写入失败
#[cfg(not(target_os = "macos"))]
pub fn write_hosts_elevated(content: &str) -> Result<(), String> {
    let tmp = std::env::temp_dir().join("devbox_hosts.tmp");
    std::fs::write(&tmp, content).map_err(|e| e.to_string())?;

    let status = Command::new("pkexec")
        .args(["sh", "-c", "cp \"$1\" \"$2\" && { resolvectl flush-caches 2>/dev/null || true; }", "sh"])
        .arg(&tmp)
        .arg(hosts_path())
        .status();
    let _ = std::fs::remove_file(&tmp);

    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => Err("写入 /etc/hosts 被取消或失败（需要管理员授权）".into()),
        Err(e) => Err(format!("调用 pkexec 失败: {e}（需要安装 polkit，或手动编辑 /etc/hosts）")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prelude_has_the_exact_shape_runtime_tests_rely_on() {
        assert_eq!(env_prelude(&[], &[]), "");
        let p = env_prelude(&["/a b".into(), "/c".into()], &[("JAVA_HOME", "/j'h")]);
        assert_eq!(p, "export PATH='/a b:/c':\"$PATH\"; export JAVA_HOME='/j'\\''h'; ");
    }

    #[test]
    fn shell_runs_a_script_and_exe_is_unchanged() {
        let out = shell("echo hi && exit 3").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");
        assert_eq!(out.status.code(), Some(3));
        assert_eq!(exe("node"), "node");
    }

    #[test]
    fn force_kill_ends_a_process() {
        let mut c = Command::new("sleep").arg("30").spawn().unwrap();
        force_kill(c.id());
        let st = c.wait().unwrap();
        assert!(!st.success());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn uris_are_percent_encoded() {
        assert_eq!(percent_encode("/home/u/我 的/a.log"), "/home/u/%E6%88%91%20%E7%9A%84/a.log");
        assert_eq!(percent_encode("/tmp/x-y_z.1~"), "/tmp/x-y_z.1~");
    }
}
