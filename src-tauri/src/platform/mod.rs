//! 平台差异集中在这里。
//!
//! 业务代码（启动项目、端口、hosts、打开目录……）只调用这里的统一接口：
//! macOS / Linux 共用 `unix`，Windows 走 `windows`。
//! 监听端口的枚举在 Linux / Windows 上用跨平台的 `listeners` 库（见 `listen`），macOS 保持用 lsof。

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

#[cfg(not(target_os = "macos"))]
mod listen;

pub use imp::*;

use serde::Serialize;

/// 当前系统，前端据此调整文案和可用功能
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct PlatformInfo {
    /// "macos" | "windows" | "linux"
    pub os: &'static str,
    /// 是否支持「服务」页（目前只有 macOS 的 Homebrew services）
    pub services: bool,
}

pub const OS: &str = if cfg!(target_os = "macos") {
    "macos"
} else if cfg!(windows) {
    "windows"
} else {
    "linux"
};

/// 「服务」页依赖 Homebrew services，只有 macOS 有
pub const HAS_SERVICES: bool = cfg!(target_os = "macos");

#[tauri::command]
pub fn get_platform() -> PlatformInfo {
    PlatformInfo { os: OS, services: HAS_SERVICES }
}

/// 解码子进程输出的一行：合法 UTF-8 原样返回；否则按系统代码页解码
/// （中文 Windows 上很多程序输出的是 GBK），实在不行才用替换字符。
pub fn decode_line(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => decode_legacy(bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_matches_the_build_target() {
        let want = if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(windows) {
            "windows"
        } else {
            "linux"
        };
        assert_eq!(get_platform().os, want);
        assert_eq!(get_platform().services, cfg!(target_os = "macos"));
    }

    #[test]
    fn utf8_lines_are_kept_and_garbage_never_panics() {
        assert_eq!(decode_line("你好 world".as_bytes()), "你好 world");
        assert_eq!(decode_line(b""), "");
        // 非法字节：不 panic，非法位置不丢整行
        let s = decode_line(&[b'o', b'k', 0xff, 0xfe, b'!']);
        assert!(s.starts_with("ok") && s.ends_with('!'), "{s:?}");
    }
}
