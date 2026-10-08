use crate::models::{ExitEvent, Project, ProjectStatus, StartOutcome};
use crate::logstore::LogBuffer;
use crate::state::{AppState, RunningProc};
use std::io::{BufRead, BufReader, Read};
use std::net::{SocketAddr, TcpStream};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, Signal, System};
use tauri::{AppHandle, Manager, State};

// ---------- 对外命令 ----------

/// 启动一个项目，并等待确认结果（已就绪 / 提前退出 / 端口超时）。
/// 最长会等待 START_PORT_TIMEOUT，所以放到阻塞线程里，不卡住界面。
#[tauri::command]
pub async fn start_project(id: String, app: AppHandle) -> Result<StartOutcome, String> {
    run_blocking(move || start_with_deps(&id, app.state::<AppState>().inner())).await
}

/// 停止一个项目
#[tauri::command]
pub fn stop_project(id: String, state: State<AppState>) -> Result<(), String> {
    stop_project_inner(&id, state.inner())
}

/// 重启一个项目，反馈同 start_project
#[tauri::command]
pub async fn restart_project(id: String, app: AppHandle) -> Result<StartOutcome, String> {
    run_blocking(move || {
        let state = app.state::<AppState>();
        let _ = stop_project_inner(&id, state.inner());
        std::thread::sleep(Duration::from_millis(600));
        start_with_deps(&id, state.inner())
    })
    .await
}

async fn run_blocking<T, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("内部错误: {e}"))?
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
    let jobs = state.jobs.lock().unwrap();

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

            let port_up = p.port.map(port_is_listening);

            ProjectStatus {
                id: p.id.clone(),
                running,
                pid,
                cpu,
                memory_mb: mem,
                uptime_secs: uptime,
                port_up,
                job: jobs.get(&p.id).map(|j| j.label.clone()),
                job_secs: jobs.get(&p.id).map(|j| j.started_at.elapsed().as_secs()),
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
    if project.start_command.trim().is_empty() {
        return Err("启动命令为空，请先编辑项目填写启动命令".into());
    }
    let mut cmd = shell_command(&project, &project.start_command)?;

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动失败: {e}（请检查工作目录与启动命令）"))?;
    let pid = child.id();

    let buf = state.log_buffer(id);
    push_log(&buf, "system", format!("▶ 启动: {} (pid {pid})", project.start_command));
    if let Some(rt) = crate::commands::runtime::describe(&project) {
        push_log(&buf, "system", format!("使用指定版本: {rt}"));
    }

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

/// 构造「在项目工作目录里，用登录 shell 执行一条命令」：带上选定的 Node / JDK 版本前缀和项目环境变量，
/// 输出走管道。启动项目和运行脚本共用，保证两者环境一致。
pub(crate) fn shell_command(project: &Project, command: &str) -> Result<Command, String> {
    if !std::path::Path::new(&project.path).is_dir() {
        return Err(format!("工作目录不存在或不是目录：{}", project.path));
    }
    // 选定的 Node / JDK 版本：写成命令前缀，保证在登录 shell 重排 PATH 之后才生效
    let prelude = crate::commands::runtime::shell_prelude(project)?;
    let mut cmd = Command::new("/bin/sh");
    cmd.current_dir(&project.path)
        .arg("-lc")
        .arg(format!("{prelude}{command}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in &project.env {
        cmd.env(k, v);
    }
    Ok(cmd)
}

// ---------- 启动依赖 ----------

/// 启动项目；若它声明了依赖，先确保依赖就绪（没运行就先启动，递归处理依赖的依赖）。
/// 依赖启动失败 / 迟迟不就绪时取消启动，并说明是哪个依赖、为什么。
pub fn start_with_deps(id: &str, state: &AppState) -> Result<StartOutcome, String> {
    start_with_deps_with(id, state, START_GRACE, START_PORT_TIMEOUT)
}

pub(crate) fn start_with_deps_with(
    id: &str,
    state: &AppState,
    grace: Duration,
    port_timeout: Duration,
) -> Result<StartOutcome, String> {
    let cx = Chain { state, grace, port_timeout };
    match cx.run(id, &mut Vec::new())? {
        ChainResult::Started { outcome, deps } => Ok(with_deps_note(outcome, &deps)),
        ChainResult::AlreadyRunning => Ok(StartOutcome::success("项目已在运行".into())),
    }
}

fn with_deps_note(mut outcome: StartOutcome, deps: &[String]) -> StartOutcome {
    if !deps.is_empty() {
        outcome.message = format!("{}（已先启动依赖：{}）", outcome.message, deps.join("、"));
    }
    outcome
}

enum ChainResult {
    /// 本项目是这次启动起来的；deps 是顺带启动的依赖名
    Started { outcome: StartOutcome, deps: Vec<String> },
    AlreadyRunning,
}

struct Chain<'a> {
    state: &'a AppState,
    grace: Duration,
    port_timeout: Duration,
}

impl Chain<'_> {
    /// path 是当前正在处理的依赖链，用来发现循环
    fn run(&self, id: &str, path: &mut Vec<String>) -> Result<ChainResult, String> {
        let project = get_project(self.state, id).ok_or("找不到项目")?;
        if path.iter().any(|p| p == id) {
            let names: Vec<String> = path
                .iter()
                .chain(std::iter::once(&id.to_string()))
                .map(|i| get_project(self.state, i).map(|p| p.name).unwrap_or_else(|| i.clone()))
                .collect();
            return Err(format!("依赖出现循环：{}", names.join(" → ")));
        }
        path.push(id.to_string());

        // 1. 先处理依赖（此时不持有自己的锁，避免两个互相等待的启动流程死锁）
        let mut started_deps: Vec<String> = Vec::new();
        for dep_id in &project.depends_on {
            let Some(dep) = get_project(self.state, dep_id) else { continue }; // 依赖已被删除，忽略
            match self.run(dep_id, path) {
                Ok(ChainResult::Started { outcome, deps }) => {
                    if outcome.level == "warning" {
                        path.pop();
                        return Err(format!("已取消启动：依赖「{}」没有真正就绪：{}", dep.name, outcome.message));
                    }
                    started_deps.extend(deps);
                    started_deps.push(dep.name);
                }
                Ok(ChainResult::AlreadyRunning) => {
                    if let Err(e) = self.wait_ready(&dep) {
                        path.pop();
                        return Err(format!("已取消启动：依赖「{}」{}", dep.name, e));
                    }
                }
                Err(e) => {
                    path.pop();
                    // 间接依赖失败时 e 本身就是「已取消启动：依赖「X」…」，拼成「依赖「B」的依赖「X」…」
                    let reason = match e.strip_prefix("已取消启动：依赖") {
                        Some(rest) => format!("的依赖{rest}"),
                        None => e,
                    };
                    return Err(format!("已取消启动：依赖「{}」{}", dep.name, reason));
                }
            }
        }
        path.pop();

        // 2. 自己：拿到启动锁后再看一眼——并发的另一个启动流程可能已经把它启动了
        let lock = self.state.start_lock(id);
        let _guard = lock.lock().unwrap();
        if running_ids(self.state).iter().any(|r| r == id) {
            return Ok(ChainResult::AlreadyRunning);
        }
        let outcome = start_and_verify_with(id, self.state, self.grace, self.port_timeout)?;
        Ok(ChainResult::Started { outcome, deps: started_deps })
    }

    /// 依赖已经在运行：配置了端口就等端口监听（可能是别的流程刚把它拉起来），没配端口则视为就绪
    fn wait_ready(&self, dep: &Project) -> Result<(), String> {
        let Some(port) = dep.port else { return Ok(()) };
        let started = Instant::now();
        while started.elapsed() < self.port_timeout {
            if port_is_listening(port) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        Err(format!("在运行，但 {} 秒内端口 {port} 仍未监听", self.port_timeout.as_secs()))
    }
}

/// 无端口时，进程存活满这么久才算「已启动」；期间退出则视为启动失败
const START_GRACE: Duration = Duration::from_secs(2);
/// 配置了端口时，最长等待端口开始监听的时间
const START_PORT_TIMEOUT: Duration = Duration::from_secs(30);

/// 启动项目并等待结果，让界面能明确告知 成功 / 失败 / 失败原因。
#[cfg(test)]
pub fn start_and_verify(id: &str, state: &AppState) -> Result<StartOutcome, String> {
    start_and_verify_with(id, state, START_GRACE, START_PORT_TIMEOUT)
}

/// 启动确认期间把项目登记为「确认中」，离开作用域（无论成功失败）自动注销
struct VerifyingGuard<'a> {
    state: &'a AppState,
    id: String,
}

impl<'a> VerifyingGuard<'a> {
    fn new(state: &'a AppState, id: &str) -> Self {
        state.verifying.lock().unwrap().insert(id.to_string());
        VerifyingGuard { state, id: id.to_string() }
    }
}

impl Drop for VerifyingGuard<'_> {
    fn drop(&mut self) {
        self.state.verifying.lock().unwrap().remove(&self.id);
    }
}

pub(crate) fn start_and_verify_with(
    id: &str,
    state: &AppState,
    grace: Duration,
    port_timeout: Duration,
) -> Result<StartOutcome, String> {
    let project = get_project(state, id).ok_or("找不到项目")?;
    // 启动前端口就已被占用：监听状态无法证明是本项目起来的，只能退回到存活判断
    let port = project.port;
    let port_busy_before = port.map(port_is_listening).unwrap_or(false);
    let watch_port = port.filter(|_| !port_busy_before);

    // 先登记「确认中」再启动：进程一进入进程表就已受保护，不存在会被巡检抢先回收的空窗
    let _verifying = VerifyingGuard::new(state, id);
    start_project_inner(id, state)?;
    let started = Instant::now();
    let limit = if watch_port.is_some() { port_timeout } else { grace };

    loop {
        // 进程是否已经退出
        let exited = {
            let mut procs = state.procs.lock().unwrap();
            match procs.get_mut(id) {
                None => return Err("项目在启动过程中被停止".into()),
                Some(rp) => match rp.child.try_wait() {
                    Ok(Some(status)) => {
                        procs.remove(id);
                        Some(status)
                    }
                    _ => None,
                },
            }
        };
        if let Some(status) = exited {
            std::thread::sleep(Duration::from_millis(250)); // 等读线程把最后的输出写进日志
            return finish_exited(id, state, &project, status);
        }

        if let Some(p) = watch_port {
            if port_is_listening(p) {
                let msg = format!("已启动，端口 {p} 已就绪（用时 {:.1}s）", started.elapsed().as_secs_f32());
                push_system_log(state, id, &format!("✔ {msg}"));
                return Ok(StartOutcome::success(msg));
            }
        }

        if started.elapsed() >= limit {
            let msg = match (watch_port, port) {
                (Some(p), _) => format!(
                    "进程在运行，但 {} 秒内端口 {p} 仍未监听，可能还在启动中；请查看日志确认",
                    limit.as_secs()
                ),
                (None, Some(p)) => {
                    let who = crate::commands::ports::describe_holders(&crate::commands::ports::who_holds(state, p));
                    let who = if who.is_empty() { "其它进程".to_string() } else { who };
                    format!("进程在运行，但端口 {p} 启动前就已被{who}占用，无法确认本项目是否就绪；若日志报端口冲突，请先结束占用者")
                }
                (None, None) => "进程已启动，但没配置端口，无法确认是否真正就绪；如果之后崩溃会再提示，也可查看日志".to_string(),
            };
            push_system_log(state, id, &msg);
            return Ok(if watch_port.is_some() || port.is_some() {
                StartOutcome::warning(msg)
            } else {
                StartOutcome::success(msg)
            });
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// 进程在确认窗口内就退出了：区分「正常跑完」和「失败」，失败时带上原因与最后的输出
fn finish_exited(
    id: &str,
    state: &AppState,
    project: &Project,
    status: std::process::ExitStatus,
) -> Result<StartOutcome, String> {
    if status.success() {
        let msg = if project.port.is_some() {
            "命令已执行完毕并退出（退出码 0），但没有常驻进程监听端口，卡片会显示为未运行".to_string()
        } else {
            "命令已执行完毕并退出（退出码 0）；它不是常驻进程，卡片会显示为未运行".to_string()
        };
        push_system_log(state, id, &msg);
        return Ok(if project.port.is_some() {
            StartOutcome::warning(msg)
        } else {
            StartOutcome::success(msg)
        });
    }

    use std::os::unix::process::ExitStatusExt;
    let reason = describe_exit(status.code(), status.signal());
    let tail = tail_since_start(&state.log_buffer(id), TAIL_LINES);
    push_system_log(state, id, &format!("✖ 启动失败：{reason}"));
    let mut msg = format!("启动失败：{reason}");
    if !tail.is_empty() {
        msg.push_str("\n最后输出：\n");
        msg.push_str(&tail.join("\n"));
    }
    Err(msg)
}

/// 把退出码 / 信号翻译成人话
pub(crate) fn describe_exit(code: Option<i32>, signal: Option<i32>) -> String {
    match (code, signal) {
        (Some(127), _) => "命令未找到（退出码 127），请检查启动命令，或该命令是否在登录 shell 的 PATH 中".into(),
        (Some(126), _) => "命令无法执行，权限不足（退出码 126）".into(),
        (Some(n), _) => format!("进程异常退出（退出码 {n}）"),
        (None, Some(sig)) => format!("进程被信号 {sig} 终止"),
        (None, None) => "进程异常退出".into(),
    }
}

/// 失败 / 退出提示里附带的输出行数
pub(crate) const TAIL_LINES: usize = 8;

/// 没有信息量的行：空行、只有 [ERROR] 这类日志级别前缀、纯分隔线
/// （Maven 的失败输出里一大半是这种，会把真正的错误行挤出摘要）
fn is_noise(line: &str) -> bool {
    let t = line.trim();
    let rest = match t.strip_prefix('[') {
        Some(r) => match r.split_once(']') {
            Some((level, after)) if level.chars().all(|c| c.is_ascii_alphabetic()) && level.len() <= 7 => after,
            _ => t,
        },
        None => t,
    };
    let rest = rest.trim();
    rest.is_empty() || rest.chars().all(|c| matches!(c, '-' | '=' | '_' | '*' | '#' | ' '))
}

/// 取本次启动以来最后 n 行 stdout / stderr 输出（不含系统消息），单行过长会截断
pub(crate) fn tail_since_start(buf: &LogBuffer, n: usize) -> Vec<String> {
    buf.with_lines(|b| {
        let mut lines: Vec<String> = Vec::new();
        for l in b.iter().rev() {
            if l.stream == "system" {
                if l.text.starts_with("▶ ") {
                    break;
                }
                continue;
            }
            if is_noise(&l.text) {
                continue;
            }
            let t: String = l.text.chars().take(200).collect();
            lines.push(t);
            if lines.len() >= n {
                break;
            }
        }
        lines.reverse();
        lines
    })
}

pub fn stop_project_inner(id: &str, state: &AppState) -> Result<(), String> {
    let running = state.procs.lock().unwrap().remove(id);
    let Some(mut rp) = running else {
        return Ok(()); // 本来就没在运行
    };

    // 有自定义停止命令则优先执行
    if let Some(project) = get_project(state, id) {
        // 停止命令（如 ./gradlew --stop）也要在同一套 Node / JDK 下执行
        let prelude = crate::commands::runtime::shell_prelude(&project).unwrap_or_default();
        if let Some(stop_cmd) = project.stop_command.clone().filter(|s| !s.trim().is_empty()) {
            let _ = Command::new("/bin/sh")
                .current_dir(&project.path)
                .arg("-lc")
                .arg(format!("{prelude}{stop_cmd}"))
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

/// 回收已经退出的进程（更新 procs 表）。
/// 用户主动停止的项目在 stop_project_inner 里已先从表中移除，不会走到这里；
/// 走到这里的都是自己退出的：非 0 退出（崩溃等）会留下一条通知给前端。
pub(crate) fn reap_dead(state: &AppState) {
    let dead: Vec<(String, std::process::ExitStatus)> = {
        let mut procs = state.procs.lock().unwrap();
        let verifying = state.verifying.lock().unwrap();
        let dead: Vec<_> = procs
            .iter_mut()
            .filter(|(id, _)| !verifying.contains(*id)) // 确认中的由确认流程自己处理
            .filter_map(|(id, rp)| match rp.child.try_wait() {
                Ok(Some(st)) => Some((id.clone(), st)),
                _ => None,
            })
            .collect();
        for (id, _) in &dead {
            procs.remove(id);
        }
        dead
    };
    for (id, status) in dead {
        record_exit(state, &id, status);
    }
}

/// 当前正在运行的项目 id（已排序，先回收已退出的）
pub fn running_ids(state: &AppState) -> Vec<String> {
    reap_dead(state);
    let mut ids: Vec<String> = state.procs.lock().unwrap().keys().cloned().collect();
    ids.sort();
    ids
}

/// 并行停止所有运行中的项目，返回停止的个数。退出应用时用：
/// 逐个停会各等 400ms，项目多时太慢
pub fn stop_all(state: &AppState) -> usize {
    let ids = running_ids(state);
    std::thread::scope(|s| {
        for id in &ids {
            s.spawn(move || {
                let _ = stop_project_inner(id, state);
            });
        }
    });
    ids.len()
}

/// 记录一次自行退出：写入项目日志，非 0 退出时再生成前端通知
fn record_exit(state: &AppState, id: &str, status: std::process::ExitStatus) {
    use std::os::unix::process::ExitStatusExt;
    if status.success() {
        push_system_log(state, id, "进程已退出（退出码 0）");
        return;
    }
    let reason = describe_exit(status.code(), status.signal());
    push_system_log(state, id, &format!("✖ 进程已退出：{reason}"));

    let tail = tail_since_start(&state.log_buffer(id), TAIL_LINES);
    let mut message = format!("进程已退出：{reason}");
    if !tail.is_empty() {
        message.push_str("\n最后输出：\n");
        message.push_str(&tail.join("\n"));
    }
    let name = get_project(state, id).map(|p| p.name).unwrap_or_else(|| id.to_string());
    // 窗口不在前台时，用系统通知告诉用户项目崩了
    let (title, body) = crate::notify::crash_text(&name, &reason, &tail);
    crate::notify::notify(state, &title, &body, false);
    let mut events = state.exit_events.lock().unwrap();
    events.push(ExitEvent {
        id: id.to_string(),
        name,
        ts: chrono::Local::now().format("%H:%M:%S").to_string(),
        message,
    });
    if events.len() > 50 {
        events.remove(0); // 前端长时间没来取时，只保留最近的
    }
}

/// 取走（并清空）尚未展示的进程退出通知；由前端定时轮询
#[tauri::command]
pub fn take_exit_events(state: State<AppState>) -> Vec<ExitEvent> {
    reap_dead(state.inner()); // 保证刚退出的进程也能被及时发现
    std::mem::take(&mut *state.exit_events.lock().unwrap())
}

pub(crate) fn get_project(state: &AppState, id: &str) -> Option<Project> {
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
pub(crate) fn kill_tree(root: u32) {
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
pub(crate) fn collect_descendants(sys: &System, root: u32) -> Vec<Pid> {
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

pub(crate) fn spawn_reader<R: Read + Send + 'static>(
    r: R,
    stream: &'static str,
    buf: LogBuffer,
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

pub(crate) fn push_log(buf: &LogBuffer, stream: &str, text: String) {
    buf.push(stream, text);
}

fn push_system_log(state: &AppState, id: &str, text: &str) {
    let buf = state.log_buffer(id);
    push_log(&buf, "system", text.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Config;

    fn state_with(project: serde_json::Value) -> AppState {
        let p: Project = serde_json::from_value(project).unwrap();
        AppState::for_test(Config { projects: vec![p], ..Default::default() })
    }

    fn run(cmd: &str, port: Option<u16>, path: &str) -> Result<StartOutcome, String> {
        let st = state_with(serde_json::json!({
            "id": "t", "name": "t", "path": path, "start_command": cmd, "port": port
        }));
        let r = start_and_verify_with("t", &st, Duration::from_millis(800), Duration::from_secs(5));
        let _ = stop_project_inner("t", &st);
        r
    }

    #[test]
    fn selected_node_and_jdk_are_used_when_starting() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("devbox-start-rt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let mk = |rel: &str, body: &str| {
            let p = d.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        mk("node18/bin/node", "echo 项目用的是-node-18.19.1");
        mk("jdk17/bin/java", "echo 项目用的是-java-home=$JAVA_HOME");
        let st = state_with(serde_json::json!({
            "id": "t", "name": "t", "path": "/tmp", "start_command": "node -v; java -version",
            "node": {"version": "18.19.1", "path": d.join("node18/bin").to_string_lossy()},
            "java": {"version": "17.0.9", "path": d.join("jdk17").to_string_lossy()},
        }));
        let out = start_and_verify_with("t", &st, Duration::from_millis(1500), Duration::from_secs(3)).unwrap();
        assert_eq!(out.level, "success", "{}", out.message);
        let logs: Vec<String> = st.log_buffer("t").snapshot().into_iter().map(|l| l.text).collect();
        let all = logs.join("\n");
        assert!(all.contains("项目用的是-node-18.19.1"), "{all}");
        assert!(all.contains(&format!("项目用的是-java-home={}", d.join("jdk17").display())), "{all}");
        assert!(all.contains("使用指定版本: Node 18.19.1 · JDK 17.0.9"), "{all}");
    }

    #[test]
    fn missing_selected_runtime_blocks_start_with_clear_message() {
        let st = state_with(serde_json::json!({
            "id": "t", "name": "t", "path": "/tmp", "start_command": "true",
            "node": {"version": "16.0.0", "path": "/no/such/bin"},
        }));
        let err = start_and_verify_with("t", &st, Duration::from_millis(500), Duration::from_secs(1)).unwrap_err();
        assert!(err.contains("Node 16.0.0") && err.contains("已不存在"), "{err}");
    }

    // ---------- 启动依赖 ----------

    fn state_of(projects: serde_json::Value) -> AppState {
        let list: Vec<Project> = serde_json::from_value(projects).unwrap();
        AppState::for_test(Config { projects: list, ..Default::default() })
    }

    fn quick(id: &str, st: &AppState) -> Result<StartOutcome, String> {
        start_with_deps_with(id, st, Duration::from_millis(300), Duration::from_secs(3))
    }

    fn started_at(st: &AppState, id: &str) -> Instant {
        st.procs.lock().unwrap().get(id).unwrap().started_at
    }

    #[test]
    fn dependency_is_started_first_and_noted() {
        let st = state_of(serde_json::json!([
            {"id":"a","name":"数据库","path":"/tmp","start_command":"sleep 30"},
            {"id":"b","name":"后端","path":"/tmp","start_command":"sleep 30","depends_on":["a"]},
        ]));
        let out = quick("b", &st).unwrap();
        assert!(out.message.contains("已先启动依赖：数据库"), "{}", out.message);
        assert!(started_at(&st, "a") <= started_at(&st, "b"), "依赖要先于自己启动");
        assert_eq!(running_ids(&st), vec!["a", "b"]);
        stop_all(&st);
    }

    #[test]
    fn already_running_dependency_is_not_restarted() {
        let st = state_of(serde_json::json!([
            {"id":"a","name":"A","path":"/tmp","start_command":"sleep 30"},
            {"id":"b","name":"B","path":"/tmp","start_command":"sleep 30","depends_on":["a"]},
        ]));
        quick("a", &st).unwrap();
        let before = started_at(&st, "a");
        let out = quick("b", &st).unwrap();
        assert!(!out.message.contains("已先启动依赖"), "{}", out.message);
        assert_eq!(started_at(&st, "a"), before, "依赖没被重启");
        stop_all(&st);
    }

    #[test]
    fn transitive_dependencies_start_in_order() {
        let st = state_of(serde_json::json!([
            {"id":"a","name":"A","path":"/tmp","start_command":"sleep 30"},
            {"id":"b","name":"B","path":"/tmp","start_command":"sleep 30","depends_on":["a"]},
            {"id":"c","name":"C","path":"/tmp","start_command":"sleep 30","depends_on":["b"]},
        ]));
        let out = quick("c", &st).unwrap();
        assert!(out.message.contains("已先启动依赖：A、B"), "{}", out.message);
        assert!(started_at(&st, "a") <= started_at(&st, "b") && started_at(&st, "b") <= started_at(&st, "c"));
        stop_all(&st);
    }

    #[test]
    fn failing_dependency_cancels_the_dependent_with_the_reason() {
        let st = state_of(serde_json::json!([
            {"id":"a","name":"数据库","path":"/tmp","start_command":"echo '连不上磁盘' >&2; exit 3"},
            {"id":"b","name":"后端","path":"/tmp","start_command":"sleep 30","depends_on":["a"]},
        ]));
        let err = quick("b", &st).unwrap_err();
        assert!(err.starts_with("已取消启动：依赖「数据库」"), "{err}");
        assert!(err.contains("退出码 3") && err.contains("连不上磁盘"), "{err}");
        assert!(running_ids(&st).is_empty(), "依赖失败后自己不该被启动");
    }

    #[test]
    fn transitive_failure_names_the_whole_chain() {
        let st = state_of(serde_json::json!([
            {"id":"x","name":"存储","path":"/tmp","start_command":"echo '磁盘满了' >&2; exit 5"},
            {"id":"b","name":"后端","path":"/tmp","start_command":"sleep 30","depends_on":["x"]},
            {"id":"c","name":"前端","path":"/tmp","start_command":"sleep 30","depends_on":["b"]},
        ]));
        let err = quick("c", &st).unwrap_err();
        assert!(err.starts_with("已取消启动：依赖「后端」的依赖「存储」启动失败"), "{err}");
        assert!(err.contains("磁盘满了"), "{err}");
        assert!(running_ids(&st).is_empty());
    }

    #[test]
    fn dependency_that_never_listens_blocks_the_dependent() {
        // a 在运行但它声明的端口一直没监听 → b 不该启动
        let st = state_of(serde_json::json!([
            {"id":"a","name":"A","path":"/tmp","start_command":"sleep 30","port":31969},
            {"id":"b","name":"B","path":"/tmp","start_command":"sleep 30","depends_on":["a"]},
        ]));
        let err = start_with_deps_with("b", &st, Duration::from_millis(300), Duration::from_secs(1)).unwrap_err();
        assert!(err.contains("已取消启动：依赖「A」没有真正就绪"), "{err}");
        assert_eq!(running_ids(&st), vec!["a"], "b 没被启动");
        // a 已经在运行、端口仍没监听：走「等待就绪」分支，同样拒绝
        let err = start_with_deps_with("b", &st, Duration::from_millis(300), Duration::from_secs(1)).unwrap_err();
        assert!(err.contains("已取消启动：依赖「A」在运行，但") && err.contains("端口 31969 仍未监听"), "{err}");
        stop_all(&st);
    }

    #[test]
    fn dependency_with_a_listening_port_is_ready() {
        let st = state_of(serde_json::json!([
            {"id":"a","name":"API","path":"/tmp","start_command":crate::testutil::listen_command(31968),"port":31968},
            {"id":"b","name":"前端","path":"/tmp","start_command":"sleep 30","depends_on":["a"]},
        ]));
        let out = start_with_deps_with("b", &st, Duration::from_millis(300), Duration::from_secs(8)).unwrap();
        assert!(out.message.contains("已先启动依赖：API"), "{}", out.message);
        assert!(port_is_listening(31968), "启动 b 时 a 的端口已经在监听");
        stop_all(&st);
    }

    #[test]
    fn runtime_cycle_is_detected_even_if_config_was_edited_by_hand() {
        let st = state_of(serde_json::json!([
            {"id":"a","name":"A","path":"/tmp","start_command":"sleep 30","depends_on":["b"]},
            {"id":"b","name":"B","path":"/tmp","start_command":"sleep 30","depends_on":["a"]},
        ]));
        let err = quick("a", &st).unwrap_err();
        assert!(err.contains("依赖出现循环") && err.contains("A → B → A"), "{err}");
        assert!(running_ids(&st).is_empty());
    }

    #[test]
    fn concurrent_starts_start_the_shared_dependency_only_once() {
        let st = std::sync::Arc::new(state_of(serde_json::json!([
            {"id":"a","name":"A","path":"/tmp","start_command":"sleep 30"},
            {"id":"b","name":"B","path":"/tmp","start_command":"sleep 30","depends_on":["a"]},
            {"id":"c","name":"C","path":"/tmp","start_command":"sleep 30","depends_on":["a"]},
        ])));
        // 模拟「本组启动」：三个启动同时发起
        let handles: Vec<_> = ["a", "b", "c"]
            .into_iter()
            .map(|id| {
                let st = st.clone();
                std::thread::spawn(move || quick(id, &st))
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(results.iter().all(|r| r.is_ok()), "{results:?}");
        assert_eq!(running_ids(&st), vec!["a", "b", "c"]);
        stop_all(&st);
    }

    #[test]
    fn starting_a_running_project_is_not_an_error() {
        let st = state_of(serde_json::json!([{"id":"a","name":"A","path":"/tmp","start_command":"sleep 30"}]));
        quick("a", &st).unwrap();
        let out = quick("a", &st).unwrap();
        assert!(out.message.contains("已在运行"), "{}", out.message);
        stop_all(&st);
    }

    #[test]
    fn noise_filter_keeps_the_real_error_line() {
        assert!(is_noise(""));
        assert!(is_noise("[ERROR] "));
        assert!(is_noise("[INFO] ------------------------------------------------------------------------"));
        assert!(is_noise("=========="));
        assert!(!is_noise("[ERROR] No plugin found for prefix 'spring-boot' in the current project"));
        assert!(!is_noise("[INFO] BUILD FAILURE"));
        assert!(!is_noise("Error: Cannot find module 'express'"));
        assert!(!is_noise("[1] something"), "方括号里不是日志级别时按正文处理");
    }

    #[test]
    fn maven_style_failure_tail_contains_root_cause() {
        let st = state_with(serde_json::json!({
            "id": "t", "name": "t", "path": "/tmp", "start_command": "x"
        }));
        let buf = st.log_buffer("t");
        let push = |s: &str| push_log(&buf, "stdout", s.to_string());
        push_log(&buf, "system", "▶ 启动: mvn spring-boot:run (pid 1)".into());
        for m in ["ruoyi-common", "ruoyi-admin"] {
            push(&format!("[INFO] {m} ........ SKIPPED"));
        }
        push("[INFO] ------------------------------------------------------------------------");
        push("[INFO] BUILD FAILURE");
        push("[INFO] ------------------------------------------------------------------------");
        push("[ERROR] No plugin found for prefix 'spring-boot' in the current project");
        push("[ERROR] ");
        push("[ERROR] To see the full stack trace of the errors, re-run Maven with the -e switch.");
        push("[ERROR] Re-run Maven using the -X switch to enable full debug logging.");
        push("[ERROR] ");
        push("[ERROR] For more information about the errors and possible solutions, please read the following articles:");
        let tail = tail_since_start(&buf, TAIL_LINES).join("\n");
        assert!(tail.contains("No plugin found for prefix 'spring-boot'"), "{tail}");
        assert!(!tail.contains("-----"), "分隔线应被过滤: {tail}");
    }

    #[test]
    fn late_crash_after_start_is_reported_once() {
        let st = state_with(serde_json::json!({
            "id": "t", "name": "站群项目", "path": "/tmp",
            "start_command": "echo '[ERROR] No plugin found' ; sleep 1; exit 4"
        }));
        // 确认窗口只有 0.2 秒：此时进程还活着，启动被报告为成功
        let out = start_and_verify_with("t", &st, Duration::from_millis(200), Duration::from_secs(1)).unwrap();
        assert_eq!(out.level, "success");
        assert!(st.exit_events.lock().unwrap().is_empty(), "还没退出，不该有通知");

        // 之后进程自己崩了：下一次回收时产生一条通知，包含原因和关键输出
        std::thread::sleep(Duration::from_millis(1600));
        reap_dead(&st);
        let evs: Vec<ExitEvent> = st.exit_events.lock().unwrap().clone();
        assert_eq!(evs.len(), 1, "{evs:?}");
        assert_eq!(evs[0].name, "站群项目");
        assert!(evs[0].message.contains("退出码 4"), "{}", evs[0].message);
        assert!(evs[0].message.contains("No plugin found"), "{}", evs[0].message);
        // 再回收不会重复通知
        reap_dead(&st);
        assert_eq!(st.exit_events.lock().unwrap().len(), 1);
    }

    #[test]
    fn user_stop_and_clean_exit_do_not_notify() {
        // 用户主动停止：不算崩溃
        let st = state_with(serde_json::json!({
            "id": "t", "name": "t", "path": "/tmp", "start_command": "sleep 30"
        }));
        start_and_verify_with("t", &st, Duration::from_millis(200), Duration::from_secs(1)).unwrap();
        stop_project_inner("t", &st).unwrap();
        reap_dead(&st);
        assert!(st.exit_events.lock().unwrap().is_empty());

        // 退出码 0：只写日志，不弹窗
        let st = state_with(serde_json::json!({
            "id": "t", "name": "t", "path": "/tmp", "start_command": "sleep 1; exit 0"
        }));
        start_and_verify_with("t", &st, Duration::from_millis(200), Duration::from_secs(1)).unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        reap_dead(&st);
        assert!(st.exit_events.lock().unwrap().is_empty());
    }

    #[test]
    fn fast_crash_is_reported_correctly_even_if_something_else_reaps_concurrently() {
        // 巡检 / 状态轮询随时会调用 reap_dead。启动确认期间进程立刻崩溃时，
        // 不能被它抢先收走，否则确认流程只看到「进程不见了」，会误报「被停止」并丢失退出码和输出。
        let st = std::sync::Arc::new(state_with(serde_json::json!({
            "id": "t", "name": "t", "path": "/tmp",
            "start_command": "echo 'Error: 磁盘满了' >&2; exit 3"
        })));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reaper = {
            let (st, stop) = (st.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                    reap_dead(&st);
                    std::thread::sleep(Duration::from_millis(1));
                }
            })
        };
        for i in 0..25 {
            let err = start_and_verify_with("t", &st, Duration::from_millis(1500), Duration::from_secs(1)).unwrap_err();
            assert!(err.contains("退出码 3") && err.contains("磁盘满了"), "第 {i} 次: {err}");
            assert!(!err.contains("被停止"), "第 {i} 次: {err}");
        }
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        reaper.join().unwrap();
        // 启动流程自己报告了失败，不应再重复产生「意外退出」通知
        assert!(st.exit_events.lock().unwrap().is_empty(), "{:?}", st.exit_events.lock().unwrap());
    }

    #[test]
    fn describes_exit_reasons() {
        assert!(describe_exit(Some(127), None).contains("命令未找到"));
        assert!(describe_exit(Some(126), None).contains("权限"));
        assert!(describe_exit(Some(3), None).contains("退出码 3"));
        assert!(describe_exit(None, Some(9)).contains("信号 9"));
    }

    #[test]
    fn crash_reports_reason_and_last_output() {
        let err = run("echo 连接数据库失败 >&2; exit 3", None, "/tmp").unwrap_err();
        assert!(err.contains("退出码 3"), "{err}");
        assert!(err.contains("连接数据库失败"), "{err}");
    }

    #[test]
    fn command_not_found_is_explained() {
        let err = run("definitely-not-a-command-xyz", None, "/tmp").unwrap_err();
        assert!(err.contains("命令未找到"), "{err}");
    }

    #[test]
    fn missing_workdir_is_reported() {
        let err = run("true", None, "/no/such/dir").unwrap_err();
        assert!(err.contains("工作目录不存在"), "{err}");
    }

    #[test]
    fn long_running_without_port_is_success() {
        let out = run("sleep 5", None, "/tmp").unwrap();
        assert_eq!(out.level, "success");
    }

    #[test]
    fn one_shot_command_exit_zero_is_not_failure() {
        let out = run("true", None, "/tmp").unwrap();
        assert_eq!(out.level, "success");
        assert!(out.message.contains("退出码 0"));
    }

    #[test]
    fn port_never_listening_is_warning() {
        // 31999 上没有任何监听；进程活着，等满 5s 超时后给出警告而不是成功
        let out = run("sleep 10", Some(31999), "/tmp").unwrap();
        assert_eq!(out.level, "warning");
        assert!(out.message.contains("31999"));
    }

    #[test]
    fn port_ready_is_success() {
        let out = run(&crate::testutil::listen_command(31998), Some(31998), "/tmp").unwrap();
        assert_eq!(out.level, "success");
        assert!(out.message.contains("已就绪"), "{}", out.message);
    }
}
