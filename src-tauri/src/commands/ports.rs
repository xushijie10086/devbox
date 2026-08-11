use crate::models::PortInfo;
use std::process::Command;
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, Signal, System};

/// 列出所有处于 LISTEN 状态的 TCP 端口占用
#[tauri::command]
pub fn list_ports() -> Result<Vec<PortInfo>, String> {
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
        result.push(PortInfo {
            port,
            pid,
            process,
            protocol,
            address,
        });
    }

    result.sort_by_key(|p| p.port);
    result.dedup_by_key(|p| (p.port, p.pid));
    Ok(result)
}

/// 结束占用某端口的进程。
/// 传入 lsof 报告的 pid，并尽量带上端口号：会先结束该 pid 的整棵进程树，
/// 再按端口反复核验，对任何仍在监听该端口的进程（含其进程树）用系统 kill -9 兜底，
/// 直到端口真正释放。这样能应对 `mvn spring-boot:run` 这类「父进程 fork 子 JVM」的场景。
#[tauri::command]
pub fn kill_process(pid: u32, port: Option<u16>) -> Result<(), String> {
    // 第一步：结束 lsof 报告的 pid 及其后代
    let killed_any = kill_pid_tree(pid);

    // 第二步：若已知端口，循环核验并清理仍占用端口的所有进程
    if let Some(port) = port {
        for _ in 0..12 {
            let holders = pids_on_port(port);
            if holders.is_empty() {
                return Ok(());
            }
            for holder in holders {
                kill_pid_tree(holder);
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        // 12 轮后仍未释放
        if pids_on_port(port).is_empty() {
            Ok(())
        } else {
            Err(format!("端口 {port} 仍被占用，可能进程权限不足或有守护进程在自动拉起"))
        }
    } else if killed_any {
        Ok(())
    } else {
        Err(format!("找不到 pid {pid} 对应的进程"))
    }
}

/// 查询当前正在监听指定端口的所有进程 pid
fn pids_on_port(port: u16) -> Vec<u32> {
    let iarg = format!("-iTCP:{port}");
    let output = Command::new("lsof")
        .args(["-nP", "-t", "-sTCP:LISTEN", iarg.as_str()])
        .output();
    match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(|l| l.trim().parse::<u32>().ok())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// 结束某 pid 及其所有后代进程：sysinfo 发 TERM→KILL，并用系统 kill -9 兜底。
/// 返回是否至少命中一个进程。
fn kill_pid_tree(pid: u32) -> bool {
    let mut sys = System::new_with_specifics(
        RefreshKind::new().with_processes(ProcessRefreshKind::new()),
    );
    sys.refresh_processes();

    // 收集进程树
    let root = Pid::from_u32(pid);
    let mut targets = vec![root];
    let mut frontier = vec![root];
    while let Some(parent) = frontier.pop() {
        for (p, proc_) in sys.processes() {
            if proc_.parent() == Some(parent) && !targets.contains(p) {
                targets.push(*p);
                frontier.push(*p);
            }
        }
    }

    let mut killed_any = false;
    for p in &targets {
        if let Some(proc_) = sys.process(*p) {
            proc_.kill_with(Signal::Term);
            killed_any = true;
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(250));
    sys.refresh_processes();
    for p in &targets {
        if let Some(proc_) = sys.process(*p) {
            proc_.kill();
        }
    }
    // 系统 kill -9 兜底（sysinfo 偶尔投递失败）
    for p in &targets {
        let pid_s = p.as_u32().to_string();
        let _ = Command::new("kill").args(["-9", pid_s.as_str()]).output();
    }
    killed_any
}
