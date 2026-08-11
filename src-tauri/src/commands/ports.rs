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

/// 结束占用某端口的进程（按 pid，结束整棵进程树）
#[tauri::command]
pub fn kill_process(pid: u32) -> Result<(), String> {
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
    std::thread::sleep(std::time::Duration::from_millis(300));
    sys.refresh_processes();
    for p in &targets {
        if let Some(proc_) = sys.process(*p) {
            proc_.kill();
        }
    }

    if killed_any {
        Ok(())
    } else {
        Err(format!("找不到 pid {pid} 对应的进程"))
    }
}
