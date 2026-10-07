//! DevBox 本地 HTTP 桥接服务。
//!
//! 仅绑定 127.0.0.1（回环地址），供本机的 MCP 适配器调用，让 agent（claude code /
//! Hermes 等）能够启动、停止、重启项目、查看状态与日志、管理端口 / hosts / brew 服务。
//!
//! 启动时随机选一个空闲端口并生成访问 token，写入
//! `~/Library/Application Support/devbox/bridge.json`，MCP 适配器据此连接与鉴权。

use crate::commands::{hosts, ports, process, projects, services};
use crate::models::{HostEntry, Project};
use crate::state::AppState;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tiny_http::{Header, Method, Response, Server};

/// 在后台线程启动 HTTP 桥接服务
pub fn spawn(app: AppHandle) {
    std::thread::spawn(move || {
        if let Err(e) = serve(app) {
            eprintln!("[devbox-bridge] 启动失败: {e}");
        }
    });
}

fn serve(app: AppHandle) -> Result<(), String> {
    // 在 8765..8785 里找一个能绑定的端口
    let mut server = None;
    let mut bound_port = 0u16;
    for port in 8765u16..8785 {
        match Server::http(("127.0.0.1", port)) {
            Ok(s) => {
                bound_port = port;
                server = Some(s);
                break;
            }
            Err(_) => continue,
        }
    }
    let server = server.ok_or("8765-8784 端口全部被占用，无法启动桥接服务")?;

    let token = uuid::Uuid::new_v4().to_string();
    write_bridge_file(bound_port, &token);
    println!("[devbox-bridge] 监听 http://127.0.0.1:{bound_port}");

    for mut request in server.incoming_requests() {
        // 读取请求体
        let mut body = String::new();
        let _ = request.as_reader().read_to_string(&mut body);

        let method = request.method().clone();
        let url = request.url().to_string();
        let path = url.split('?').next().unwrap_or("").to_string();

        // /health 不需要鉴权，其余接口校验 token
        let authorized = path == "/health" || header_token(&request) == token;

        let reply: Value = if !authorized {
            json!({ "ok": false, "error": "未授权：token 不正确" })
        } else {
            let state = app.state::<AppState>();
            route(state.inner(), &method, &path, &body)
        };

        let json_str = reply.to_string();
        let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..])
            .unwrap();
        let response = Response::from_string(json_str).with_header(header);
        let _ = request.respond(response);
    }
    Ok(())
}

/// 根据方法与路径分发，返回统一信封 {ok, data} / {ok:false, error}
fn route(state: &AppState, method: &Method, path: &str, body: &str) -> Value {
    if *method == Method::Get && path == "/health" {
        return ok(json!({ "version": env!("CARGO_PKG_VERSION") }));
    }
    // 其余一律按 POST 处理，body 为 JSON（允许为空）
    let arg: Value = serde_json::from_str(body).unwrap_or(Value::Null);

    match path {
        // ---- 项目 ----
        "/projects/list" => ok(json!(state.config.lock().unwrap().projects.clone())),
        "/projects/status" => ok(json!(process::compute_statuses(state))),
        "/projects/start" => match resolve_project(state, &arg) {
            Some(p) => wrap(process::start_project_inner(&p.id, state)),
            None => err("找不到该项目（请用 id 或 name 指定）"),
        },
        "/projects/stop" => match resolve_project(state, &arg) {
            Some(p) => wrap(process::stop_project_inner(&p.id, state)),
            None => err("找不到该项目（请用 id 或 name 指定）"),
        },
        "/projects/restart" => match resolve_project(state, &arg) {
            Some(p) => {
                let _ = process::stop_project_inner(&p.id, state);
                std::thread::sleep(std::time::Duration::from_millis(600));
                wrap(process::start_project_inner(&p.id, state))
            }
            None => err("找不到该项目（请用 id 或 name 指定）"),
        },
        "/projects/logs" => match resolve_project(state, &arg) {
            Some(p) => {
                let tail = arg.get("tail").and_then(|v| v.as_u64()).map(|n| n as usize);
                ok(json!(read_logs(state, &p.id, tail)))
            }
            None => err("找不到该项目（请用 id 或 name 指定）"),
        },

        // ---- 端口 ----
        "/ports/list" => match ports::list_ports() {
            Ok(v) => ok(json!(v)),
            Err(e) => err(&e),
        },
        "/ports/kill" => {
            let pid = arg.get("pid").and_then(|v| v.as_u64()).map(|n| n as u32);
            let port = arg.get("port").and_then(|v| v.as_u64()).map(|n| n as u16);
            match pid {
                Some(pid) => wrap(ports::kill_process(pid, port)),
                None => err("缺少参数 pid"),
            }
        }

        // ---- hosts ----
        "/hosts/get" => match hosts::get_hosts() {
            Ok(v) => ok(json!(v)),
            Err(e) => err(&e),
        },
        "/hosts/raw" => match hosts::get_hosts_raw() {
            Ok(v) => ok(json!(v)),
            Err(e) => err(&e),
        },
        "/hosts/save" => {
            match arg.get("entries").cloned().map(serde_json::from_value::<Vec<HostEntry>>) {
                Some(Ok(entries)) => wrap(hosts::save_hosts(entries)),
                _ => err("缺少或无法解析参数 entries"),
            }
        }

        // ---- brew 服务 ----
        "/services/list" => match services::list_services() {
            Ok(v) => ok(json!(v)),
            Err(e) => err(&e),
        },
        "/services/start" => match arg_name(&arg) {
            Some(n) => wrap(services::start_service(n)),
            None => err("缺少参数 name"),
        },
        "/services/stop" => match arg_name(&arg) {
            Some(n) => wrap(services::stop_service(n)),
            None => err("缺少参数 name"),
        },
        "/services/restart" => match arg_name(&arg) {
            Some(n) => wrap(services::restart_service(n)),
            None => err("缺少参数 name"),
        },

        // ---- 项目探测（辅助 agent 新增项目时填参）----
        "/projects/detect" => match arg.get("path").and_then(|v| v.as_str()) {
            Some(p) => match projects::detect_project(p.to_string()) {
                Ok(d) => ok(json!(d)),
                Err(e) => err(&e),
            },
            None => err("缺少参数 path"),
        },

        "/projects/start-commands" => match arg.get("path").and_then(|v| v.as_str()) {
            Some(p) => match crate::commands::startcmd::detect_start_commands(p.to_string()) {
                Ok(c) => ok(json!(c)),
                Err(e) => err(&e),
            },
            None => err("缺少参数 path"),
        },

        _ => err(&format!("未知接口: {path}")),
    }
}

// ---------- 辅助 ----------

fn ok(data: Value) -> Value {
    json!({ "ok": true, "data": data })
}

fn err(msg: &str) -> Value {
    json!({ "ok": false, "error": msg })
}

fn wrap(r: Result<(), String>) -> Value {
    match r {
        Ok(_) => json!({ "ok": true, "data": null }),
        Err(e) => json!({ "ok": false, "error": e }),
    }
}

fn arg_name(arg: &Value) -> Option<String> {
    arg.get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// 按 id 优先、其次 name 查找项目
fn resolve_project(state: &AppState, arg: &Value) -> Option<Project> {
    let cfg = state.config.lock().unwrap();
    if let Some(id) = arg.get("id").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        if let Some(p) = cfg.projects.iter().find(|p| p.id == id) {
            return Some(p.clone());
        }
    }
    if let Some(name) = arg.get("name").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        if let Some(p) = cfg.projects.iter().find(|p| p.name == name) {
            return Some(p.clone());
        }
    }
    None
}

/// 读取项目日志缓冲，可选只取末尾 tail 行
fn read_logs(state: &AppState, id: &str, tail: Option<usize>) -> Vec<crate::models::LogLine> {
    let all = state.log_buffer(id).snapshot();
    match tail {
        Some(n) if n < all.len() => all[all.len() - n..].to_vec(),
        _ => all,
    }
}

fn header_token(request: &tiny_http::Request) -> String {
    for h in request.headers() {
        if h.field.as_str().as_str().eq_ignore_ascii_case("X-DevBox-Token") {
            return h.value.as_str().to_string();
        }
    }
    String::new()
}

/// 把桥接端口与 token 写到磁盘，供 MCP 适配器读取
fn write_bridge_file(port: u16, token: &str) {
    let base = dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let dir = base.join("devbox");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("bridge.json");
    let content = json!({
        "port": port,
        "token": token,
        "base_url": format!("http://127.0.0.1:{port}"),
    });
    let _ = std::fs::write(path, serde_json::to_string_pretty(&content).unwrap_or_default());
}
