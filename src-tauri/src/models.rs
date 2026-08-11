use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 一个受管理的开发项目（前端 / 后端 / 其它）
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Project {
    pub id: String,
    pub name: String,
    /// 工作目录（绝对路径）
    pub path: String,
    /// "frontend" | "backend" | "other"
    #[serde(default = "default_kind")]
    pub kind: String,
    /// 启动命令，例如 "npm run dev"
    pub start_command: String,
    /// 可选的自定义停止命令；为空则由 DevBox 直接结束进程树
    #[serde(default)]
    pub stop_command: Option<String>,
    /// 期望端口（用于端口面板关联与健康检查）
    #[serde(default)]
    pub port: Option<u16>,
    /// 快捷打开的地址，例如 "http://localhost:3000"
    #[serde(default)]
    pub url: Option<String>,
    /// 追加的环境变量
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// 打开编辑器的命令，默认 "code"
    #[serde(default)]
    pub editor: Option<String>,
    /// 崩溃后是否自动重启
    #[serde(default)]
    pub auto_restart: bool,
}

fn default_kind() -> String {
    "other".to_string()
}

/// 启动组：一键拉起一批项目 + 一批 brew 服务
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Profile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub project_ids: Vec<String>,
    /// 需要一起启动的 brew 服务名，例如 ["mysql", "redis"]
    #[serde(default)]
    pub service_names: Vec<String>,
}

/// 持久化到磁盘的配置
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Config {
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default)]
    pub profiles: Vec<Profile>,
}

/// 项目的运行时状态
#[derive(Serialize, Clone, Debug)]
pub struct ProjectStatus {
    pub id: String,
    pub running: bool,
    pub pid: Option<u32>,
    pub cpu: Option<f32>,
    pub memory_mb: Option<u64>,
    pub uptime_secs: Option<u64>,
    /// 期望端口是否已监听
    pub port_up: Option<bool>,
}

/// 端口占用信息（来自 lsof）
#[derive(Serialize, Clone, Debug)]
pub struct PortInfo {
    pub port: u16,
    pub pid: u32,
    pub process: String,
    pub protocol: String,
    pub address: String,
}

/// /etc/hosts 中的一条记录
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HostEntry {
    pub ip: String,
    pub hostnames: Vec<String>,
    pub enabled: bool,
    #[serde(default)]
    pub comment: Option<String>,
}

/// brew 服务信息
#[derive(Serialize, Clone, Debug)]
pub struct ServiceInfo {
    pub name: String,
    /// started / stopped / error / none / unknown
    pub status: String,
    pub user: Option<String>,
    pub file: Option<String>,
}

/// 单行日志
#[derive(Serialize, Clone, Debug)]
pub struct LogLine {
    pub ts: String,
    /// "stdout" | "stderr" | "system"
    pub stream: String,
    pub text: String,
}
