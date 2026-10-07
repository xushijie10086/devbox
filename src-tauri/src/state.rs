use crate::models::{Config, ExitEvent, LogLine};
use std::collections::{HashMap, VecDeque};
use std::process::Child;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// 每个项目最多保留的日志行数
pub const MAX_LOG_LINES: usize = 2000;

/// 一个正在运行的项目进程
pub struct RunningProc {
    pub pid: u32,
    pub child: Child,
    pub started_at: Instant,
}

/// 应用句柄：后台线程（巡检、托盘菜单回调）要发系统通知、刷新托盘时用
pub static APP_HANDLE: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();

/// 全局应用状态
pub struct AppState {
    pub config: Mutex<Config>,
    pub config_path: std::path::PathBuf,
    /// project_id -> 运行中的进程
    pub procs: Mutex<HashMap<String, RunningProc>>,
    /// project_id -> 日志缓冲（读线程写入，前端轮询读取）
    pub logs: Mutex<HashMap<String, Arc<Mutex<VecDeque<LogLine>>>>>,
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
            exit_events: Mutex::new(Vec::new()),
            quitting: AtomicBool::new(false),
            quit_confirmed: AtomicBool::new(false),
            restore_pending: AtomicBool::new(false),
        }
    }

    /// 测试用：用给定配置构造状态，配置文件写到临时目录
    #[cfg(test)]
    pub fn for_test(config: Config) -> Self {
        let path = std::env::temp_dir().join(format!("devbox-test-config-{:?}.json", std::thread::current().id()));
        Self::with_config(config, path)
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

    /// 获取（必要时创建）某项目的日志缓冲
    pub fn log_buffer(&self, project_id: &str) -> Arc<Mutex<VecDeque<LogLine>>> {
        let mut logs = self.logs.lock().unwrap();
        logs.entry(project_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(VecDeque::with_capacity(MAX_LOG_LINES))))
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
