//! 工具注册表：名称、说明、参数 schema、风险等级，以及分发。具体实现在 read.rs / write.rs。

use super::rpc::Ctx;
use super::{read, write};
use crate::models::{McpMode, Project, ProjectStatus};
use crate::state::AppState;
use serde_json::{json, Map, Value};
use std::sync::OnceLock;

/// 风险等级：决定在哪个权限档位下可用
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Risk {
    /// 只读取信息
    Read,
    /// 改变项目 / 服务的运行状态，但都是 DevBox 本来就在管的东西，且可逆
    Write,
    /// 影响范围超出 DevBox 管理的项目：结束任意进程、改系统 hosts、增删改项目配置、改工作区代码
    Danger,
}

impl Risk {
    pub fn as_str(self) -> &'static str {
        match self {
            Risk::Read => "read",
            Risk::Write => "write",
            Risk::Danger => "danger",
        }
    }
    pub fn min_mode(self) -> McpMode {
        match self {
            Risk::Read => McpMode::Readonly,
            Risk::Write => McpMode::Standard,
            Risk::Danger => McpMode::Full,
        }
    }
    pub fn allowed_in(self, mode: McpMode) -> bool {
        match mode {
            McpMode::Readonly => self == Risk::Read,
            McpMode::Standard => self != Risk::Danger,
            McpMode::Full => true,
        }
    }
}

pub struct ToolDef {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub risk: Risk,
    /// 重复调用结果不变（停止已停止的项目之类）
    pub idempotent: bool,
    pub schema: Value,
}

/// 工具的返回：text 给模型读，data 是同样信息的结构化版本
pub struct ToolOut {
    pub text: String,
    pub data: Value,
}

impl ToolOut {
    pub fn new(text: impl Into<String>, data: Value) -> Self {
        ToolOut { text: text.into(), data }
    }
    pub fn text(text: impl Into<String>) -> Self {
        ToolOut { text: text.into(), data: Value::Null }
    }
}

// ---------------------------------------------------------------- 参数

/// 读取调用参数的小工具：宽松地接受模型常见的写法（数字写成字符串、空串当没传）
pub struct A<'a>(pub &'a Value);

impl<'a> A<'a> {
    pub fn str(&self, key: &str) -> Option<&'a str> {
        self.0.get(key).and_then(|v| v.as_str()).map(|s| s.trim()).filter(|s| !s.is_empty())
    }
    pub fn req(&self, key: &str) -> Result<&'a str, String> {
        self.str(key).ok_or_else(|| format!("缺少参数 {key}"))
    }
    pub fn num(&self, key: &str) -> Option<u64> {
        match self.0.get(key)? {
            Value::Number(n) => n.as_u64().or_else(|| n.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64)),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        }
    }
    pub fn bool(&self, key: &str, default: bool) -> bool {
        match self.0.get(key) {
            Some(Value::Bool(b)) => *b,
            Some(Value::String(s)) => matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes"),
            _ => default,
        }
    }
    /// 数字参数，限定在 [min, max]，缺省取 default
    pub fn clamp(&self, key: &str, default: u64, min: u64, max: u64) -> u64 {
        self.num(key).unwrap_or(default).clamp(min, max)
    }
    pub fn has(&self, key: &str) -> bool {
        self.0.get(key).is_some()
    }
}

/// 调用参数里值得记进调用记录的「对象」：项目 / 端口 / 服务 / 路径…
pub fn target_of(args: &Value) -> Option<String> {
    let a = A(args);
    for k in ["project", "id", "name", "group", "path"] {
        if let Some(s) = a.str(k) {
            return Some(s.chars().take(80).collect());
        }
    }
    for k in ["port", "pid"] {
        if let Some(n) = a.num(k) {
            return Some(format!("{k} {n}"));
        }
    }
    None
}

/// 按 id → 名称 → 忽略大小写的名称 → 唯一的名称片段 查找项目。
/// 参数名 project；兼容旧版的 id / name。
pub fn find_project(state: &AppState, args: &Value) -> Result<Project, String> {
    let a = A(args);
    let key = a
        .str("project")
        .or_else(|| a.str("id"))
        .or_else(|| a.str("name"))
        .or_else(|| a.str("project_id"))
        .ok_or("缺少参数 project（项目名称或 id，可用 devbox_list_projects 查看）")?;
    lookup_project(state, key)
}

pub fn lookup_project(state: &AppState, key: &str) -> Result<Project, String> {
    let cfg = state.config.lock().unwrap();
    let ps = &cfg.projects;
    if let Some(p) = ps.iter().find(|p| p.id == key).or_else(|| ps.iter().find(|p| p.name == key)) {
        return Ok(p.clone());
    }
    let lower = key.to_lowercase();
    if let Some(p) = ps.iter().find(|p| p.name.to_lowercase() == lower) {
        return Ok(p.clone());
    }
    let partial: Vec<&Project> = ps.iter().filter(|p| p.name.to_lowercase().contains(&lower)).collect();
    match partial.as_slice() {
        [one] => Ok((*one).clone()),
        [] => {
            let names: Vec<&str> = ps.iter().map(|p| p.name.as_str()).collect();
            Err(if names.is_empty() {
                format!("找不到项目「{key}」：DevBox 里还没有任何项目")
            } else {
                format!("找不到项目「{key}」。现有项目：{}", names.join("、"))
            })
        }
        many => Err(format!(
            "「{key}」匹配到多个项目：{}。请写完整名称",
            many.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join("、")
        )),
    }
}

/// 所有项目的运行状态，按 id 索引
pub fn statuses(state: &AppState) -> std::collections::HashMap<String, ProjectStatus> {
    crate::commands::process::compute_statuses(state).into_iter().map(|s| (s.id.clone(), s)).collect()
}

pub fn status_of(state: &AppState, id: &str) -> Option<ProjectStatus> {
    statuses(state).remove(id)
}

pub fn fmt_secs(s: u64) -> String {
    match s {
        0..=59 => format!("{s} 秒"),
        60..=3599 => format!("{} 分钟", s / 60),
        _ => format!("{} 小时 {} 分钟", s / 3600, s % 3600 / 60),
    }
}

/// 一行文字描述项目 + 运行状态
pub fn status_line(p: &Project, st: Option<&ProjectStatus>) -> String {
    let mut s = String::new();
    let running = st.is_some_and(|s| s.running);
    s.push_str(if running { "● " } else { "○ " });
    s.push_str(&p.name);
    let mut tags = vec![p.kind.clone()];
    if let Some(g) = &p.group {
        tags.push(g.clone());
    }
    s.push_str(&format!(" [{}]", tags.join(" / ")));
    match st {
        Some(st) if st.running => {
            s.push_str(&format!(" 运行中 pid {}", st.pid.unwrap_or(0)));
            if let Some(u) = st.uptime_secs {
                s.push_str(&format!("，已运行 {}", fmt_secs(u)));
            }
            if let Some(port) = p.port {
                s.push_str(&format!("，端口 {port} {}", if st.port_up == Some(true) { "已监听" } else { "未监听" }));
            }
            if let (Some(cpu), Some(mem)) = (st.cpu, st.memory_mb) {
                s.push_str(&format!("，CPU {cpu:.0}% 内存 {mem}MB"));
            }
        }
        _ => {
            s.push_str(" 未运行");
            if let Some(port) = p.port {
                s.push_str(&format!("（期望端口 {port}）"));
            }
        }
    }
    if let Some(j) = st.and_then(|s| s.job.as_ref()) {
        s.push_str(&format!("，脚本「{j}」运行中"));
    }
    if let Some(e) = &p.active_env {
        s.push_str(&format!("，环境 {e}"));
    }
    s
}

/// 项目的结构化简介（不含环境变量的值：那里常放密码）
pub fn project_brief(p: &Project, st: Option<&ProjectStatus>, all: &[Project]) -> Value {
    let deps: Vec<&str> = p
        .depends_on
        .iter()
        .filter_map(|d| all.iter().find(|x| &x.id == d))
        .map(|x| x.name.as_str())
        .collect();
    json!({
        "id": p.id,
        "name": p.name,
        "kind": p.kind,
        "group": p.group,
        "path": p.path,
        "start_command": p.start_command,
        "port": p.port,
        "url": p.url,
        "running": st.is_some_and(|s| s.running),
        "pid": st.and_then(|s| s.pid),
        "uptime_secs": st.and_then(|s| s.uptime_secs),
        "cpu_percent": st.and_then(|s| s.cpu),
        "memory_mb": st.and_then(|s| s.memory_mb),
        "port_listening": st.and_then(|s| s.port_up),
        "running_script": st.and_then(|s| s.job.clone()),
        "environment": p.active_env,
        "depends_on": deps,
    })
}

// ---------------------------------------------------------------- schema 构造

pub fn s(desc: &str) -> Value {
    json!({ "type": "string", "description": desc })
}
pub fn n(desc: &str) -> Value {
    json!({ "type": "integer", "description": desc })
}
pub fn b(desc: &str) -> Value {
    json!({ "type": "boolean", "description": desc })
}
pub fn en(desc: &str, values: &[&str]) -> Value {
    json!({ "type": "string", "description": desc, "enum": values })
}
pub fn obj(props: Vec<(&str, Value)>, required: &[&str]) -> Value {
    let mut m = Map::new();
    for (k, v) in props {
        m.insert(k.to_string(), v);
    }
    json!({ "type": "object", "properties": m, "required": required, "additionalProperties": false })
}
/// 「project」参数的统一写法
pub fn project_prop() -> (&'static str, Value) {
    ("project", s("项目名称（或 id）。名称不区分大小写，唯一的片段也能匹配；不确定时先 devbox_list_projects"))
}

fn def(name: &'static str, title: &'static str, risk: Risk, idempotent: bool, description: &'static str, schema: Value) -> ToolDef {
    ToolDef { name, title, description, risk, idempotent, schema }
}

pub fn all() -> &'static [ToolDef] {
    static ALL: OnceLock<Vec<ToolDef>> = OnceLock::new();
    ALL.get_or_init(build_all)
}

pub fn find(name: &str) -> Option<&'static ToolDef> {
    all().iter().find(|t| t.name == name)
}

/// 当前档位可用的工具，渲染成 tools/list 的条目
pub fn list(mode: McpMode) -> Vec<Value> {
    all()
        .iter()
        .filter(|t| t.risk.allowed_in(mode))
        .map(|t| {
            json!({
                "name": t.name,
                "title": t.title,
                "description": t.description,
                "inputSchema": t.schema,
                "annotations": {
                    "title": t.title,
                    "readOnlyHint": t.risk == Risk::Read,
                    "destructiveHint": t.risk == Risk::Danger,
                    "idempotentHint": t.idempotent,
                    "openWorldHint": false,
                },
            })
        })
        .collect()
}

pub fn call(ctx: &Ctx, name: &str, args: &Value) -> Result<ToolOut, String> {
    match name {
        // ---- 只读 ----
        "devbox_overview" => read::overview(ctx, args),
        "devbox_list_projects" => read::list_projects(ctx, args),
        "devbox_get_project" => read::get_project(ctx, args),
        "devbox_project_status" => read::project_status(ctx, args),
        "devbox_project_logs" => read::project_logs(ctx, args),
        "devbox_wait_for_log" => read::wait_for_log(ctx, args),
        "devbox_recent_exits" => read::recent_exits(ctx, args),
        "devbox_list_groups" => read::list_groups(ctx, args),
        "devbox_list_scripts" => read::list_scripts(ctx, args),
        "devbox_project_envs" => read::project_envs(ctx, args),
        "devbox_git_status" => read::git_status(ctx, args),
        "devbox_git_branches" => read::git_branches(ctx, args),
        "devbox_list_runtimes" => read::list_runtimes(ctx, args),
        "devbox_list_ports" => read::list_ports(ctx, args),
        "devbox_check_port" => read::check_port(ctx, args),
        "devbox_get_hosts" => read::get_hosts(ctx, args),
        "devbox_list_services" => read::list_services(ctx, args),
        "devbox_detect_project" => read::detect_project(ctx, args),
        // ---- 启停与日常操作 ----
        "devbox_start_project" => write::start_project(ctx, args),
        "devbox_stop_project" => write::stop_project(ctx, args),
        "devbox_restart_project" => write::restart_project(ctx, args),
        "devbox_start_group" => write::start_group(ctx, args),
        "devbox_stop_group" => write::stop_group(ctx, args),
        "devbox_stop_all" => write::stop_all(ctx, args),
        "devbox_clear_logs" => write::clear_logs(ctx, args),
        "devbox_set_env" => write::set_env(ctx, args),
        "devbox_run_script" => write::run_script(ctx, args),
        "devbox_cancel_script" => write::cancel_script(ctx, args),
        "devbox_git_fetch" => write::git_fetch(ctx, args),
        "devbox_start_service" => write::service(ctx, args, "start"),
        "devbox_stop_service" => write::service(ctx, args, "stop"),
        "devbox_restart_service" => write::service(ctx, args, "restart"),
        // ---- 危险操作 ----
        "devbox_kill_port" => write::kill_port(ctx, args),
        "devbox_edit_hosts" => write::edit_hosts(ctx, args),
        "devbox_save_hosts" => write::save_hosts(ctx, args),
        "devbox_add_project" => write::add_project(ctx, args),
        "devbox_update_project" => write::update_project(ctx, args),
        "devbox_delete_project" => write::delete_project(ctx, args),
        "devbox_git_pull" => write::git_pull(ctx, args),
        "devbox_git_checkout" => write::git_checkout(ctx, args),
        _ => Err(format!("工具「{name}」没有实现")),
    }
}

fn build_all() -> Vec<ToolDef> {
    use Risk::*;
    let log_filter_props = || {
        vec![
            project_prop(),
            ("tail", n("取末尾多少行，默认 200，最多 2000（过滤之后再取末尾）")),
            ("since_seq", n("只要序号大于它的行。用上次返回的 next_since 就能只拿新增的日志")),
            ("level", en("最低级别：all（默认）/ warn（含错误）/ error。error 会自动带上错误后面的堆栈行", &["all", "warn", "error"])),
            ("grep", s("关键词，不区分大小写；用 | 分隔多个，命中任意一个即可，如 \"exception|timeout\"")),
            ("exclude", s("排除含这些关键词的行，同样用 | 分隔")),
            ("stream", en("只看某个输出流", &["stdout", "stderr", "system"])),
            ("context", n("命中行前后各带几行上下文（0-10，默认 0），配合 grep / level 使用")),
            ("max_chars", n("返回内容的总字符数上限，默认 30000；超出时保留最新的")),
        ]
    };
    vec![
        // ------------------------------------------------------------ 只读
        def("devbox_overview", "总览", Read, true,
            "【先调这个】一次看清 DevBox 的全局情况：所有项目的运行状态（按项目组）、端口是否监听、最近崩溃的项目、当前 MCP 权限档位。",
            obj(vec![], &[])),
        def("devbox_list_projects", "项目列表", Read, true,
            "列出 DevBox 里登记的项目及运行状态。可按项目组、类型、是否在运行、名称关键词筛选。",
            obj(vec![
                ("group", s("只看某个项目组；写「未分组」查没有分组的项目")),
                ("kind", en("项目类型", &["frontend", "backend", "other"])),
                ("running", b("true 只看运行中的，false 只看没运行的")),
                ("query", s("名称或路径包含的关键词")),
            ], &[])),
        def("devbox_get_project", "项目详情", Read, true,
            "一个项目的完整信息：配置（启动命令、端口、依赖、环境变量的名字——值不会返回）、运行状态、git 分支与领先/落后、可选环境、日志概况；端口被别的进程占着时一并告诉你是谁。",
            obj(vec![project_prop()], &["project"])),
        def("devbox_project_status", "运行状态", Read, true,
            "项目的运行状态：是否在运行、pid、运行时长、CPU / 内存、期望端口是否已监听、是否有脚本在跑。不传 project 返回全部。",
            obj(vec![("project", s("项目名称或 id；不传则返回所有项目"))], &[])),
        def("devbox_project_logs", "项目日志", Read, true,
            "读取项目的运行日志（已去掉颜色代码）。排查问题的首选：level=error 只看报错和堆栈；grep 搜关键词并用 context 带上前后文；since_seq 增量拉取只拿新增的行。返回里的 next_since 用于下一次增量拉取。",
            obj(log_filter_props(), &["project"])),
        def("devbox_wait_for_log", "等待日志出现", Read, true,
            "等项目的日志里出现指定内容后再返回，用来确认「启动完成」「编译成功」「出现报错」，不必反复轮询。默认只看调用之后新产生的日志。进程中途退出会立刻返回，不会白等到超时。",
            obj(vec![
                project_prop(),
                ("pattern", s("要等的关键词，不区分大小写；用 | 分隔多个，命中任意一个即可，如 \"started|listening on\"")),
                ("level", en("改为等某个级别的行出现（可与 pattern 同时用）", &["warn", "error"])),
                ("timeout_secs", n("最长等多少秒，默认 30，最多 120")),
                ("since_seq", n("从哪个序号之后开始看，默认只看之后新产生的")),
            ], &["project"])),
        def("devbox_recent_exits", "最近的崩溃", Read, true,
            "最近自行退出（崩溃）的项目，含退出原因和最后几行输出。DevBox 运行期间有效，应用重启后清空。",
            obj(vec![("limit", n("最多返回几条，默认 10"))], &[])),
        def("devbox_list_groups", "项目组", Read, true,
            "项目组列表及每组的项目数、运行数。配合 devbox_start_group / devbox_stop_group 一键启停一组。",
            obj(vec![], &[])),
        def("devbox_list_scripts", "可运行的脚本", Read, true,
            "项目目录里识别出的一次性脚本（build / test / lint / install…），可用 devbox_run_script 运行。",
            obj(vec![project_prop()], &["project"])),
        def("devbox_project_envs", "项目环境", Read, true,
            "项目里自动识别出的环境（dev / test / prod…，来自 .env.*、Spring profiles、package.json 脚本等）以及当前选中的是哪个。切换用 devbox_set_env。不传 project 返回全部。",
            obj(vec![("project", s("项目名称或 id；不传则返回所有项目"))], &[])),
        def("devbox_git_status", "git 状态", Read, true,
            "项目的当前分支、领先 / 落后上游多少个提交、有多少未提交的修改。不传 project 返回全部（用缓存，不执行 git）。",
            obj(vec![("project", s("项目名称或 id；不传则返回所有项目"))], &[])),
        def("devbox_git_branches", "分支列表", Read, true,
            "项目的本地和远程分支（含最近提交）。想切分支先看这个；远程分支较新时先 devbox_git_fetch。",
            obj(vec![project_prop()], &["project"])),
        def("devbox_list_runtimes", "本机运行时", Read, true,
            "本机安装的 Node 和 JDK 版本（nvm / fnm / SDKMAN / Homebrew 等来源），项目可以指定使用其中某个版本。",
            obj(vec![], &[])),
        def("devbox_list_ports", "监听端口", Read, true,
            "本机正在监听的 TCP 端口及占用进程。scope=project（默认）只列与 DevBox 项目相关的；scope=all 列全部。",
            obj(vec![
                ("scope", en("project（默认）或 all", &["project", "all"])),
                ("query", s("按端口号或进程名 / 项目名筛选")),
            ], &[])),
        def("devbox_check_port", "端口占用检查", Read, true,
            "查某个端口被谁占用（是哪个 DevBox 项目，还是外部进程）。传 port，或传 project 查它登记的端口。启动失败提示端口被占时先用它定位。",
            obj(vec![("port", n("端口号")), ("project", s("项目名称或 id，查它登记的端口"))], &[])),
        def("devbox_get_hosts", "hosts 记录", Read, true,
            "DevBox 管理的 hosts 记录（ip、域名、是否启用）。raw=true 返回整个 hosts 文件原文。",
            obj(vec![("raw", b("true 返回完整原文"))], &[])),
        def("devbox_list_services", "brew 服务", Read, true,
            "Homebrew 管理的后台服务（mysql、redis、postgresql…）及状态。仅 macOS。",
            obj(vec![], &[])),
        def("devbox_detect_project", "识别项目目录", Read, true,
            "分析一个目录：推断项目名称、类型、启动命令、端口、要求的 Node / JDK 版本、可选环境。添加项目前先用它，再 devbox_add_project。",
            obj(vec![("path", s("项目目录的绝对路径"))], &["path"])),

        // ------------------------------------------------------------ 启停与日常操作
        def("devbox_start_project", "启动项目", Write, true,
            "启动项目并等待结果：配置了端口就等到端口开始监听（最多约 30 秒），没配端口则确认进程存活。会先按依赖顺序启动它依赖的项目。已在运行则直接返回当前状态。失败时返回原因和最后的输出。",
            obj(vec![project_prop()], &["project"])),
        def("devbox_stop_project", "停止项目", Write, true,
            "停止项目（结束整棵进程树；配置了自定义停止命令则先执行）。本来就没运行也不会报错。",
            obj(vec![project_prop()], &["project"])),
        def("devbox_restart_project", "重启项目", Write, false,
            "停止再启动项目，反馈同 devbox_start_project。改了代码、切了环境之后用它让变化生效。",
            obj(vec![project_prop()], &["project"])),
        def("devbox_start_group", "启动一组项目", Write, true,
            "按列表顺序启动某个项目组里的所有项目（各自的依赖会先启动）。已在运行的跳过。",
            obj(vec![("group", s("项目组名称，见 devbox_list_groups"))], &["group"])),
        def("devbox_stop_group", "停止一组项目", Write, true,
            "并行停止某个项目组里运行中的所有项目。",
            obj(vec![("group", s("项目组名称，见 devbox_list_groups"))], &["group"])),
        def("devbox_stop_all", "停止全部项目", Write, true,
            "停止所有运行中的项目。收工或需要腾出全部端口时用。",
            obj(vec![], &[])),
        def("devbox_clear_logs", "清空日志", Write, true,
            "清空项目日志（内存和磁盘文件一起清）。想让后面的日志只包含接下来的操作时，先清空再重启。",
            obj(vec![project_prop()], &["project"])),
        def("devbox_set_env", "切换环境", Write, true,
            "切换项目使用的环境（dev / test / prod…，名称见 devbox_project_envs）；不传 env 回到默认。只影响之后的启动，运行中的项目需要 devbox_restart_project 才生效。",
            obj(vec![project_prop(), ("env", s("环境名；不传或留空表示回到默认"))], &["project"])),
        def("devbox_run_script", "运行脚本", Write, false,
            "运行项目里的一次性脚本（install / build / test / lint…），脚本名取自 devbox_list_scripts。最多等 wait_secs 秒：期间结束就返回结果；没结束则继续在后台跑，用 devbox_project_logs 看输出、devbox_cancel_script 取消。每个项目同时只能跑一个脚本。",
            obj(vec![
                project_prop(),
                ("script", s("脚本名（list_scripts 里的 label，如 \"build\"）或完整命令")),
                ("wait_secs", n("最多等待秒数，默认 60，最多 300")),
            ], &["project", "script"])),
        def("devbox_cancel_script", "取消脚本", Write, true,
            "取消项目正在运行的脚本。",
            obj(vec![project_prop()], &["project"])),
        def("devbox_git_fetch", "git fetch", Write, true,
            "获取远程最新的分支信息（git fetch --prune），不改动工作区。",
            obj(vec![project_prop()], &["project"])),
        def("devbox_start_service", "启动 brew 服务", Write, true,
            "启动一个 Homebrew 服务（如 mysql、redis）。仅 macOS。",
            obj(vec![("name", s("服务名，见 devbox_list_services"))], &["name"])),
        def("devbox_stop_service", "停止 brew 服务", Write, true,
            "停止一个 Homebrew 服务。仅 macOS。",
            obj(vec![("name", s("服务名，见 devbox_list_services"))], &["name"])),
        def("devbox_restart_service", "重启 brew 服务", Write, false,
            "重启一个 Homebrew 服务。仅 macOS。",
            obj(vec![("name", s("服务名，见 devbox_list_services"))], &["name"])),

        // ------------------------------------------------------------ 危险操作（完全档）
        def("devbox_kill_port", "结束占用端口的进程", Danger, true,
            "结束占用某端口（或指定 pid）的进程树。会终止任何进程，包括不属于 DevBox 的；如果占用者是 DevBox 的项目，请改用 devbox_stop_project。先用 devbox_check_port 确认占用者再动手。",
            obj(vec![("port", n("端口号")), ("pid", n("进程 id；只传 port 时会结束该端口的所有占用者"))], &[])),
        def("devbox_edit_hosts", "修改 hosts（增量）", Danger, false,
            "增量修改 DevBox 管理的 hosts 记录：add 添加 / remove 删除 / enable 启用 / disable 停用。会弹出系统的管理员授权，需要用户在本机确认。",
            obj(vec![
                ("action", en("操作", &["add", "remove", "enable", "disable"])),
                ("hostname", s("域名，如 api.local")),
                ("ip", s("add 时的 ip，默认 127.0.0.1")),
                ("comment", s("add 时的备注")),
            ], &["action", "hostname"])),
        def("devbox_save_hosts", "整体保存 hosts", Danger, true,
            "用给定记录整体替换 DevBox 管理的 hosts 块（块外内容不动）。会弹出系统的管理员授权。多数情况请用 devbox_edit_hosts，避免漏掉已有记录。",
            obj(vec![(
                "entries",
                json!({
                    "type": "array",
                    "description": "完整的记录列表",
                    "items": obj(vec![
                        ("ip", s("ip 地址")),
                        ("hostnames", json!({ "type": "array", "items": { "type": "string" }, "description": "域名列表" })),
                        ("enabled", b("是否启用")),
                        ("comment", s("备注")),
                    ], &["ip", "hostnames", "enabled"]),
                }),
            )], &["entries"])),
        def("devbox_add_project", "添加项目", Danger, false,
            "把一个目录登记为 DevBox 项目。只需要 path，其余没写的字段（名称、类型、启动命令、端口、地址）会按 devbox_detect_project 的识别结果补全。",
            obj(vec![
                ("path", s("项目目录的绝对路径")),
                ("name", s("项目名称，不能与现有项目重名")),
                ("kind", en("类型", &["frontend", "backend", "other"])),
                ("start_command", s("启动命令，如 npm run dev")),
                ("stop_command", s("自定义停止命令（可选）")),
                ("port", n("期望端口")),
                ("url", s("快捷打开的地址")),
                ("group", s("所属项目组（不存在会自动创建）")),
                ("env", json!({ "type": "object", "additionalProperties": { "type": "string" }, "description": "追加的环境变量" })),
                ("auto_restart", b("崩溃后自动重启")),
                ("depends_on", json!({ "type": "array", "items": { "type": "string" }, "description": "启动前需要先就绪的项目（名称或 id）" })),
            ], &["path"])),
        def("devbox_update_project", "修改项目配置", Danger, true,
            "修改项目的配置，只改你传了的字段。修改启动命令等于改变将要执行的命令，所以需要完全档。运行中的项目需重启才生效。传 env 会整体替换环境变量。",
            obj(vec![
                project_prop(),
                ("name", s("新名称")),
                ("path", s("新的工作目录")),
                ("kind", en("类型", &["frontend", "backend", "other"])),
                ("start_command", s("启动命令")),
                ("stop_command", s("自定义停止命令；传空串清除")),
                ("port", n("期望端口；传 0 清除")),
                ("url", s("快捷打开的地址；传空串清除")),
                ("group", s("所属项目组；传空串移出分组")),
                ("env", json!({ "type": "object", "additionalProperties": { "type": "string" }, "description": "整体替换环境变量" })),
                ("auto_restart", b("崩溃后自动重启")),
                ("depends_on", json!({ "type": "array", "items": { "type": "string" }, "description": "整体替换依赖（名称或 id）" })),
            ], &["project"])),
        def("devbox_delete_project", "删除项目", Danger, true,
            "从 DevBox 里删除项目（会先停止它）。只删除登记，不动磁盘上的代码。必须传 confirm=true。",
            obj(vec![project_prop(), ("confirm", b("必须为 true"))], &["project", "confirm"])),
        def("devbox_git_pull", "git pull", Danger, false,
            "在项目目录执行 git pull --ff-only：只快进，有分叉时不会擅自合并，而是告诉你需要手动处理。会改动工作区代码。",
            obj(vec![project_prop()], &["project"])),
        def("devbox_git_checkout", "切换分支", Danger, false,
            "切换项目的 git 分支。有未提交修改且与目标分支冲突时 git 会拒绝。会改动工作区代码，运行中的项目需重启才生效。",
            obj(vec![
                project_prop(),
                ("branch", s("分支名；远程分支写成 origin/feature-x")),
                ("remote", b("是否为远程分支（名字里带 origin/ 时会自动判断）")),
            ], &["project", "branch"])),
    ]
}
