//! 应用生命周期：退出时停止项目、记录「最近运行的项目」并在下次启动时提示恢复。
//!
//! 为什么退出时必须停止项目：项目进程的 stdout / stderr 管道由 DevBox 持有，
//! DevBox 一旦退出，管道断开，项目再写日志会收到 SIGPIPE 而崩溃，
//! 所以「退出但保持项目运行」做不到，只能停止并在下次启动时恢复。

use crate::commands::process::{reap_dead, running_ids, stop_all};
use crate::commands::scripts::cancel_all_jobs;
use crate::models::ProcRecord;
use crate::state::AppState;
use serde::Serialize;
use std::sync::atomic::Ordering::SeqCst;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

/// 某个 pid 的进程启动时间（自 epoch 的秒数）；进程不存在返回 None
fn proc_start_time(sys: &sysinfo::System, pid: u32) -> Option<u64> {
    sys.process(sysinfo::Pid::from_u32(pid)).map(|p| p.start_time())
}

fn new_system() -> sysinfo::System {
    use sysinfo::{ProcessRefreshKind, RefreshKind, System};
    let mut sys = System::new_with_specifics(RefreshKind::new().with_processes(ProcessRefreshKind::new()));
    sys.refresh_processes();
    sys
}

/// 把「正在运行的项目」同步到配置里（有变化才落盘）。返回是否有变化。
/// - 进程记录（pid + 启动时间）随时更新，这样即使被强杀也能找到孤儿进程
/// - 「最近运行的项目」（用于恢复提示）在退出过程中、或还没问过用户是否恢复时冻结，
///   保护上次的记录不被覆盖
pub fn sync_last_running(state: &AppState) -> bool {
    if state.quitting.load(SeqCst) {
        return false;
    }
    let running = running_ids(state);
    let records: Vec<ProcRecord> = if running.is_empty() {
        Vec::new()
    } else {
        let sys = new_system();
        let procs = state.procs.lock().unwrap();
        running
            .iter()
            .filter_map(|id| {
                let pid = procs.get(id)?.pid;
                Some(ProcRecord { id: id.clone(), pid, start_time: proc_start_time(&sys, pid)? })
            })
            .collect()
    };
    let changed = {
        let mut cfg = state.config.lock().unwrap();
        let mut changed = false;
        if cfg.last_procs != records {
            cfg.last_procs = records;
            changed = true;
        }
        if !state.restore_pending.load(SeqCst) && cfg.last_running != running {
            cfg.last_running = running;
            changed = true;
        }
        changed
    };
    if changed {
        let _ = state.persist();
    }
    changed
}

/// 启动时清理上次残留的项目进程：DevBox 被强杀 / 崩溃（含开发模式下重编译重启）后，
/// 项目进程会变成孤儿继续跑，占着端口，而新实例里它们显示为「未运行」。
/// 只有 pid 和进程启动时间都对得上才清理，pid 被别的进程复用了不会误杀。返回被清理的项目名。
pub fn reap_orphans(state: &AppState) -> Vec<String> {
    let records = std::mem::take(&mut state.config.lock().unwrap().last_procs);
    if records.is_empty() {
        return vec![];
    }
    let sys = new_system();
    let mut cleaned = Vec::new();
    for r in &records {
        if proc_start_time(&sys, r.pid) == Some(r.start_time) {
            crate::commands::process::kill_tree(r.pid);
            let name = state
                .config
                .lock()
                .unwrap()
                .projects
                .iter()
                .find(|p| p.id == r.id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| r.id.clone());
            eprintln!("[devbox] 清理上次残留的项目进程: {name} (pid {})", r.pid);
            cleaned.push(name);
        }
    }
    let _ = state.persist(); // 记录已清空，落盘
    state.orphans_cleaned.lock().unwrap().extend(cleaned.iter().cloned());
    cleaned
}

/// 退出前：最后记录一次运行集合，然后冻结
pub fn freeze_snapshot(state: &AppState) {
    sync_last_running(state);
    state.quitting.store(true, SeqCst);
}

/// 应用真正退出时的清理：记录运行集合并停止所有项目，保证不留孤儿进程
pub fn handle_exit(state: &AppState) {
    freeze_snapshot(state);
    cancel_all_jobs(state);
    stop_all(state);
}

/// 后台巡检：即使窗口关着 / 在后台，也能及时发现项目崩溃，并保持运行集合记录最新
pub fn tick(state: &AppState) {
    reap_dead(state);
    sync_last_running(state);
}

pub fn spawn_supervisor(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(2));
        tick(&app.state::<AppState>());
        crate::tray::refresh(&app); // 项目启停 / 增删 / 改名后，托盘菜单跟着更新
    });
}

/// 正在运行的项目数和脚本任务数（退出前要告诉用户）
fn busy(state: &AppState) -> (usize, usize) {
    (running_ids(state).len(), state.jobs.lock().unwrap().len())
}

/// 点了窗口的关闭按钮：有运行中的项目就拦下关闭、让前端确认。
/// 必须在关闭这一刻拦：等窗口被销毁、变成 ExitRequested 时再拦，已经没有窗口可以弹确认了。
pub fn on_close_requested(window: &tauri::Window, api: &tauri::CloseRequestApi) {
    let app = window.app_handle();
    let state = app.state::<AppState>();
    if state.quit_confirmed.load(SeqCst) {
        return;
    }
    let (projects, jobs) = busy(&state);
    if projects + jobs == 0 {
        return;
    }
    api.prevent_close();
    let _ = app.emit("quit-requested", serde_json::json!({ "projects": projects, "jobs": jobs }));
}

/// 收到退出请求：没有运行中的项目就直接放行；有的话拦下来，让前端弹确认
pub fn on_exit_requested(app: &AppHandle, api: &tauri::ExitRequestApi) {
    let state = app.state::<AppState>();
    if state.quit_confirmed.load(SeqCst) {
        return;
    }
    let (projects, jobs) = busy(&state);
    if projects + jobs == 0 {
        return;
    }
    api.prevent_exit();
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    let _ = app.emit("quit-requested", serde_json::json!({ "projects": projects, "jobs": jobs }));
}

/// Ctrl+C / kill 等终止信号：视为已确认退出（没人能回答对话框），走正常清理流程
pub fn install_signal_handler(app: AppHandle) {
    let _ = ctrlc::set_handler(move || {
        app.state::<AppState>().quit_confirmed.store(true, SeqCst);
        app.exit(0);
    });
}

/// 用户在前端确认退出
#[tauri::command]
pub fn quit_app(app: AppHandle, state: State<AppState>) {
    state.quit_confirmed.store(true, SeqCst);
    app.exit(0);
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct RestoreItem {
    pub id: String,
    pub name: String,
}

/// 上次退出时在运行、现在没在运行的项目（项目已被删除的忽略）
fn restore_candidates(state: &AppState) -> Vec<RestoreItem> {
    if !state.restore_pending.load(SeqCst) {
        return vec![];
    }
    let running = running_ids(state);
    let cfg = state.config.lock().unwrap();
    cfg.last_running
        .iter()
        .filter(|id| !running.contains(id))
        .filter_map(|id| cfg.projects.iter().find(|p| &p.id == id))
        .map(|p| RestoreItem { id: p.id.clone(), name: p.name.clone() })
        .collect()
}

#[derive(Serialize, Clone, Debug)]
pub struct RestoreInfo {
    /// 可以恢复的项目
    pub items: Vec<RestoreItem>,
    /// 启动时已清理掉的、上次异常退出残留的进程（项目名）
    pub cleaned: Vec<String>,
}

/// 前端启动时询问：有哪些项目可以恢复，以及清理了哪些残留进程
#[tauri::command]
pub fn pending_restore(state: State<AppState>) -> RestoreInfo {
    let items = restore_candidates(&state);
    if items.is_empty() {
        // 没有可恢复的，直接放行记录，不再等待
        state.restore_pending.store(false, SeqCst);
    }
    let cleaned = std::mem::take(&mut *state.orphans_cleaned.lock().unwrap());
    RestoreInfo { items, cleaned }
}

/// 用户已答复（恢复或忽略）：解除冻结，从现在起重新记录运行集合
#[tauri::command]
pub fn resolve_restore(state: State<AppState>) {
    state.restore_pending.store(false, SeqCst);
    sync_last_running(&state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::process::{start_and_verify, start_and_verify_with};
    use crate::models::Config;
    use std::time::Duration;

    fn state(n: usize) -> AppState {
        let projects = (0..n)
            .map(|i| {
                serde_json::from_value(serde_json::json!({
                    "id": format!("p{i}"), "name": format!("项目{i}"), "path": "/tmp", "start_command": "sleep 30"
                }))
                .unwrap()
            })
            .collect();
        AppState::for_test(Config { projects, ..Default::default() })
    }

    fn alive(pid: u32) -> bool {
        std::process::Command::new("kill").args(["-0", &pid.to_string()]).output().map_or(false, |o| o.status.success())
    }

    #[test]
    fn running_set_is_tracked_and_persisted() {
        let st = state(3);
        assert!(!sync_last_running(&st), "没有变化不落盘");
        start_and_verify("p0", &st).unwrap();
        start_and_verify("p2", &st).unwrap();
        assert!(sync_last_running(&st));
        assert_eq!(st.config.lock().unwrap().last_running, vec!["p0", "p2"]);
        assert!(!sync_last_running(&st), "同一集合不重复落盘");
        // 落盘内容可以被重新读回
        let saved: Config = serde_json::from_str(&std::fs::read_to_string(&st.config_path).unwrap()).unwrap();
        assert_eq!(saved.last_running, vec!["p0", "p2"]);

        crate::commands::process::stop_project_inner("p0", &st).unwrap();
        assert!(sync_last_running(&st));
        assert_eq!(st.config.lock().unwrap().last_running, vec!["p2"]);
        stop_all(&st);
    }

    #[test]
    fn exit_stops_everything_but_remembers_what_was_running() {
        let st = state(2);
        start_and_verify("p0", &st).unwrap();
        start_and_verify("p1", &st).unwrap();
        let pids: Vec<u32> = st.procs.lock().unwrap().values().map(|r| r.pid).collect();
        assert!(pids.iter().all(|p| alive(*p)));

        handle_exit(&st);
        std::thread::sleep(Duration::from_millis(300));
        assert!(st.procs.lock().unwrap().is_empty());
        assert!(pids.iter().all(|p| !alive(*p)), "退出后不能留下孤儿进程");
        // 记录的是退出「之前」在运行的项目，而不是停止之后的空集合
        assert_eq!(st.config.lock().unwrap().last_running, vec!["p0", "p1"]);
        // 冻结后即使巡检也不会把记录冲掉
        tick(&st);
        assert_eq!(st.config.lock().unwrap().last_running, vec!["p0", "p1"]);
    }

    #[test]
    fn restore_candidates_skip_running_deleted_and_unanswered_freeze() {
        let st = state(3);
        st.config.lock().unwrap().last_running = vec!["p0".into(), "p1".into(), "gone".into()];
        assert!(restore_candidates(&st).is_empty(), "没有待恢复标记时不提示");

        st.restore_pending.store(true, SeqCst);
        start_and_verify("p1", &st).unwrap(); // p1 已经在运行了
        let items = restore_candidates(&st);
        assert_eq!(items, vec![RestoreItem { id: "p0".into(), name: "项目0".into() }], "已运行的、已删除的都不提示");

        // 还没答复期间，「最近运行的项目」被冻结，不会因为「现在只有 p1 在跑」而覆盖
        sync_last_running(&st);
        assert_eq!(st.config.lock().unwrap().last_running, vec!["p0", "p1", "gone"]);
        // 但进程记录照常更新（强杀后才找得到孤儿）
        assert_eq!(st.config.lock().unwrap().last_procs.len(), 1);

        // 答复后解除冻结，开始记录实际运行集合
        st.restore_pending.store(false, SeqCst);
        assert!(sync_last_running(&st));
        assert_eq!(st.config.lock().unwrap().last_running, vec!["p1"]);
        stop_all(&st);
    }

    #[test]
    fn crash_is_noticed_by_the_supervisor_without_a_window() {
        let st = AppState::for_test(Config {
            projects: vec![serde_json::from_value(serde_json::json!({
                "id": "c", "name": "崩溃项目", "path": "/tmp", "start_command": "sleep 1; exit 3"
            }))
            .unwrap()],
            ..Default::default()
        });
        // 确认窗口只有 0.2 秒：此时进程还活着，启动算成功；它会在 1 秒后自己崩
        start_and_verify_with("c", &st, Duration::from_millis(200), Duration::from_secs(1)).unwrap();
        tick(&st);
        assert_eq!(st.config.lock().unwrap().last_running, vec!["c"]);
        std::thread::sleep(Duration::from_millis(1700));
        tick(&st); // 巡检线程每 2 秒做的事
        assert_eq!(st.exit_events.lock().unwrap().len(), 1, "窗口关着也能发现崩溃");
        assert!(st.config.lock().unwrap().last_running.is_empty(), "崩了的项目不再算运行中");
    }

    // ---------- 孤儿进程 ----------

    fn spawn_detached_sleep(secs: u32) -> std::process::Child {
        std::process::Command::new("sleep").arg(secs.to_string()).spawn().unwrap()
    }

    #[test]
    fn process_records_carry_pid_and_start_time_and_are_updated() {
        let st = state(2);
        start_and_verify("p0", &st).unwrap();
        assert!(sync_last_running(&st));
        let rec = st.config.lock().unwrap().last_procs.clone();
        assert_eq!(rec.len(), 1);
        assert_eq!(rec[0].id, "p0");
        assert_eq!(rec[0].pid, st.procs.lock().unwrap().get("p0").unwrap().pid);
        assert!(rec[0].start_time > 1_600_000_000, "应是 epoch 秒: {}", rec[0].start_time);
        assert!(!sync_last_running(&st), "没变化不重复落盘");
        // 重启 → pid 变了，记录跟着变
        crate::commands::process::stop_project_inner("p0", &st).unwrap();
        start_and_verify("p0", &st).unwrap();
        assert!(sync_last_running(&st));
        assert_ne!(st.config.lock().unwrap().last_procs[0].pid, rec[0].pid);
        stop_all(&st);
        sync_last_running(&st);
        assert!(st.config.lock().unwrap().last_procs.is_empty(), "项目都停了，记录清空");
    }

    #[test]
    fn orphans_from_a_killed_previous_run_are_cleaned_up() {
        // 模拟上一个 DevBox 被强杀：项目进程还活着，配置里留着它的记录，而本实例的 procs 是空的
        let st = state(1);
        let mut orphan = spawn_detached_sleep(60);
        let sys = new_system();
        let start = proc_start_time(&sys, orphan.id()).unwrap();
        st.config.lock().unwrap().last_procs = vec![ProcRecord { id: "p0".into(), pid: orphan.id(), start_time: start }];
        assert!(alive(orphan.id()));

        let cleaned = reap_orphans(&st);
        assert_eq!(cleaned, vec!["项目0"]);
        let _ = orphan.wait(); // 回收僵尸，才能判断它真的退出了
        assert!(!alive(orphan.id()), "残留进程应被结束");
        assert!(st.config.lock().unwrap().last_procs.is_empty());
        assert_eq!(*st.orphans_cleaned.lock().unwrap(), vec!["项目0"]);
    }

    #[test]
    fn a_reused_pid_is_never_killed() {
        // pid 还在但启动时间对不上 = 这个 pid 已被别的进程复用，绝不能杀
        let st = state(1);
        let mut other = spawn_detached_sleep(60);
        let sys = new_system();
        let start = proc_start_time(&sys, other.id()).unwrap();
        st.config.lock().unwrap().last_procs =
            vec![ProcRecord { id: "p0".into(), pid: other.id(), start_time: start + 1000 }];
        assert!(reap_orphans(&st).is_empty());
        assert!(alive(other.id()), "启动时间对不上，不能误杀");
        let _ = other.kill();
        let _ = other.wait();
    }

    #[test]
    fn dead_or_missing_records_are_ignored_and_cleared() {
        let st = state(1);
        st.config.lock().unwrap().last_procs = vec![ProcRecord { id: "p0".into(), pid: 4_000_000, start_time: 1 }];
        assert!(reap_orphans(&st).is_empty());
        assert!(st.config.lock().unwrap().last_procs.is_empty(), "记录用完即清");
        assert!(reap_orphans(&st).is_empty(), "没有记录时什么也不做");
    }

    #[test]
    fn clean_exit_leaves_no_orphans_to_reap_next_time() {
        let st = state(2);
        start_and_verify("p0", &st).unwrap();
        start_and_verify("p1", &st).unwrap();
        sync_last_running(&st);
        handle_exit(&st); // 正常退出：停掉所有项目
        std::thread::sleep(Duration::from_millis(300));
        // 下次启动：记录里的 pid 已经不存在了，不会清理任何东西
        // （先取出再构造：同一条语句里连续 lock() 同一个 Mutex，前一个守卫还没释放会自死锁）
        let (projects, last_procs) = {
            let cfg = st.config.lock().unwrap();
            (cfg.projects.clone(), cfg.last_procs.clone())
        };
        let next = AppState::for_test(Config { projects, last_procs, ..Default::default() });
        assert!(reap_orphans(&next).is_empty());
    }
}
