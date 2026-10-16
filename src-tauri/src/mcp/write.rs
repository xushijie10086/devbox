//! 会改变状态的工具：启停、脚本、环境切换（标准档），以及影响范围更大的操作（完全档）。

use super::rpc::Ctx;
use super::tools::*;
use crate::commands::{envs, git, hosts, ports, process, projects, scripts, services};
use crate::models::{HostEntry, Project};
use crate::state::AppState;
use serde_json::{json, Value};
use std::time::Duration;

/// 在后台线程里跑一个可能很久的任务，最多等 wait。超时返回 None，任务继续跑。
/// 没有应用句柄时（单元测试）退化成就地同步执行。
fn run_with_deadline<T: Send + 'static>(
    state: &AppState,
    wait: Duration,
    f: impl FnOnce(&AppState) -> T + Send + 'static,
) -> Option<T> {
    use tauri::Manager;
    match crate::state::APP_HANDLE.get() {
        Some(app) => {
            let app = app.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let state = app.state::<AppState>();
                let _ = tx.send(f(state.inner()));
            });
            rx.recv_timeout(wait).ok()
        }
        None => Some(f(state)),
    }
}

fn is_running(state: &AppState, id: &str) -> bool {
    state.procs.lock().unwrap().contains_key(id)
}

/// 启动后的状态一行 + 结构化数据
fn after(state: &AppState, p: &Project) -> (String, Value) {
    let all = state.config.lock().unwrap().projects.clone();
    let st = status_of(state, &p.id);
    (status_line(p, st.as_ref()), project_brief(p, st.as_ref(), &all))
}

pub fn start_project(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let state = ctx.state;
    let p = find_project(state, args)?;
    if is_running(state, &p.id) {
        let (line, data) = after(state, &p);
        return Ok(ToolOut::new(format!("「{}」已经在运行，无需再启动。\n{line}", p.name), json!({ "outcome": "already_running", "project": data })));
    }
    let outcome = process::start_with_deps(&p.id, state)
        .map_err(|e| format!("{e}\n（用 devbox_project_logs 并设 level=error 可以看到更多细节）"))?;
    let (line, data) = after(state, &p);
    let mut text = format!("{}\n{line}", outcome.message);
    if outcome.level == "warning" {
        text = format!("⚠ {text}");
    }
    Ok(ToolOut::new(text, json!({ "outcome": outcome.level, "message": outcome.message, "project": data })))
}

pub fn stop_project(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let state = ctx.state;
    let p = find_project(state, args)?;
    let was = is_running(state, &p.id);
    process::stop_project_inner(&p.id, state)?;
    Ok(ToolOut::new(
        if was { format!("已停止「{}」", p.name) } else { format!("「{}」本来就没有在运行", p.name) },
        json!({ "project": p.name, "was_running": was }),
    ))
}

pub fn restart_project(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let state = ctx.state;
    let p = find_project(state, args)?;
    process::stop_project_inner(&p.id, state)?;
    std::thread::sleep(Duration::from_millis(600));
    let outcome = process::start_with_deps(&p.id, state)
        .map_err(|e| format!("已停止，但重新启动失败：{e}\n（用 devbox_project_logs 并设 level=error 可以看到更多细节）"))?;
    let (line, data) = after(state, &p);
    Ok(ToolOut::new(format!("已重启「{}」：{}\n{line}", p.name, outcome.message), json!({ "outcome": outcome.level, "project": data })))
}

fn group_members(state: &AppState, group: &str) -> Result<Vec<Project>, String> {
    let cfg = state.config.lock().unwrap();
    let members: Vec<Project> = cfg
        .projects
        .iter()
        .filter(|p| if group == "未分组" { p.group.is_none() } else { p.group.as_deref() == Some(group) })
        .cloned()
        .collect();
    if members.is_empty() {
        let names = cfg.project_groups.join("、");
        return Err(format!("项目组「{group}」不存在或没有项目。现有项目组：{}", if names.is_empty() { "（无）" } else { &names }));
    }
    Ok(members)
}

pub fn start_group(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let state = ctx.state;
    let group = A(args).req("group")?;
    let members = group_members(state, group)?;
    let mut lines = Vec::new();
    let mut rows = Vec::new();
    let mut failed = 0;
    for p in &members {
        if is_running(state, &p.id) {
            lines.push(format!("- {}：已在运行", p.name));
            rows.push(json!({ "project": p.name, "result": "already_running" }));
            continue;
        }
        match process::start_with_deps(&p.id, state) {
            Ok(o) => {
                lines.push(format!("✓ {}：{}", p.name, o.message.lines().next().unwrap_or("")));
                rows.push(json!({ "project": p.name, "result": o.level, "message": o.message }));
            }
            Err(e) => {
                failed += 1;
                lines.push(format!("✗ {}：{}", p.name, e.lines().next().unwrap_or("")));
                rows.push(json!({ "project": p.name, "result": "failed", "message": e }));
            }
        }
    }
    let text = format!("启动项目组「{group}」：{} 个成功，{failed} 个失败\n{}", members.len() - failed, lines.join("\n"));
    if failed == members.len() {
        return Err(text);
    }
    Ok(ToolOut::new(text, json!({ "group": group, "results": rows })))
}

pub fn stop_group(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let state = ctx.state;
    let group = A(args).req("group")?;
    let members = group_members(state, group)?;
    let running: Vec<&Project> = members.iter().filter(|p| is_running(state, &p.id)).collect();
    std::thread::scope(|s| {
        for p in &running {
            s.spawn(move || {
                let _ = process::stop_project_inner(&p.id, state);
            });
        }
    });
    let names: Vec<&str> = running.iter().map(|p| p.name.as_str()).collect();
    Ok(ToolOut::new(
        if names.is_empty() { format!("项目组「{group}」里没有运行中的项目") } else { format!("已停止项目组「{group}」的 {} 个项目：{}", names.len(), names.join("、")) },
        json!({ "group": group, "stopped": names }),
    ))
}

pub fn stop_all(ctx: &Ctx, _args: &Value) -> Result<ToolOut, String> {
    let n = process::stop_all(ctx.state);
    Ok(ToolOut::new(if n == 0 { "没有运行中的项目".to_string() } else { format!("已停止 {n} 个项目") }, json!({ "stopped": n })))
}

pub fn clear_logs(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let p = find_project(ctx.state, args)?;
    ctx.state.log_buffer(&p.id).clear();
    Ok(ToolOut::text(format!("已清空「{}」的日志", p.name)))
}

pub fn set_env(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let state = ctx.state;
    let p = find_project(state, args)?;
    let env = A(args).str("env").map(|s| s.to_string());
    let mut message = envs::set_project_env_inner(&p.id, env.clone(), state)?;
    let running = is_running(state, &p.id);
    if running {
        message.push_str("\n项目正在运行，需要 devbox_restart_project 重启后才会用上新环境");
    }
    Ok(ToolOut::new(message, json!({ "project": p.name, "environment": env, "restart_needed": running })))
}

pub fn run_script(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let state = ctx.state;
    let a = A(args);
    let p = find_project(state, args)?;
    let want = a.req("script")?;
    let list = scripts::list_scripts_inner(&p.id, state)?;
    let lower = want.to_lowercase();
    let entry = list
        .iter()
        .find(|e| e.label == want)
        .or_else(|| list.iter().find(|e| e.command == want))
        .or_else(|| list.iter().find(|e| e.label.to_lowercase() == lower))
        .or_else(|| {
            let hits: Vec<_> = list.iter().filter(|e| e.label.to_lowercase().contains(&lower)).collect();
            if hits.len() == 1 { Some(hits[0]) } else { None }
        })
        .ok_or_else(|| {
            // 只允许运行 DevBox 从项目文件里识别出来的脚本，不执行任意命令
            format!(
                "「{want}」不是「{}」里识别出的脚本。可用脚本：{}",
                p.name,
                if list.is_empty() { "（无）".to_string() } else { list.iter().map(|e| e.label.as_str()).collect::<Vec<_>>().join("、") }
            )
        })?
        .clone();
    let wait = Duration::from_secs(a.clamp("wait_secs", 60, 1, 300));
    let (id, command, label) = (p.id.clone(), entry.command.clone(), entry.label.clone());
    let finished = run_with_deadline(state, wait, move |st| scripts::run_script_blocking(st, &id, &command, &label));
    match finished {
        Some(Ok(o)) => Ok(ToolOut::new(format!("脚本「{}」（{}）：{}", entry.label, entry.command, o.message), json!({ "script": entry.label, "finished": true, "outcome": o.level, "message": o.message }))),
        Some(Err(e)) => Err(format!("脚本「{}」失败：{e}", entry.label)),
        None => Ok(ToolOut::new(
            format!("脚本「{}」已运行 {} 秒仍未结束，继续在后台执行。用 devbox_project_logs 看输出（since_seq 增量），不需要了就 devbox_cancel_script。", entry.label, wait.as_secs()),
            json!({ "script": entry.label, "finished": false }),
        )),
    }
}

pub fn cancel_script(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let p = find_project(ctx.state, args)?;
    scripts::cancel_job(ctx.state, &p.id)?;
    Ok(ToolOut::text(format!("已取消「{}」正在运行的脚本", p.name)))
}

pub fn git_fetch(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let p = find_project(ctx.state, args)?;
    let o = git::git_fetch_inner(&p.id, ctx.state)?;
    Ok(ToolOut::text(format!("{}：{}", p.name, o.message)))
}

pub fn service(ctx: &Ctx, args: &Value, action: &str) -> Result<ToolOut, String> {
    let _ = ctx;
    let name = A(args).req("name")?.to_string();
    match action {
        "start" => services::start_service(name.clone())?,
        "stop" => services::stop_service(name.clone())?,
        _ => services::restart_service(name.clone())?,
    }
    let done = match action {
        "start" => "启动",
        "stop" => "停止",
        _ => "重启",
    };
    Ok(ToolOut::new(format!("已{done}服务 {name}"), json!({ "service": name, "action": action })))
}

// ---------------------------------------------------------------- 完全档

pub fn kill_port(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let state = ctx.state;
    let a = A(args);
    let port = match a.num("port") {
        Some(p) => Some(u16::try_from(p).map_err(|_| "端口号必须在 1-65535 之间".to_string())?),
        None => None,
    };
    let pid = a.num("pid").map(|p| u32::try_from(p).map_err(|_| "pid 太大".to_string())).transpose()?;
    let me = std::process::id();
    let guard = |pid: u32| -> Result<(), String> {
        if pid <= 1 || pid == me {
            return Err(format!("拒绝结束 pid {pid}：那是系统进程或 DevBox 自己"));
        }
        Ok(())
    };

    let mut killed: Vec<Value> = Vec::new();
    match (pid, port) {
        (Some(pid), port) => {
            guard(pid)?;
            ports::kill_process(pid, port)?;
            killed.push(json!({ "pid": pid, "port": port }));
        }
        (None, Some(port)) => {
            let holders = ports::who_holds(state, port);
            if holders.is_empty() {
                return Ok(ToolOut::new(format!("端口 {port} 本来就是空闲的"), json!({ "killed": [] })));
            }
            for h in &holders {
                guard(h.pid)?;
            }
            for h in &holders {
                ports::kill_process(h.pid, Some(port))?;
                killed.push(json!({ "pid": h.pid, "process": h.process, "project": h.project }));
            }
        }
        (None, None) => return Err("需要 port 或 pid 其中之一".into()),
    }
    let text = format!(
        "已结束：{}",
        killed
            .iter()
            .map(|k| format!("pid {}{}", k["pid"], k["process"].as_str().map_or(String::new(), |p| format!("（{p}）"))))
            .collect::<Vec<_>>()
            .join("、")
    );
    Ok(ToolOut::new(text, json!({ "killed": killed })))
}

/// hosts 的字段会被原样写进系统文件，换行、空白、注释符都不能混进去
pub(super) fn validate_host_entry(e: &HostEntry) -> Result<(), String> {
    if e.ip.parse::<std::net::IpAddr>().is_err() {
        return Err(format!("「{}」不是合法的 ip 地址", e.ip));
    }
    if e.hostnames.is_empty() {
        return Err("hostnames 不能为空".into());
    }
    for h in &e.hostnames {
        let ok = !h.is_empty()
            && h.len() <= 253
            && h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '*'));
        if !ok {
            return Err(format!("域名「{h}」不合法：只能包含字母、数字、- . _ *"));
        }
    }
    if let Some(c) = &e.comment {
        if c.chars().any(|c| c.is_control()) {
            return Err("备注里不能有换行等控制字符".into());
        }
    }
    Ok(())
}

pub fn edit_hosts(_ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let a = A(args);
    let action = a.req("action")?;
    let hostname = a.req("hostname")?.to_string();
    let mut list = hosts::get_hosts()?;
    let msg = match action {
        "add" => {
            let entry = HostEntry {
                ip: a.str("ip").unwrap_or("127.0.0.1").to_string(),
                hostnames: vec![hostname.clone()],
                enabled: true,
                comment: a.str("comment").map(|s| s.to_string()),
            };
            validate_host_entry(&entry)?;
            if list.iter().any(|e| e.ip == entry.ip && e.hostnames.contains(&hostname)) {
                return Err(format!("{} {hostname} 已经存在", entry.ip));
            }
            let msg = format!("已添加 {} {hostname}", entry.ip);
            list.push(entry);
            msg
        }
        "remove" => {
            let before = list.len();
            let mut touched = false;
            for e in list.iter_mut() {
                if e.hostnames.contains(&hostname) {
                    e.hostnames.retain(|h| h != &hostname);
                    touched = true;
                }
            }
            list.retain(|e| !e.hostnames.is_empty());
            if !touched {
                return Err(format!("DevBox 管理的 hosts 里没有 {hostname}"));
            }
            format!("已删除 {hostname}（{} → {} 条记录）", before, list.len())
        }
        "enable" | "disable" => {
            let enable = action == "enable";
            let mut touched = 0;
            for e in list.iter_mut().filter(|e| e.hostnames.contains(&hostname)) {
                e.enabled = enable;
                touched += 1;
            }
            if touched == 0 {
                return Err(format!("DevBox 管理的 hosts 里没有 {hostname}"));
            }
            format!("已{} {hostname}（{touched} 条记录；同一行上的其它域名一起生效）", if enable { "启用" } else { "停用" })
        }
        other => return Err(format!("action 只能是 add / remove / enable / disable，收到「{other}」")),
    };
    hosts::save_hosts(list.clone())?;
    Ok(ToolOut::new(msg, json!({ "entries": serde_json::to_value(&list).unwrap_or(Value::Null) })))
}

pub fn save_hosts(_ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let raw = args.get("entries").cloned().ok_or("缺少参数 entries")?;
    let entries: Vec<HostEntry> = serde_json::from_value(raw).map_err(|e| format!("entries 格式不对：{e}"))?;
    for e in &entries {
        validate_host_entry(e)?;
    }
    let n = entries.len();
    hosts::save_hosts(entries)?;
    Ok(ToolOut::new(format!("已保存 {n} 条 hosts 记录"), json!({ "saved": n })))
}

/// 名称或 id 列表 → id 列表
fn resolve_deps(state: &AppState, names: &Value, self_id: Option<&str>) -> Result<Vec<String>, String> {
    let Some(arr) = names.as_array() else { return Err("depends_on 必须是数组".into()) };
    let mut ids = Vec::new();
    for v in arr {
        let key = v.as_str().ok_or("depends_on 里每项都应是项目名称或 id")?;
        let dep = lookup_project(state, key)?;
        if Some(dep.id.as_str()) == self_id {
            return Err("项目不能依赖它自己".into());
        }
        if !ids.contains(&dep.id) {
            ids.push(dep.id);
        }
    }
    Ok(ids)
}

fn env_map(v: &Value) -> Result<std::collections::HashMap<String, String>, String> {
    let obj = v.as_object().ok_or("env 必须是对象，如 {\"NODE_ENV\": \"development\"}")?;
    Ok(obj
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string())))
        .collect())
}

fn name_taken(state: &AppState, name: &str, except: Option<&str>) -> bool {
    state
        .config
        .lock()
        .unwrap()
        .projects
        .iter()
        .any(|p| Some(p.id.as_str()) != except && p.name.to_lowercase() == name.to_lowercase())
}

pub fn add_project(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let state = ctx.state;
    let a = A(args);
    let path = a.req("path")?;
    if !std::path::Path::new(path).is_absolute() {
        return Err("path 必须是绝对路径".into());
    }
    let detected = projects::detect_project(path.to_string())?;
    let dir_name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name = a.str("name").map(|s| s.to_string()).or(detected.name.clone()).filter(|s| !s.is_empty()).unwrap_or(dir_name);
    if name_taken(state, &name, None) {
        return Err(format!("已经有名为「{name}」的项目了，请换个名字（name 参数）"));
    }
    let start_command = a
        .str("start_command")
        .map(|s| s.to_string())
        .or(detected.start_command.clone())
        .ok_or("没能识别出启动命令，请通过 start_command 参数指定")?;
    let kind = a.str("kind").map(|s| s.to_string()).or(detected.kind.clone()).unwrap_or_else(|| "other".into());
    if !matches!(kind.as_str(), "frontend" | "backend" | "other") {
        return Err("kind 只能是 frontend / backend / other".into());
    }
    let port = match a.num("port") {
        Some(p) => Some(u16::try_from(p).map_err(|_| "端口号必须在 1-65535 之间".to_string())?).filter(|p| *p > 0),
        None => detected.port,
    };
    let project = Project {
        id: String::new(),
        name: name.clone(),
        path: path.to_string(),
        kind,
        start_command,
        stop_command: a.str("stop_command").map(|s| s.to_string()),
        port,
        url: a.str("url").map(|s| s.to_string()).or(detected.url.clone()),
        env: match args.get("env") {
            Some(v) => env_map(v)?,
            None => Default::default(),
        },
        editor: None,
        auto_restart: a.bool("auto_restart", false),
        group: a.str("group").map(|s| s.to_string()),
        node: detected.node.as_ref().and_then(|s| s.matched.clone()),
        java: detected.java.as_ref().and_then(|s| s.matched.clone()),
        depends_on: match args.get("depends_on") {
            Some(v) => resolve_deps(state, v, None)?,
            None => vec![],
        },
        active_env: None,
    };
    let saved = projects::save_project_inner(project, state)?;
    let (line, data) = after(state, &saved);
    Ok(ToolOut::new(format!("已添加项目「{name}」\n{line}\n启动命令：{}", saved.start_command), json!({ "project": data })))
}

pub fn update_project(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let state = ctx.state;
    let a = A(args);
    let mut p = find_project(state, args)?;
    let mut changed: Vec<&str> = Vec::new();
    // 可清空的可选字符串：传了就更新，空串表示清除
    let opt_str = |key: &str| -> Option<Option<String>> {
        args.get(key).and_then(|v| v.as_str()).map(|s| Some(s.trim().to_string()).filter(|s| !s.is_empty()))
    };

    if let Some(name) = a.str("name") {
        if name_taken(state, name, Some(&p.id)) {
            return Err(format!("已经有名为「{name}」的项目了"));
        }
        p.name = name.to_string();
        changed.push("name");
    }
    if let Some(path) = a.str("path") {
        if !std::path::Path::new(path).is_dir() {
            return Err(format!("目录不存在：{path}"));
        }
        p.path = path.to_string();
        changed.push("path");
    }
    if let Some(kind) = a.str("kind") {
        if !matches!(kind, "frontend" | "backend" | "other") {
            return Err("kind 只能是 frontend / backend / other".into());
        }
        p.kind = kind.to_string();
        changed.push("kind");
    }
    if let Some(cmd) = a.str("start_command") {
        p.start_command = cmd.to_string();
        changed.push("start_command");
    }
    if let Some(v) = opt_str("stop_command") {
        p.stop_command = v;
        changed.push("stop_command");
    }
    if a.has("port") {
        let n = a.num("port").ok_or("port 必须是数字")?;
        p.port = Some(u16::try_from(n).map_err(|_| "端口号必须在 1-65535 之间".to_string())?).filter(|p| *p > 0);
        changed.push("port");
    }
    if let Some(v) = opt_str("url") {
        p.url = v;
        changed.push("url");
    }
    if let Some(v) = opt_str("group") {
        p.group = v;
        changed.push("group");
    }
    if let Some(v) = args.get("env") {
        p.env = env_map(v)?;
        changed.push("env");
    }
    if args.get("auto_restart").is_some() {
        p.auto_restart = a.bool("auto_restart", p.auto_restart);
        changed.push("auto_restart");
    }
    if let Some(v) = args.get("depends_on") {
        p.depends_on = resolve_deps(state, v, Some(&p.id))?;
        changed.push("depends_on");
    }
    if changed.is_empty() {
        return Err("没有传任何要修改的字段".into());
    }
    let saved = projects::save_project_inner(p, state)?;
    let running = is_running(state, &saved.id);
    let mut text = format!("已更新「{}」：{}", saved.name, changed.join("、"));
    if running {
        text.push_str("\n项目正在运行，需要 devbox_restart_project 重启后才会生效");
    }
    let (_, data) = after(state, &saved);
    Ok(ToolOut::new(text, json!({ "changed": changed, "restart_needed": running, "project": data })))
}

pub fn delete_project(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let p = find_project(ctx.state, args)?;
    if !A(args).bool("confirm", false) {
        return Err(format!("删除「{}」需要显式传 confirm=true", p.name));
    }
    projects::delete_project_inner(&p.id, ctx.state)?;
    Ok(ToolOut::text(format!("已删除项目「{}」（磁盘上的代码没有动）", p.name)))
}

pub fn git_pull(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let p = find_project(ctx.state, args)?;
    let o = git::git_pull_inner(&p.id, ctx.state)?;
    Ok(ToolOut::new(format!("{}：{}", p.name, o.message), json!({ "project": p.name, "level": o.level, "message": o.message })))
}

pub fn git_checkout(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let a = A(args);
    let p = find_project(ctx.state, args)?;
    let branch = a.req("branch")?;
    let kind = if a.bool("remote", false) || branch.starts_with("origin/") { "remote" } else { "local" };
    let o = git::git_checkout_inner(&p.id, branch, kind, ctx.state)?;
    Ok(ToolOut::new(format!("{}：{}", p.name, o.message), json!({ "project": p.name, "level": o.level, "message": o.message })))
}
