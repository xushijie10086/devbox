use crate::logstore::{file_name_for, LogBuffer, LogStore};
use crate::models::{Config, ExitEvent};
use std::collections::HashMap;
use std::process::Child;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// 一个正在运行的项目进程
pub struct RunningProc {
    pub pid: u32,
    pub child: Child,
    pub started_at: Instant,
}

/// 应用句柄：后台线程（巡检、托盘菜单回调）要发系统通知、刷新托盘时用
pub static APP_HANDLE: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();

/// 一个正在运行的脚本任务（install / build / test 等一次性命令）。每个项目同时最多一个
pub struct Job {
    pub label: String,
    pub pid: u32,
    pub started_at: Instant,
    /// 用户点了取消：结束后据此区分「被取消」和「失败」
    pub cancelled: bool,
}

/// 全局应用状态
pub struct AppState {
    pub config: Mutex<Config>,
    pub config_path: std::path::PathBuf,
    /// project_id -> 运行中的进程
    pub procs: Mutex<HashMap<String, RunningProc>>,
    /// project_id -> 日志缓冲（读线程写入，前端轮询读取）
    pub logs: Mutex<HashMap<String, LogBuffer>>,
    /// 正在做「启动确认」的项目 id：巡检的 reap_dead 不去回收它们，
    /// 由确认流程自己观察并报告结果（否则进程一退出就可能被抢先收走，确认流程会误报「被停止」）
    pub verifying: Mutex<std::collections::HashSet<String>>,
    /// project_id -> 启动锁：同一个项目同一时刻只允许一个启动流程（含等待依赖）在跑，
    /// 「本组启动」「托盘」「恢复」等入口并发发起启动时，靠它保证顺序和去重
    pub start_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// project_id -> 最近一次计算的 git 状态（后台线程刷新）
    pub git_status: Mutex<HashMap<String, crate::commands::git::GitStatus>>,
    /// project_id -> 正在运行的脚本任务
    pub jobs: Mutex<HashMap<String, Job>>,
    /// 启动时清理掉的上次残留进程（项目名），告知前端后清空
    pub orphans_cleaned: Mutex<Vec<String>>,
    /// 尚未被前端取走的进程退出通知
    pub exit_events: Mutex<Vec<ExitEvent>>,
    /// 正在退出：冻结「最近运行集合」的记录，避免停止项目的过程把它覆盖成空
    pub quitting: AtomicBool,
    /// 用户已确认退出（或收到终止信号），可以放行 ExitRequested
    pub quit_confirmed: AtomicBool,
    /// 启动时发现上次有项目在运行、且还没问过用户是否恢复
    pub restore_pending: AtomicBool,
}

impl AppState {
    pub fn new() -> Self {
        let config_path = default_config_path();
        let config = load_config(&config_path);
        let restore_pending = config
            .last_running
            .iter()
            .any(|id| config.projects.iter().any(|p| &p.id == id));
        let mut st = Self::with_config(config, config_path);
        st.restore_pending = AtomicBool::new(restore_pending);
        st
    }

    fn with_config(config: Config, config_path: std::path::PathBuf) -> Self {
        AppState {
            config: Mutex::new(config),
            config_path,
            procs: Mutex::new(HashMap::new()),
            logs: Mutex::new(HashMap::new()),
            verifying: Mutex::new(std::collections::HashSet::new()),
            start_locks: Mutex::new(HashMap::new()),
            git_status: Mutex::new(HashMap::new()),
            jobs: Mutex::new(HashMap::new()),
            orphans_cleaned: Mutex::new(Vec::new()),
            exit_events: Mutex::new(Vec::new()),
            quitting: AtomicBool::new(false),
            quit_confirmed: AtomicBool::new(false),
            restore_pending: AtomicBool::new(false),
        }
    }

    /// 测试用：用给定配置构造状态，配置文件写到临时目录
    #[cfg(test)]
    pub fn for_test(config: Config) -> Self {
        // 每个测试状态用独立的全新目录：配置和日志文件都落在里面，互不串味，也不会读到上次运行的残留
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "devbox-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        Self::with_config(config, dir.join("config.json"))
    }

    /// 获取（必要时创建）某项目的启动锁
    pub fn start_lock(&self, project_id: &str) -> Arc<Mutex<()>> {
        self.start_locks
            .lock()
            .unwrap()
            .entry(project_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// 把当前配置写回磁盘
    pub fn persist(&self) -> Result<(), String> {
        let cfg = self.config.lock().unwrap();
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(&*cfg).map_err(|e| e.to_string())?;
        std::fs::write(&self.config_path, json).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 日志文件所在目录（与配置文件同级的 logs/）
    pub fn log_dir(&self) -> std::path::PathBuf {
        self.config_path.parent().map(|p| p.to_path_buf()).unwrap_or_default().join("logs")
    }

    /// 获取（必要时创建）某项目的日志仓；首次创建时会载入上次运行留下的历史日志
    pub fn log_buffer(&self, project_id: &str) -> LogBuffer {
        let mut logs = self.logs.lock().unwrap();
        logs.entry(project_id.to_string())
            .or_insert_with(|| Arc::new(LogStore::new(Some(self.log_dir().join(file_name_for(project_id))))))
            .clone()
    }
}

fn default_config_path() -> std::path::PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    base.join("devbox").join("config.json")
}

fn load_config(path: &std::path::Path) -> Config {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => Config::default(),
    }
}
