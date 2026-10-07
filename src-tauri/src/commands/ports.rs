use crate::commands::process::collect_descendants;
use crate::models::{PortInfo, Project};
use serde::Serialize;
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
            group: None,
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
    let trees = running_trees(&state);

    Ok(attribute_ports(ports, &projects, &trees))
}

/// 运行中项目的进程树：(项目 id, 该项目进程树内的全部 pid)
fn running_trees(state: &AppState) -> Vec<(String, Vec<u32>)> {
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
    roots
        .into_iter()
        .map(|(id, root)| {
            let pids = collect_descendants(&sys, root).iter().map(|p| p.as_u32()).collect();
            (id, pids)
        })
        .collect()
}

/// 占用某个端口的一个进程
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct PortHolder {
    pub pid: u32,
    pub process: String,
    pub address: String,
    /// 若占用者属于某个运行中的 DevBox 项目：项目 id 与名称
    pub project_id: Option<String>,
    pub project: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct PortCheck {
    /// 项目登记的端口；没登记则为 None（无需检测）
    pub port: Option<u16>,
    /// 正在监听该端口的进程；空表示端口空闲
    pub holders: Vec<PortHolder>,
}

/// 谁在监听这个端口；`projects` 与 `trees` 用来识别占用者是不是某个运行中的项目
fn holders_of(port: u16, all: Vec<PortInfo>, projects: &[Project], trees: &[(String, Vec<u32>)]) -> Vec<PortHolder> {
    all.into_iter()
        .filter(|p| p.port == port)
        .map(|p| {
            let owner = trees.iter().find(|(_, pids)| pids.contains(&p.pid)).map(|(id, _)| id.clone());
            let name = owner.as_ref().and_then(|id| projects.iter().find(|x| &x.id == id)).map(|x| x.name.clone());
            PortHolder { pid: p.pid, process: p.process, address: p.address, project_id: owner, project: name }
        })
        .collect()
}

/// 一句话描述占用者，用在提示里：「node（PID 123）」/「项目「后端」」
pub fn describe_holders(holders: &[PortHolder]) -> String {
    holders
        .iter()
        .map(|h| match &h.project {
            Some(name) => format!("项目「{name}」"),
            None => format!("{}（PID {}）", h.process, h.pid),
        })
        .collect::<Vec<_>>()
        .join("、")
}

/// 检测某端口当前被谁占用（lsof 不可用时返回空，不阻塞启动）
pub fn who_holds(state: &AppState, port: u16) -> Vec<PortHolder> {
    let Ok(all) = list_ports() else { return vec![] };
    let projects = state.config.lock().unwrap().projects.clone();
    holders_of(port, all, &projects, &running_trees(state))
}

/// 启动前的端口预检：项目登记了端口、且已被占用时，说明是谁占的，让界面给出「结束占用并启动」
#[tauri::command(async)]
pub fn check_project_port(id: String, state: State<AppState>) -> Result<PortCheck, String> {
    let project = state
        .config
        .lock()
        .unwrap()
        .projects
        .iter()
        .find(|p| p.id == id)
        .cloned()
        .ok_or("找不到项目")?;
    let Some(port) = project.port else {
        return Ok(PortCheck { port: None, holders: vec![] });
    };
    // lsof 失败要让调用方知道（界面会忽略并照常启动），不能当成「端口空闲」
    let all = list_ports()?;
    let projects = state.config.lock().unwrap().projects.clone();
    Ok(PortCheck { port: Some(port), holders: holders_of(port, all, &projects, &running_trees(&state)) })
}

/// 给端口标注所属项目并丢弃无关端口。进程树归属优先于端口号匹配。
/// trees 为 (项目 id, 该项目运行进程树内的全部 pid)。
fn attribute_ports(
    ports: Vec<PortInfo>,
    projects: &[Project],
    trees: &[(String, Vec<u32>)],
) -> Vec<PortInfo> {
    let by_id = |id: &str| projects.iter().find(|p| p.id == id);
    ports
        .into_iter()
        .filter_map(|mut info| {
            let owner = trees
                .iter()
                .find(|(_, pids)| pids.contains(&info.pid))
                .and_then(|(id, _)| by_id(id))
                .or_else(|| projects.iter().find(|p| p.port == Some(info.port)));
            owner.map(|p| {
                info.project = Some(p.name.clone());
                info.group = p.group.clone();
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
            group: None,
        }
    }

    #[test]
    fn ports_carry_the_group_of_their_project() {
        let mut a = project("a", "前端", Some(3000));
        a.group = Some("站群".into());
        let b = project("b", "后端", Some(8080)); // 未分组
        let out = attribute_ports(vec![port(3000, 1), port(8080, 2)], &[a, b], &[]);
        assert_eq!(
            out.iter().map(|p| (p.port, p.group.as_deref())).collect::<Vec<_>>(),
            [(3000, Some("站群")), (8080, None)]
        );
        // 进程树归属：端口号不同也跟着所属项目走
        let mut c = project("c", "网关", None);
        c.group = Some("网关组".into());
        let out = attribute_ports(vec![port(9000, 7)], &[c], &[("c".into(), vec![7])]);
        assert_eq!((out[0].project.as_deref(), out[0].group.as_deref()), (Some("网关"), Some("网关组")));
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

    // ---------- 端口预检 ----------

    fn info(port: u16, pid: u32, process: &str) -> PortInfo {
        PortInfo { port, pid, process: process.into(), protocol: "TCP".into(), address: format!("*:{port}"), project: None, group: None }
    }

    #[test]
    fn holders_are_attributed_to_running_projects_or_left_external() {
        let projects = vec![project("a", "后端", Some(8080))];
        let trees = vec![("a".to_string(), vec![100, 200])];
        let all = vec![info(8080, 200, "java"), info(8080, 999, "nginx"), info(3000, 5, "node")];
        let h = holders_of(8080, all, &projects, &trees);
        assert_eq!(h.len(), 2, "只取该端口的监听者");
        assert_eq!(h[0].project.as_deref(), Some("后端"));
        assert_eq!(h[0].project_id.as_deref(), Some("a"));
        assert_eq!((h[1].process.as_str(), h[1].project.as_deref()), ("nginx", None));
        assert_eq!(describe_holders(&h), "项目「后端」、nginx（PID 999）");
        assert!(holders_of(9999, vec![info(8080, 1, "x")], &projects, &trees).is_empty(), "空闲端口");
    }

    fn listen_in_child(port: u16) -> std::process::Child {
        let c = Command::new("python3")
            .args(["-m", "http.server", &port.to_string(), "--bind", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        for _ in 0..60 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return c;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("测试用的监听进程没起来");
    }

    #[test]
    fn external_listener_is_found_by_lsof() {
        if Command::new("python3").arg("--version").output().is_err() || Command::new("lsof").arg("-v").output().is_err() {
            return;
        }
        let mut child = listen_in_child(31976);
        let st = AppState::for_test(crate::models::Config::default());
        let h = who_holds(&st, 31976);
        assert_eq!(h.len(), 1, "{h:?}");
        assert_eq!(h[0].pid, child.id());
        assert!(h[0].process.starts_with("python"), "{}", h[0].process);
        assert!(h[0].project.is_none(), "不是 DevBox 项目，应视为外部进程");
        assert!(who_holds(&st, 31979).is_empty(), "空闲端口");
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn listener_started_by_a_devbox_project_is_attributed_to_it() {
        if Command::new("python3").arg("--version").output().is_err() || Command::new("lsof").arg("-v").output().is_err() {
            return;
        }
        let st = AppState::for_test(crate::models::Config {
            projects: vec![serde_json::from_value(serde_json::json!({
                "id": "db", "name": "数据库", "path": "/tmp", "port": 31977,
                "start_command": "exec python3 -m http.server 31977 --bind 127.0.0.1"
            }))
            .unwrap()],
            ..Default::default()
        });
        crate::commands::process::start_with_deps_with("db", &st, std::time::Duration::from_millis(300), std::time::Duration::from_secs(8)).unwrap();
        let h = who_holds(&st, 31977);
        assert_eq!(h.len(), 1, "{h:?}");
        assert_eq!((h[0].project_id.as_deref(), h[0].project.as_deref()), (Some("db"), Some("数据库")));
        crate::commands::process::stop_all(&st);
    }

    #[test]
    fn start_warning_names_who_already_holds_the_port() {
        if Command::new("python3").arg("--version").output().is_err() || Command::new("lsof").arg("-v").output().is_err() {
            return;
        }
        let mut squatter = listen_in_child(31978);
        let st = AppState::for_test(crate::models::Config {
            projects: vec![serde_json::from_value(serde_json::json!({
                "id": "p", "name": "后端", "path": "/tmp", "port": 31978, "start_command": "sleep 30"
            }))
            .unwrap()],
            ..Default::default()
        });
        let out = crate::commands::process::start_and_verify_with("p", &st, std::time::Duration::from_millis(300), std::time::Duration::from_secs(2)).unwrap();
        assert_eq!(out.level, "warning");
        assert!(out.message.contains("端口 31978") && out.message.contains("python") && out.message.contains("占用"), "{}", out.message);
        crate::commands::process::stop_all(&st);
        let _ = squatter.kill();
        let _ = squatter.wait();
    }
}
