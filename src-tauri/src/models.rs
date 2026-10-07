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
    /// 所属项目组（项目库里的 tab 分类）；None 表示未分组
    #[serde(default)]
    pub group: Option<String>,
    /// 指定的 Node 版本；None 表示用系统默认
    #[serde(default)]
    pub node: Option<RuntimeChoice>,
    /// 指定的 JDK 版本；None 表示用系统默认
    #[serde(default)]
    pub java: Option<RuntimeChoice>,
}

/// 项目选定的运行时版本。version 仅用于展示；真正生效的是 path：
/// Node 为其 bin 目录，JDK 为 JAVA_HOME。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RuntimeChoice {
    pub version: String,
    pub path: String,
}

fn default_kind() -> String {
    "other".to_string()
}

/// 持久化到磁盘的配置
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Config {
    #[serde(default)]
    pub projects: Vec<Project>,
    /// 项目组列表（决定 tab 的顺序，也允许存在暂时没有项目的空组）
    #[serde(default)]
    pub project_groups: Vec<String>,
    /// 最近一次记录到的「正在运行的项目 id」。应用退出（含被强杀 / 崩溃）后，
    /// 下次启动据此提示是否恢复
    #[serde(default)]
    pub last_running: Vec<String>,
    /// 关闭系统通知（默认开启，所以字段取反，缺省即开启）
    #[serde(default)]
    pub mute_notifications: bool,
}

/// 自动探测到的项目信息（用于表单一键填充）
#[derive(Serialize, Clone, Debug, Default)]
pub struct DetectedProject {
    pub name: Option<String>,
    pub kind: Option<String>,
    pub start_command: Option<String>,
    pub port: Option<u16>,
    pub url: Option<String>,
    /// 面向用户的简短说明，例如"已根据 package.json 自动填充"
    pub summary: String,
    /// 项目声明的 Node 版本要求，以及本机能否满足
    pub node: Option<RuntimeSuggestion>,
    /// 项目声明的 JDK 版本要求，以及本机能否满足
    pub java: Option<RuntimeSuggestion>,
    /// 需要提醒用户注意的事项（如多模块 Maven 项目的启动目录）
    pub notes: Vec<String>,
}

/// 项目对运行时版本的要求，以及在本机已安装版本里匹配到的结果
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct RuntimeSuggestion {
    /// 项目声明的要求原文，如 "20"、">=18"、"1.8"
    pub wanted: String,
    /// 来自哪个文件，如 ".nvmrc"、"pom.xml"
    pub source: String,
    /// 本机匹配到的版本；None 表示本机没有满足要求的版本
    pub matched: Option<RuntimeChoice>,
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
    /// 正在运行的脚本任务名（install / build 等），没有则为 None
    pub job: Option<String>,
    pub job_secs: Option<u64>,
}

/// 项目进程在「启动确认」之后才退出（崩溃等）时留给前端的通知
#[derive(Serialize, Clone, Debug)]
pub struct ExitEvent {
    pub id: String,
    pub name: String,
    /// 退出发生的时间 HH:MM:SS
    pub ts: String,
    /// 原因 + 最后几行输出
    pub message: String,
}

/// 启动结果：level 为 "success" 或 "warning"（失败走 Err，不在这里）
#[derive(Serialize, Clone, Debug)]
pub struct StartOutcome {
    pub level: &'static str,
    pub message: String,
}

impl StartOutcome {
    pub fn success(message: String) -> Self {
        Self { level: "success", message }
    }
    pub fn warning(message: String) -> Self {
        Self { level: "warning", message }
    }
}

/// 端口占用信息（来自 lsof）
#[derive(Serialize, Clone, Debug)]
pub struct PortInfo {
    pub port: u16,
    pub pid: u32,
    pub process: String,
    pub protocol: String,
    pub address: String,
    /// 关联的项目名；与任何项目无关时为 None
    pub project: Option<String>,
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
