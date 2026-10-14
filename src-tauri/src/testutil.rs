//! 测试辅助：用测试程序自己当「监听某个端口」的子进程。
//!
//! 以前用 `python3 -m http.server`，结果依赖运行机器上的 python3：macOS 的 CI 上经登录 shell
//! 解析到的 python3 迟迟不监听端口，测试就跟着不稳。换成测试程序自己，行为在任何机器上都一样。

use std::process::{Child, Command, Stdio};
use std::time::Duration;

const ENV: &str = "DEVBOX_TEST_LISTEN_PORT";

/// 子进程的入口：被 [`listen_command`] / [`spawn_listener`] 以 `--exact` 重新执行时，
/// 在环境变量指定的端口上监听并一直挂着；平时（没有环境变量）什么也不做，直接通过。
#[test]
fn listener_helper() {
    let Ok(port) = std::env::var(ENV) else { return };
    let listener = std::net::TcpListener::bind(("127.0.0.1", port.parse().unwrap())).unwrap();
    for _ in listener.incoming() {}
}

/// 放进项目启动命令里的一行 shell：监听 127.0.0.1:port，行为像一个正常的服务
#[cfg(unix)]
pub fn listen_command(port: u16) -> String {
    let exe = std::env::current_exe().unwrap();
    format!("{ENV}={port} exec '{}' --exact testutil::listener_helper --test-threads=1", exe.display())
}

/// Windows 版：cmd 里没有 exec，设好环境变量后直接运行（cmd 会留在父进程位置，结束进程树时一并结束）
#[cfg(windows)]
pub fn listen_command(port: u16) -> String {
    let exe = std::env::current_exe().unwrap();
    format!("set \"{ENV}={port}\" && \"{}\" --exact testutil::listener_helper --test-threads=1", exe.display())
}

/// 一条睡 `secs` 秒、不占 CPU 的命令
#[cfg(unix)]
pub fn sleep_command(secs: u32) -> String {
    format!("sleep {secs}")
}

/// Windows 没有 sleep，用 ping 自己（每次间隔 1 秒）计时
#[cfg(windows)]
pub fn sleep_command(secs: u32) -> String {
    format!("ping -n {} 127.0.0.1 >nul", secs + 1)
}

/// 一个确实存在的目录，当项目工作目录用（Unix 的 /tmp 在 Windows 上并不存在）
pub fn workdir() -> String {
    std::env::temp_dir().to_string_lossy().into_owned()
}

/// 直接起一个监听 127.0.0.1:port 的外部进程（不属于任何 DevBox 项目），等它真的在监听后返回
pub fn spawn_listener(port: u16) -> Child {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "testutil::listener_helper", "--test-threads=1"])
        .env(ENV, port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..60 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return child;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait(); // 起不来也别留下僵尸进程
    panic!("测试用的监听进程没起来（端口 {port}）");
}
