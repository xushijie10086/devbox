use crate::commands::process::collect_descendants;
use crate::models::{PortInfo, Project};
use crate::state::AppState;
use std::process::Command;
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, Signal, System};
use tauri::State;

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
            project: None,
        });
    }

    result.sort_by_key(|p| p.port);
    result.dedup_by_key(|p| (p.port, p.pid));
    Ok(result)
}

/// 只列出与已登记项目相关的监听端口，并标注所属项目。
/// 相关的判定：监听进程属于某个运行中项目的进程树，或端口等于项目登记的期望端口。
#[tauri::command]
pub fn list_project_ports(state: State<AppState>) -> Result<Vec<PortInfo>, String> {
    let ports = list_ports()?;
    let projects = state.config.lock().unwrap().projects.clone();
    let roots: Vec<(String, u32)> = state
        .procs
        .lock()
        .unwrap()
        .iter()
        .map(|(id, rp)| (id.clone(), rp.pid))
        .collect();

    let mut sys = System::new_with_specifics(
        RefreshKind::new().with_processes(ProcessRefreshKind::new()),
    );
    sys.refresh_processes();
    let trees: Vec<(String, Vec<u32>)> = roots
        .into_iter()
        .map(|(id, root)| {
            let pids = collect_descendants(&sys, root).iter().map(|p| p.as_u32()).collect();
            (id, pids)
        })
        .collect();

    Ok(attribute_ports(ports, &projects, &trees))
}

/// 给端口标注所属项目并丢弃无关端口。进程树归属优先于端口号匹配。
/// trees 为 (项目 id, 该项目运行进程树内的全部 pid)。
fn attribute_ports(
    ports: Vec<PortInfo>,
    projects: &[Project],
    trees: &[(String, Vec<u32>)],
) -> Vec<PortInfo> {
    let name_of = |id: &str| projects.iter().find(|p| p.id == id).map(|p| p.name.clone());
    ports
        .into_iter()
        .filter_map(|mut info| {
            let owner = trees
                .iter()
                .find(|(_, pids)| pids.contains(&info.pid))
                .and_then(|(id, _)| name_of(id))
                .or_else(|| {
                    projects
                        .iter()
                        .find(|p| p.port == Some(info.port))
                        .map(|p| p.name.clone())
                });
            owner.map(|name| {
                info.project = Some(name);
                info
            })
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn project(id: &str, name: &str, port: Option<u16>) -> Project {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "path": "/tmp", "start_command": "x", "port": port
        }))
        .unwrap()
    }

    fn port(port: u16, pid: u32) -> PortInfo {
        PortInfo {
            port,
            pid,
            process: "node".into(),
            protocol: "TCP".into(),
            address: format!("*:{port}"),
            project: None,
        }
    }

    #[test]
    fn keeps_only_project_related_ports() {
        let projects = vec![project("a", "前端", Some(3000)), project("b", "后端", None)];
        // 后端运行中，进程树含 pid 200；5432 是无关的系统服务
        let trees = vec![("b".to_string(), vec![100, 200])];
        let out = attribute_ports(
            vec![port(3000, 1), port(8080, 200), port(5432, 9)],
            &projects,
            &trees,
        );
        let got: Vec<_> = out.iter().map(|p| (p.port, p.project.clone().unwrap())).collect();
        assert_eq!(got, vec![(3000, "前端".to_string()), (8080, "后端".to_string())]);
    }

    #[test]
    fn process_tree_wins_over_declared_port() {
        let projects = vec![project("a", "前端", Some(8080)), project("b", "后端", None)];
        let trees = vec![("b".to_string(), vec![200])];
        let out = attribute_ports(vec![port(8080, 200)], &projects, &trees);
        assert_eq!(out[0].project.as_deref(), Some("后端"));
    }
}
