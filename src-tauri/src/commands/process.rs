use crate::models::{LogLine, Project, ProjectStatus};
use crate::state::{AppState, RunningProc, MAX_LOG_LINES};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::net::{SocketAddr, TcpStream};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, Signal, System};
use tauri::State;

// ---------- 对外命令 ----------

/// 启动一个项目
#[tauri::command]
pub fn start_project(id: String, state: State<AppState>) -> Result<(), String> {
    start_project_inner(&id, state.inner())
}

/// 停止一个项目
#[tauri::command]
pub fn stop_project(id: String, state: State<AppState>) -> Result<(), String> {
    stop_project_inner(&id, state.inner())
}

/// 重启一个项目
#[tauri::command]
pub fn restart_project(id: String, state: State<AppState>) -> Result<(), String> {
    let _ = stop_project_inner(&id, state.inner());
    std::thread::sleep(Duration::from_millis(600));
    start_project_inner(&id, state.inner())
}

/// 获取所有项目的运行时状态（含 CPU / 内存 / 端口探测）
#[tauri::command]
pub fn project_statuses(state: State<AppState>) -> Vec<ProjectStatus> {
    compute_statuses(state.inner())
}

/// 计算所有项目的运行时状态（供前端命令与本地 HTTP 服务共用）
pub fn compute_statuses(state: &AppState) -> Vec<ProjectStatus> {
    reap_dead(state);

    let projects = state.config.lock().unwrap().projects.clone();
    let procs = state.procs.lock().unwrap();

    // 只在有运行进程时才做一次系统刷新，避免不必要开销
    let mut sys = System::new_with_specifics(
        RefreshKind::new().with_processes(ProcessRefreshKind::everything()),
    );
    sys.refresh_processes();

    projects
        .iter()
        .map(|p| {
            let running_proc = procs.get(&p.id);
            let running = running_proc.is_some();
            let pid = running_proc.map(|rp| rp.pid);
            let uptime = running_proc.map(|rp| rp.started_at.elapsed().as_secs());

            let (cpu, mem) = match pid {
                Some(pid) => match sys.process(Pid::from_u32(pid)) {
                    Some(proc_) => (
                        Some(proc_.cpu_usage()),
                        Some(proc_.memory() / 1024 / 1024),
                    ),
                    None => (None, None),
                },
                None => (None, None),
            };

            let port_up = p.port.map(|port| port_is_listening(port));

            ProjectStatus {
                id: p.id.clone(),
                running,
                pid,
                cpu,
                memory_mb: mem,
                uptime_secs: uptime,
                port_up,
            }
        })
        .collect()
}

/// 健康检查心跳：由前端定时调用，负责重启开启了 auto_restart 但已崩溃的项目
#[tauri::command]
pub fn health_tick(state: State<AppState>) -> Vec<String> {
    reap_dead(state.inner());
    let projects = state.config.lock().unwrap().projects.clone();
    let mut restarted = Vec::new();
    for p in projects.iter().filter(|p| p.auto_restart) {
        let alive = state.procs.lock().unwrap().contains_key(&p.id);
        if !alive {
            push_system_log(state.inner(), &p.id, "检测到进程退出，自动重启…");
            if start_project_inner(&p.id, state.inner()).is_ok() {
                restarted.push(p.id.clone());
            }
        }
    }
    restarted
}

// ---------- 内部实现 ----------

pub fn start_project_inner(id: &str, state: &AppState) -> Result<(), String> {
    if state.procs.lock().unwrap().contains_key(id) {
        return Err("项目已在运行".into());
    }

    let project = get_project(state, id).ok_or("找不到项目")?;

    let mut cmd = Command::new("/bin/sh");
    cmd.current_dir(&project.path)
        .arg("-lc")
        .arg(&project.start_command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for (k, v) in &project.env {
        cmd.env(k, v);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动失败: {e}（请检查工作目录与启动命令）"))?;
    let pid = child.id();

    let buf = state.log_buffer(id);
    push_log(&buf, "system", format!("▶ 启动: {} (pid {pid})", project.start_command));

    if let Some(out) = child.stdout.take() {
        spawn_reader(out, "stdout", buf.clone());
    }
    if let Some(err) = child.stderr.take() {
        spawn_reader(err, "stderr", buf.clone());
    }

    state.procs.lock().unwrap().insert(
        id.to_string(),
        RunningProc {
            pid,
            child,
            started_at: Instant::now(),
        },
    );
    Ok(())
}

pub fn stop_project_inner(id: &str, state: &AppState) -> Result<(), String> {
    let running = state.procs.lock().unwrap().remove(id);
    let Some(mut rp) = running else {
        return Ok(()); // 本来就没在运行
    };

    // 有自定义停止命令则优先执行
    if let Some(project) = get_project(state, id) {
        if let Some(stop_cmd) = project.stop_command.filter(|s| !s.trim().is_empty()) {
            let _ = Command::new("/bin/sh")
                .current_dir(&project.path)
                .arg("-lc")
                .arg(&stop_cmd)
                .status();
        }
    }

    // 结束整棵进程树（先 TERM 再 KILL）
    kill_tree(rp.pid);
    std::thread::sleep(Duration::from_millis(400));
    let _ = rp.child.kill();
    let _ = rp.child.wait();

    push_system_log(state, id, "■ 已停止");
    Ok(())
}

/// 回收已经退出的进程（更新 procs 表）
fn reap_dead(state: &AppState) {
    let mut procs = state.procs.lock().unwrap();
    let dead: Vec<String> = procs
        .iter_mut()
        .filter_map(|(id, rp)| match rp.child.try_wait() {
            Ok(Some(_)) => Some(id.clone()),
            _ => None,
        })
        .collect();
    for id in dead {
        procs.remove(&id);
    }
}

fn get_project(state: &AppState, id: &str) -> Option<Project> {
    state
        .config
        .lock()
        .unwrap()
        .projects
        .iter()
        .find(|p| p.id == id)
        .cloned()
}

/// 结束进程树：TERM 给整棵树，KILL 兜底
fn kill_tree(root: u32) {
    let mut sys = System::new_with_specifics(
        RefreshKind::new().with_processes(ProcessRefreshKind::new()),
    );
    sys.refresh_processes();

    let pids = collect_descendants(&sys, root);
    // 先 SIGTERM，给进程清理机会
    for pid in &pids {
        if let Some(p) = sys.process(*pid) {
            p.kill_with(Signal::Term);
        }
    }
    std::thread::sleep(Duration::from_millis(300));
    // 刷新后对仍存活的发 SIGKILL
    sys.refresh_processes();
    for pid in &pids {
        if let Some(p) = sys.process(*pid) {
            p.kill();
        }
    }
}

/// 收集 root 及其所有后代进程
fn collect_descendants(sys: &System, root: u32) -> Vec<Pid> {
    let root_pid = Pid::from_u32(root);
    let mut result = vec![root_pid];
    let mut frontier = vec![root_pid];
    while let Some(parent) = frontier.pop() {
        for (pid, proc_) in sys.processes() {
            if proc_.parent() == Some(parent) && !result.contains(pid) {
                result.push(*pid);
                frontier.push(*pid);
            }
        }
    }
    result
}

/// 探测本地端口是否有监听
fn port_is_listening(port: u16) -> bool {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    TcpStream::connect_timeout(&addr, Duration::from_millis(120)).is_ok()
}

// ---------- 日志辅助 ----------

fn spawn_reader<R: Read + Send + 'static>(
    r: R,
    stream: &'static str,
    buf: Arc<Mutex<VecDeque<LogLine>>>,
) {
    std::thread::spawn(move || {
        let reader = BufReader::new(r);
        for line in reader.lines() {
            match line {
                Ok(text) => push_log(&buf, stream, text),
                Err(_) => break,
            }
        }
    });
}

fn push_log(buf: &Arc<Mutex<VecDeque<LogLine>>>, stream: &str, text: String) {
    let mut b = buf.lock().unwrap();
    if b.len() >= MAX_LOG_LINES {
        b.pop_front();
    }
    b.push_back(LogLine {
        ts: chrono::Local::now().format("%H:%M:%S").to_string(),
        stream: stream.to_string(),
        text,
    });
}

fn push_system_log(state: &AppState, id: &str, text: &str) {
    let buf = state.log_buffer(id);
    push_log(&buf, "system", text.to_string());
}
