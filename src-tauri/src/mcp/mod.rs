//! 对 AI agent 开放的 MCP 服务（Model Context Protocol，Streamable HTTP 传输）。
//!
//! DevBox 启动后在 127.0.0.1 上监听，`POST /mcp` 就是标准的 MCP 端点：
//! 支持 Streamable HTTP 的客户端（Claude Code、Cursor、Cline…）填上地址和 token 就能直连；
//! 只支持 stdio 的客户端用 `devbox-mcp/` 里那个几十行、零依赖的转发器。
//! 工具、资源、提示词都由本进程提供，所以应用升级后 agent 看到的能力自动跟着升级。
//!
//! 安全：
//! - 只绑定回环地址；每个请求都要带 token（常量时间比较），token 持久化在配置目录（0600）；
//! - 校验 Host / Origin，防止网页通过 DNS 重绑定攻击本机服务；
//! - 权限档位（只读 / 标准 / 完全）由服务端强制执行：越权的工具既不出现在列表里，直接调用也会被拒绝；
//! - 总开关随时可关，调用记录可在界面里查看。

mod extras;
mod logq;
mod read;
mod rpc;
mod tools;
mod write;

use crate::models::McpMode;
use crate::state::AppState;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::Read;
use std::sync::atomic::{AtomicU16, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Manager, State};
use tiny_http::{Header, Method, Response, Server};

/// 单个请求体上限
const MAX_BODY: u64 = 1024 * 1024;
/// 同时处理的请求数上限（每个请求一个线程；wait_for_log 之类会占着线程等待）
const MAX_INFLIGHT: usize = 32;
/// 界面里保留多少条调用记录
const MAX_ACTIVITY: usize = 200;
/// 端口候选范围
const PORTS: std::ops::Range<u16> = 8765..8785;

/// 一次工具调用的记录
#[derive(Serialize, Clone, Debug)]
pub struct Activity {
    pub ts: String,
    pub tool: String,
    /// 针对哪个项目 / 端口 / 服务（有的话）
    pub target: Option<String>,
    pub ok: bool,
    pub ms: u64,
    /// 失败时的简短原因
    pub error: Option<String>,
    /// 谁调的（客户端自报的名字或 User-Agent）
    pub client: String,
}

/// MCP 服务的运行时信息，放在 AppState 里
#[derive(Default)]
pub struct Runtime {
    pub port: AtomicU16,
    pub token: Mutex<String>,
    pub activity: Mutex<VecDeque<Activity>>,
    pub calls: AtomicU64,
    inflight: AtomicUsize,
    /// 服务没能启动的原因（端口全被占用等），界面里据此提示
    pub error: Mutex<Option<String>>,
}

impl Runtime {
    pub fn record(&self, a: Activity) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let mut q = self.activity.lock().unwrap();
        q.push_back(a);
        while q.len() > MAX_ACTIVITY {
            q.pop_front();
        }
    }
}

// ---------------------------------------------------------------- HTTP 层

/// 在后台线程启动服务
pub fn spawn(app: AppHandle) {
    std::thread::spawn(move || {
        if let Err(e) = serve(app.clone()) {
            eprintln!("[devbox-mcp] 启动失败: {e}");
            *app.state::<AppState>().mcp.error.lock().unwrap() = Some(e);
        }
    });
}

fn bridge_path(state: &AppState) -> std::path::PathBuf {
    state.config_path.parent().map(|p| p.to_path_buf()).unwrap_or_default().join("bridge.json")
}

/// 上次留下的端口和 token：沿用它们，agent 的配置才不会每次重启应用就失效
fn previous_bridge(state: &AppState) -> (Option<u16>, Option<String>) {
    let Ok(text) = std::fs::read_to_string(bridge_path(state)) else { return (None, None) };
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return (None, None) };
    let port = v.get("port").and_then(|p| p.as_u64()).and_then(|p| u16::try_from(p).ok());
    let token = v.get("token").and_then(|t| t.as_str()).filter(|t| t.len() >= 32).map(|t| t.to_string());
    (port, token)
}

fn new_token() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

/// 写 bridge.json，供 stdio 转发器读取。含 token，所以在类 Unix 上收紧权限
fn write_bridge_file(state: &AppState) {
    let port = state.mcp.port.load(Ordering::SeqCst);
    let token = state.mcp.token.lock().unwrap().clone();
    let path = bridge_path(state);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let content = json!({
        "port": port,
        "token": token,
        "base_url": format!("http://127.0.0.1:{port}"),
        "mcp_url": format!("http://127.0.0.1:{port}/mcp"),
        "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
    });
    let _ = std::fs::write(&path, serde_json::to_string_pretty(&content).unwrap_or_default());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
}

fn serve(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let (prev_port, prev_token) = previous_bridge(&state);

    // 先试上次的端口，再从范围里找空闲的
    let candidates = prev_port.into_iter().chain(PORTS);
    let mut bound = None;
    for port in candidates {
        if let Ok(s) = Server::http(("127.0.0.1", port)) {
            bound = Some((port, s));
            break;
        }
    }
    let (port, server) = bound.ok_or("8765-8784 端口全部被占用，无法启动 MCP 服务")?;

    state.mcp.port.store(port, Ordering::SeqCst);
    *state.mcp.token.lock().unwrap() = prev_token.unwrap_or_else(new_token);
    write_bridge_file(&state);
    println!("[devbox-mcp] 监听 http://127.0.0.1:{port}/mcp");

    run_loop(app.clone(), server);
    Ok(())
}

/// 接收请求：每个请求一个线程，慢请求（等日志、跑脚本、git pull）不会堵住别的调用
fn run_loop<R: tauri::Runtime>(app: AppHandle<R>, server: Server) {
    for mut request in server.incoming_requests() {
        let app = app.clone();
        std::thread::spawn(move || {
            let state = app.state::<AppState>();
            let method = request.method().clone();
            let path = request.url().split('?').next().unwrap_or("").to_string();
            let headers: Vec<(String, String)> = request
                .headers()
                .iter()
                .map(|h| (h.field.as_str().as_str().to_ascii_lowercase(), h.value.as_str().to_string()))
                .collect();
            let mut body = String::new();
            let too_big = {
                let mut limited = request.as_reader().take(MAX_BODY + 1);
                let _ = limited.read_to_string(&mut body);
                body.len() as u64 > MAX_BODY
            };
            let reply = if too_big {
                Reply::json(413, json!({ "error": "请求体太大" }))
            } else {
                handle_http(state.inner(), &method, &path, &headers, &body)
            };
            let mut resp = Response::from_string(reply.body).with_status_code(reply.status);
            if let Ok(h) = Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..]) {
                resp = resp.with_header(h);
            }
            for (k, v) in reply.headers {
                if let Ok(h) = Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                    resp = resp.with_header(h);
                }
            }
            let _ = request.respond(resp);
        });
    }
}

/// 一次 HTTP 应答
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub headers: Vec<(String, String)>,
}

impl Reply {
    fn json(status: u16, v: Value) -> Self {
        Reply { status, body: v.to_string(), headers: vec![] }
    }
    fn empty(status: u16) -> Self {
        Reply { status, body: String::new(), headers: vec![] }
    }
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// 主机名是否指向本机（去掉端口、方括号）
fn is_local_host(host: &str) -> bool {
    let h = host.trim();
    let h = h.strip_prefix('[').and_then(|r| r.split(']').next()).unwrap_or_else(|| h.rsplit_once(':').map_or(h, |(a, _)| a));
    matches!(h, "127.0.0.1" | "localhost" | "::1")
}

/// Origin 形如 http://localhost:5173；null / 缺失不在这里判断
fn origin_is_local(origin: &str) -> bool {
    let rest = origin.split_once("://").map_or(origin, |(_, r)| r);
    is_local_host(rest.split('/').next().unwrap_or(""))
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn bearer(headers: &[(String, String)]) -> String {
    if let Some(v) = header(headers, "authorization") {
        if let Some(t) = v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")) {
            return t.trim().to_string();
        }
    }
    header(headers, "x-devbox-token").unwrap_or("").trim().to_string()
}

/// 处理一个 HTTP 请求（不含 socket 读写，方便直接在测试里调用）
pub fn handle_http(state: &AppState, method: &Method, path: &str, headers: &[(String, String)], body: &str) -> Reply {
    if *method == Method::Get && path == "/health" {
        return Reply::json(200, json!({ "ok": true, "name": "devbox", "version": env!("CARGO_PKG_VERSION"), "mcp": "/mcp" }));
    }
    if path != "/mcp" {
        return Reply::json(404, json!({ "error": "未知路径；MCP 端点是 POST /mcp" }));
    }
    // DNS 重绑定防护：网页里的脚本能让域名解析到 127.0.0.1，但 Host / Origin 改不了
    if let Some(host) = header(headers, "host") {
        if !is_local_host(host) {
            return Reply::json(403, json!({ "error": "Host 不是本机地址" }));
        }
    }
    if let Some(origin) = header(headers, "origin") {
        if !origin_is_local(origin) {
            return Reply::json(403, json!({ "error": "不接受来自网页的跨站请求" }));
        }
    }
    let token = state.mcp.token.lock().unwrap().clone();
    if token.is_empty() || !constant_time_eq(&bearer(headers), &token) {
        return Reply {
            status: 401,
            body: json!({ "error": "未授权：token 不正确。在 DevBox 的「MCP」页面可以复制最新的 token" }).to_string(),
            headers: vec![("WWW-Authenticate".into(), "Bearer".into())],
        };
    }
    if *method != Method::Post {
        // 本服务不提供服务端推送流（GET）和会话（DELETE）
        return Reply { status: 405, body: json!({ "error": "只支持 POST" }).to_string(), headers: vec![("Allow".into(), "POST".into())] };
    }

    let (enabled, mode) = {
        let cfg = state.config.lock().unwrap();
        (cfg.mcp.enabled, cfg.mcp.mode)
    };
    if !enabled {
        return Reply::json(403, json!({ "error": "DevBox 的 MCP 服务已被用户关闭。请在 DevBox 的「MCP」页面打开" }));
    }
    if state.mcp.inflight.fetch_add(1, Ordering::SeqCst) >= MAX_INFLIGHT {
        state.mcp.inflight.fetch_sub(1, Ordering::SeqCst);
        return Reply::json(503, json!({ "error": "请求过多，请稍后再试" }));
    }
    let client = header(headers, "x-devbox-client").or_else(|| header(headers, "user-agent")).unwrap_or("unknown");
    let ctx = rpc::Ctx { state, mode, client: client.chars().take(60).collect() };
    let reply = rpc::handle_body(&ctx, body);
    state.mcp.inflight.fetch_sub(1, Ordering::SeqCst);
    match reply {
        rpc::Body::Accepted => Reply::empty(202),
        rpc::Body::Json(v) => Reply::json(200, v),
        rpc::Body::ParseError(v) => Reply::json(400, v),
    }
}

// ---------------------------------------------------------------- 界面用的命令

#[derive(Serialize)]
pub struct ToolBrief {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// "read" | "write" | "danger"
    pub risk: &'static str,
    /// 当前档位下是否可用
    pub allowed: bool,
}

#[derive(Serialize)]
pub struct McpInfo {
    pub enabled: bool,
    pub mode: McpMode,
    pub port: u16,
    pub url: String,
    pub token: String,
    pub bridge_file: String,
    pub version: &'static str,
    pub calls_total: u64,
    /// 服务启动失败的原因；正常为 None
    pub error: Option<String>,
    pub tools: Vec<ToolBrief>,
}

fn info_of(state: &AppState) -> McpInfo {
    let (enabled, mode) = {
        let cfg = state.config.lock().unwrap();
        (cfg.mcp.enabled, cfg.mcp.mode)
    };
    let port = state.mcp.port.load(Ordering::SeqCst);
    McpInfo {
        enabled,
        mode,
        port,
        url: if port == 0 { String::new() } else { format!("http://127.0.0.1:{port}/mcp") },
        token: state.mcp.token.lock().unwrap().clone(),
        bridge_file: bridge_path(state).to_string_lossy().into_owned(),
        version: env!("CARGO_PKG_VERSION"),
        calls_total: state.mcp.calls.load(Ordering::Relaxed),
        error: state.mcp.error.lock().unwrap().clone(),
        tools: tools::all()
            .iter()
            .map(|t| ToolBrief {
                name: t.name,
                title: t.title,
                description: t.description,
                risk: t.risk.as_str(),
                allowed: t.risk.allowed_in(mode),
            })
            .collect(),
    }
}

/// MCP 服务的当前状态：地址、token、权限档位、可用工具
#[tauri::command]
pub fn mcp_info(state: State<AppState>) -> McpInfo {
    info_of(&state)
}

/// 修改总开关和权限档位，立即生效
#[tauri::command]
pub fn mcp_set_settings(enabled: bool, mode: McpMode, state: State<AppState>) -> Result<McpInfo, String> {
    {
        let mut cfg = state.config.lock().unwrap();
        cfg.mcp.enabled = enabled;
        cfg.mcp.mode = mode;
    }
    state.persist()?;
    Ok(info_of(&state))
}

/// 重新生成 token：旧 token 立即失效，要把新的填回 agent 的配置里
#[tauri::command]
pub fn mcp_reset_token(state: State<AppState>) -> McpInfo {
    *state.mcp.token.lock().unwrap() = new_token();
    write_bridge_file(&state);
    info_of(&state)
}

/// 最近的工具调用记录（新的在前）
#[tauri::command]
pub fn mcp_activity(state: State<AppState>) -> Vec<Activity> {
    state.mcp.activity.lock().unwrap().iter().rev().cloned().collect()
}

/// 清空调用记录
#[tauri::command]
pub fn mcp_clear_activity(state: State<AppState>) {
    state.mcp.activity.lock().unwrap().clear();
}

#[cfg(test)]
mod tests;
