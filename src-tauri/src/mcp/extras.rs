//! 工具以外的 MCP 能力：服务说明（instructions）、资源（resources）、提示词（prompts）。

use super::rpc::{mode_label, Ctx};
use super::{read, Activity};
use serde_json::{json, Value};

/// initialize 时返回给客户端，会被放进模型的上下文，所以要短、要有用
pub fn instructions(ctx: &Ctx) -> String {
    format!(
        "DevBox 是用户本机的开发环境管家，管理着一组本地项目（前端 / 后端 / 数据库等）的启停、日志、端口、环境和 git。\n\
         当前权限档位：{mode}。{mode_hint}\n\n\
         建议的用法：\n\
         1. 先调 devbox_overview 了解有哪些项目、谁在运行、谁刚崩溃。项目参数 project 写名称即可（不区分大小写）。\n\
         2. 启动用 devbox_start_project：它会按依赖顺序启动，并等到端口监听或确认进程存活才返回，失败时带原因。\n\
         3. 排查问题：先 devbox_project_logs level=error，需要上下文再 grep + context；\
         要确认「启动完成 / 编译成功」用 devbox_wait_for_log，不要反复轮询；增量拉取用返回的 next_since。\n\
         4. 启动失败提示端口被占：devbox_check_port 看是谁占的；是 DevBox 项目就 stop 它，外部进程再考虑 devbox_kill_port（完全档）。\n\
         5. 项目有 dev / test / prod 等环境时，devbox_project_envs 查看、devbox_set_env 切换，再 devbox_restart_project 生效。\n\
         6. 凡是会改变状态的操作，做之前先告诉用户你要做什么；不要为了「试试看」去停用户正在用的项目。\n\
         注意：项目的环境变量值不会通过 MCP 返回；日志里可能含有敏感信息，转述时请留意。",
        mode = mode_label(ctx.mode),
        mode_hint = match ctx.mode {
            crate::models::McpMode::Readonly => "只能查看，不能启停或修改任何东西；需要的话请用户在 DevBox 的「MCP」页面调高档位。",
            crate::models::McpMode::Standard => "可以启停项目 / 服务、跑脚本、清日志、切环境；结束任意端口进程、改 hosts、增删改项目、git pull / 切分支需要用户调到「完全」档。",
            crate::models::McpMode::Full => "所有操作都可用，请格外小心 devbox_kill_port、devbox_edit_hosts、devbox_delete_project 这类影响面大的操作。",
        }
    )
}

// ---------------------------------------------------------------- 资源

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(Ok(v)) = s.get(i + 1..i + 3).map(|h| u8::from_str_radix(h, 16)) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn resources_list(ctx: &Ctx) -> Value {
    let mut list = vec![
        json!({ "uri": "devbox://overview", "name": "overview", "title": "DevBox 总览", "description": "所有项目的运行状态、最近的崩溃", "mimeType": "text/plain" }),
        json!({ "uri": "devbox://projects", "name": "projects", "title": "项目列表", "description": "全部项目及运行状态（JSON）", "mimeType": "application/json" }),
        json!({ "uri": "devbox://ports", "name": "ports", "title": "项目相关的监听端口", "mimeType": "text/plain" }),
    ];
    for p in ctx.state.config.lock().unwrap().projects.iter() {
        list.push(json!({
            "uri": format!("devbox://logs/{}", encode(&p.name)),
            "name": format!("logs-{}", p.name),
            "title": format!("{} 的最新日志", p.name),
            "description": "最近 200 行，已去掉颜色代码",
            "mimeType": "text/plain",
        }));
    }
    json!({ "resources": list })
}

pub fn resource_templates() -> Value {
    json!({ "resourceTemplates": [{
        "uriTemplate": "devbox://logs/{project}",
        "name": "project-logs",
        "title": "项目日志",
        "description": "某个项目最近 200 行日志；{project} 是项目名称（需 URL 编码）或 id",
        "mimeType": "text/plain",
    }] })
}

pub fn resources_read(ctx: &Ctx, params: &Value) -> Result<Value, (i64, String)> {
    let uri = params.get("uri").and_then(|u| u.as_str()).ok_or((-32602, "缺少参数 uri".to_string()))?;
    let started = std::time::Instant::now();
    let (mime, outcome): (&str, Result<String, String>) = match uri {
        "devbox://overview" => ("text/plain", read::overview(ctx, &json!({})).map(|o| o.text)),
        "devbox://projects" => ("application/json", read::list_projects(ctx, &json!({})).map(|o| o.data.to_string())),
        "devbox://ports" => ("text/plain", read::list_ports(ctx, &json!({})).map(|o| o.text)),
        other => match other.strip_prefix("devbox://logs/") {
            Some(name) => ("text/plain", read::project_logs(ctx, &json!({ "project": decode(name), "tail": 200 })).map(|o| o.text)),
            None => return Err((-32002, format!("没有这个资源：{other}"))),
        },
    };
    ctx.state.mcp.record(Activity {
        ts: chrono::Local::now().format("%H:%M:%S").to_string(),
        tool: "resources/read".into(),
        target: Some(uri.chars().take(80).collect()),
        ok: outcome.is_ok(),
        ms: started.elapsed().as_millis() as u64,
        error: outcome.as_ref().err().map(|e| e.chars().take(200).collect()),
        client: ctx.client.clone(),
    });
    match outcome {
        Ok(text) => Ok(json!({ "contents": [{ "uri": uri, "mimeType": mime, "text": text }] })),
        Err(e) => Err((-32002, e)),
    }
}

// ---------------------------------------------------------------- 提示词

pub fn prompts_list() -> Value {
    json!({ "prompts": [
        {
            "name": "diagnose_project",
            "title": "排查项目问题",
            "description": "某个项目起不来、崩了或表现异常时，按固定流程查日志、端口、环境并给出结论",
            "arguments": [{ "name": "project", "description": "项目名称", "required": true }],
        },
        {
            "name": "start_dev_stack",
            "title": "一键拉起开发环境",
            "description": "按依赖顺序启动一组项目，逐个确认就绪，失败时定位原因",
            "arguments": [{ "name": "group", "description": "项目组名称；不填则启动所有项目", "required": false }],
        },
        {
            "name": "free_port",
            "title": "释放被占用的端口",
            "description": "查清谁占着某个端口，选择最温和的方式释放它",
            "arguments": [{ "name": "port", "description": "端口号", "required": true }],
        },
        {
            "name": "health_check",
            "title": "开发环境体检",
            "description": "检查所有项目是否正常：该运行的在不在、端口监听没、有没有近期崩溃或错误日志",
            "arguments": [],
        },
    ] })
}

pub fn prompts_get(params: &Value) -> Result<Value, (i64, String)> {
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let arg = |k: &str| params.get("arguments").and_then(|a| a.get(k)).and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let need = |k: &str| arg(k).ok_or((-32602, format!("缺少参数 {k}")));
    let (desc, text) = match name {
        "diagnose_project" => {
            let p = need("project")?;
            (
                format!("排查「{p}」"),
                format!(
                    "请帮我排查 DevBox 里项目「{p}」的问题。按这个顺序做，每一步只在需要时才往下：\n\
                     1. devbox_get_project 看它的状态、端口、分支、当前环境，以及期望端口是不是被别的进程占了。\n\
                     2. devbox_recent_exits 看它最近有没有崩溃，退出原因是什么。\n\
                     3. devbox_project_logs（level=error，context=3）看报错和堆栈；没有错误就看 level=warn 和最后 50 行。\n\
                     4. 如果像是端口冲突，用 devbox_check_port 确认占用者。\n\
                     5. 如果像是环境不对（连错库、缺配置），用 devbox_project_envs 对照当前环境。\n\
                     最后告诉我：根本原因、你的依据（引用关键日志行）、建议怎么修。需要重启或改动状态的操作，先说明再征求我同意，不要擅自做。"
                ),
            )
        }
        "start_dev_stack" => {
            let target = match arg("group") {
                Some(g) => format!("项目组「{g}」（devbox_start_group）"),
                None => "所有项目（先 devbox_list_projects，按依赖顺序用 devbox_start_project 逐个启动）".to_string(),
            };
            (
                "拉起开发环境".to_string(),
                format!(
                    "请帮我启动 {target}。已经在运行的不要重启。\n\
                     启动后用 devbox_overview 确认每个项目都在运行且端口已监听；有失败的，读它的错误日志（devbox_project_logs level=error）定位原因并告诉我，不要反复重试。"
                ),
            )
        }
        "free_port" => {
            let port = need("port")?;
            (
                format!("释放端口 {port}"),
                format!(
                    "端口 {port} 被占用了，请帮我释放。\n\
                     1. devbox_check_port 查占用者。\n\
                     2. 如果占用者是 DevBox 的某个项目，用 devbox_stop_project 停掉它——这是最温和的方式。\n\
                     3. 如果是外部进程，先告诉我进程名和 pid，由我决定是否结束；我同意后才用 devbox_kill_port。\n\
                     4. 释放后再次 devbox_check_port 确认。"
                ),
            )
        }
        "health_check" => (
            "开发环境体检".to_string(),
            "请给我的开发环境做一次体检：\n\
             1. devbox_overview 看全局；\n\
             2. 对运行中的项目，端口没监听或有脚本卡住的单独指出；\n\
             3. devbox_recent_exits 看最近的崩溃；\n\
             4. 对运行中的项目用 devbox_project_logs（level=error，tail=20）扫一遍近期错误；\n\
             5. devbox_git_status 看有没有项目落后上游很多或有大量未提交修改。\n\
             汇总成一份简短的清单：正常的一行带过，有问题的写清现象和建议。只读，不要做任何修改。"
                .to_string(),
        ),
        other => return Err((-32602, format!("没有名为「{other}」的提示词"))),
    };
    Ok(json!({ "description": desc, "messages": [{ "role": "user", "content": { "type": "text", "text": text } }] }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 百分号编码往返() {
        for s in ["my app", "前端 项目", "a/b?c", "plain-name_1.0"] {
            assert_eq!(decode(&encode(s)), s);
        }
        assert_eq!(encode("a b"), "a%20b");
    }

    #[test]
    fn 解码容忍残缺的百分号() {
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("%zz"), "%zz");
        assert_eq!(decode("a%2"), "a%2");
    }
}
