//! Windows 的平台实现。
//!
//! 命令用 `cmd /S /C "…"` 执行（不是 PowerShell：npm / mvn / code 这些 `.cmd` 垫片在两者里都能跑，
//! 而 cmd 的语法和用户平时写的启动命令一致）。所有子进程都带 CREATE_NO_WINDOW，
//! 免得从没有控制台的 GUI 程序里启动时闪出黑窗。

use std::ffi::OsStr;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn hidden(program: impl AsRef<OsStr>) -> Command {
    let mut c = Command::new(program);
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

fn comspec() -> std::ffi::OsString {
    std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into())
}

// ---------- 执行命令 ----------

/// 用 cmd 执行一段脚本。
/// `/S` 让 cmd 只去掉最外层的一对引号，这样脚本里自己带引号（如路径带空格）也不会被吃掉；
/// 参数必须用 raw_arg 原样传入，否则 Rust 会再转义一遍引号。
pub fn shell(script: &str) -> Command {
    let mut c = hidden(comspec());
    c.raw_arg(format!("/S /C \"{script}\""));
    c
}

/// 可执行文件名：Windows 上带 .exe
pub fn exe(name: &str) -> String {
    format!("{name}.exe")
}

/// 拼在命令前面的环境前缀：把 `path_front` 放到 PATH 最前，并设置 `vars`。没有要设置的就返回空串。
pub fn env_prelude(path_front: &[String], vars: &[(&str, &str)]) -> String {
    let mut s = String::new();
    if !path_front.is_empty() {
        s.push_str(&format!("set \"PATH={};%PATH%\" && ", path_front.join(";")));
    }
    for (k, v) in vars {
        s.push_str(&format!("set \"{k}={v}\" && "));
    }
    s
}

/// 强制结束一个进程及其子进程树
pub fn force_kill(pid: u32) {
    let _ = hidden("taskkill").args(["/F", "/T", "/PID", &pid.to_string()]).output();
}

/// Windows 没有信号，进程只有退出码
pub fn exit_signal(_status: &std::process::ExitStatus) -> Option<i32> {
    None
}

/// 非 UTF-8 的输出按系统「ANSI 代码页」解码（中文 Windows 是 936/GBK）
pub(super) fn decode_legacy(bytes: &[u8]) -> String {
    // SAFETY: GetACP 没有参数，只返回当前进程的 ANSI 代码页
    let cp = unsafe { windows_sys::Win32::Globalization::GetACP() };
    decode_with_codepage(bytes, cp)
}

fn decode_with_codepage(bytes: &[u8], cp: u32) -> String {
    let enc = match cp {
        936 | 54936 => Some(encoding_rs::GBK),
        950 => Some(encoding_rs::BIG5),
        932 => Some(encoding_rs::SHIFT_JIS),
        949 => Some(encoding_rs::EUC_KR),
        1250..=1258 => encoding_rs::Encoding::for_label(format!("windows-{cp}").as_bytes()),
        _ => None,
    };
    match enc {
        Some(e) => e.decode(bytes).0.into_owned(),
        None => String::from_utf8_lossy(bytes).into_owned(),
    }
}

// ---------- 构建工具 / Python 的平台差异 ----------

/// 系统里默认的 Python 命令（Windows 上通常没有 python3）
pub const PYTHON: &str = "python";

/// 项目里的构建包装脚本：Windows 上是 mvnw.cmd / gradlew.bat，cmd 里直接写文件名即可
pub fn wrapper(dir: &Path, name: &str) -> Option<String> {
    ["cmd", "bat"].iter().map(|ext| format!("{name}.{ext}")).find(|f| dir.join(f).exists())
}

/// 项目自己的 Python 虚拟环境里的解释器（相对项目目录）
pub fn venv_python(dir: &Path) -> Option<String> {
    [".venv", "venv"]
        .iter()
        .find(|v| dir.join(v).join("Scripts").join("python.exe").exists())
        .map(|v| format!("{v}\\Scripts\\python.exe"))
}

// ---------- 监听端口 ----------

pub fn listening_ports() -> Result<Vec<crate::models::PortInfo>, String> {
    super::listen::list()
}

pub fn pids_on_port(port: u16) -> Vec<u32> {
    super::listen::pids_on_port(port)
}

// ---------- 打开 / 定位 ----------

/// 用编辑器打开项目目录；经 cmd 执行，这样 `code` 这类 `.cmd` 垫片能被找到
pub fn open_in_editor(editor: &str, path: &str) -> Result<(), String> {
    shell(&format!("{editor} \"{path}\""))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打开编辑器失败: {e}"))
}

/// 在默认浏览器打开地址（不用 `start`：URL 里的 & 会被 cmd 当成命令分隔符）
pub fn open_url(url: &str) -> Result<(), String> {
    hidden("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打开浏览器失败: {e}"))
}

/// 在终端中打开目录：优先 Windows Terminal，没有就开一个 cmd 窗口
pub fn open_terminal(path: &str) -> Result<(), String> {
    if Command::new("wt").arg("-d").arg(path).spawn().is_ok() {
        return Ok(());
    }
    let mut c = hidden(comspec());
    // start 会给 cmd.exe 开一个新的控制台窗口
    c.raw_arg(format!("/S /C \"start \"DevBox\" /D \"{path}\" cmd.exe\""));
    c.spawn().map(|_| ()).map_err(|e| format!("打开终端失败: {e}"))
}

/// 在资源管理器中显示：目录就打开它；文件则选中它
pub fn reveal(path: &str) -> Result<(), String> {
    // explorer 不认正斜杠
    let p = path.replace('/', "\\");
    let mut c = Command::new("explorer");
    if Path::new(&p).is_file() {
        c.raw_arg(format!("/select,\"{p}\""));
    } else {
        c.raw_arg(format!("\"{p}\""));
    }
    c.spawn().map(|_| ()).map_err(|e| format!("打开资源管理器失败: {e}"))
}

// ---------- hosts ----------

pub fn hosts_path() -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    PathBuf::from(root).join("System32").join("drivers").join("etc").join("hosts")
}

/// 以管理员权限（UAC 弹窗）把 `content` 写入 hosts 并刷新 DNS 缓存。用户取消授权会返回错误。
/// 做法：把「复制 + 刷新」写成一个临时的 .cmd，再让 PowerShell 以 RunAs 运行它并等待、取回退出码。
pub fn write_hosts_elevated(content: &str) -> Result<(), String> {
    let dir = std::env::temp_dir();
    let tmp = dir.join("devbox_hosts.tmp");
    let script = dir.join("devbox_hosts.cmd");
    std::fs::write(&tmp, content).map_err(|e| e.to_string())?;
    let body = format!(
        "@echo off\r\ncopy /Y \"{}\" \"{}\" >nul\r\nif errorlevel 1 exit /b 1\r\nipconfig /flushdns >nul\r\nexit /b 0\r\n",
        tmp.display(),
        hosts_path().display()
    );
    std::fs::write(&script, body).map_err(|e| e.to_string())?;

    let ps = format!(
        "$p = Start-Process -FilePath '{}' -Verb RunAs -Wait -PassThru -WindowStyle Hidden; exit $p.ExitCode",
        script.display().to_string().replace('\'', "''")
    );
    let status = hidden("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &ps]).status();

    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_file(&script);

    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => Err("写入 hosts 被取消或失败（需要管理员授权）".into()),
        Err(e) => Err(format!("调用 PowerShell 失败: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(script: &str) -> (String, Option<i32>) {
        let out = shell(script).output().unwrap();
        (String::from_utf8_lossy(&out.stdout).trim().to_string(), out.status.code())
    }

    #[test]
    fn shell_runs_a_script_and_keeps_quotes_and_exit_code() {
        assert_eq!(run("echo hi && exit 3"), ("hi".into(), Some(3)));
        // 脚本里自带引号（路径带空格）不会被吃掉
        let dir = std::env::temp_dir().join("devbox win 空格");
        std::fs::create_dir_all(&dir).unwrap();
        let (out, code) = run(&format!("cd /d \"{}\" && cd", dir.display()));
        assert_eq!(code, Some(0), "{out}");
        // cd 的输出走控制台代码页，中文在 CI 的英文系统上显示不出来，只核对前半段
        assert!(out.to_lowercase().contains("devbox win "), "{out}");
    }

    #[test]
    fn exe_adds_the_extension() {
        assert_eq!(exe("node"), "node.exe");
    }

    #[test]
    fn prelude_has_the_expected_shape_and_really_takes_effect() {
        assert_eq!(env_prelude(&[], &[]), "");
        let p = env_prelude(&["C:\\x y".into(), "D:\\z".into()], &[("JAVA_HOME", "C:\\j h")]);
        assert_eq!(p, "set \"PATH=C:\\x y;D:\\z;%PATH%\" && set \"JAVA_HOME=C:\\j h\" && ");
        // 真的在 cmd 里跑一遍：set 变量名显示出当前值（前缀匹配，会连带列出 JAVA_HOME_17_X64 之类，逐行核对）
        let (out, code) = run(&format!("{p}set JAVA_HOME"));
        assert_eq!(code, Some(0), "{out}");
        assert!(out.lines().any(|l| l == "JAVA_HOME=C:\\j h"), "{out}");
        let (out, _) = run(&format!("{p}set PATH"));
        assert!(out.to_lowercase().starts_with("path=c:\\x y;d:\\z;"), "{out}");
    }

    #[test]
    fn force_kill_ends_the_whole_tree() {
        // cmd → ping：杀掉 cmd 这一层，整棵树都要没
        let mut c = shell("ping -n 60 127.0.0.1 >nul").spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        force_kill(c.id());
        let st = c.wait().unwrap();
        assert!(!st.success());
    }

    #[test]
    fn legacy_output_is_decoded_with_the_code_page() {
        // "你好" 的 GBK 编码
        assert_eq!(decode_with_codepage(&[0xC4, 0xE3, 0xBA, 0xC3], 936), "你好");
        assert_eq!(decode_with_codepage(&[0x63, 0x61, 0x66, 0xE9], 1252), "café");
        // 不认识的代码页：不 panic
        assert!(!decode_with_codepage(&[0xff], 437).is_empty());
    }

    #[test]
    fn hosts_file_lives_under_system32() {
        let p = hosts_path().to_string_lossy().to_lowercase();
        assert!(p.ends_with("system32\\drivers\\etc\\hosts"), "{p}");
    }
}
