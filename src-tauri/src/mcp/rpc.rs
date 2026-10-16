//! JSON-RPC 2.0 / MCP 协议层：initialize、tools、resources、prompts。

use super::{extras, tools, Activity};
use crate::models::McpMode;
use crate::state::AppState;
use serde_json::{json, Value};
use std::time::Instant;

/// 本服务实现的协议版本，按新旧排列；客户端要的版本在列表里就照它来，否则用最新的
const SUPPORTED: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// 处理一条消息所需的上下文
pub struct Ctx<'a> {
    pub state: &'a AppState,
    pub mode: McpMode,
    /// 调用方自报的名字，写进调用记录
    pub client: String,
}

/// HTTP 层该怎么回
pub enum Body {
    /// 只有通知 / 响应，没有要回的内容 → 202
    Accepted,
    Json(Value),
    ParseError(Value),
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn result(id: &Value, r: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": r })
}

/// 处理 POST 体：单条消息或批量数组
pub fn handle_body(ctx: &Ctx, body: &str) -> Body {
    let parsed: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return Body::ParseError(error(&Value::Null, -32700, &format!("不是合法的 JSON：{e}"))),
    };
    match parsed {
        Value::Array(items) => {
            if items.is_empty() {
                return Body::Json(error(&Value::Null, -32600, "批量请求不能为空"));
            }
            let replies: Vec<Value> = items.iter().filter_map(|m| handle_message(ctx, m)).collect();
            if replies.is_empty() {
                Body::Accepted
            } else {
                Body::Json(Value::Array(replies))
            }
        }
        single => match handle_message(ctx, &single) {
            Some(r) => Body::Json(r),
            None => Body::Accepted,
        },
    }
}

/// 处理一条 JSON-RPC 消息；通知（没有 id）和客户端发来的响应不需要回复，返回 None
pub fn handle_message(ctx: &Ctx, msg: &Value) -> Option<Value> {
    let Some(obj) = msg.as_object() else {
        return Some(error(&Value::Null, -32600, "消息必须是 JSON 对象"));
    };
    let method = obj.get("method").and_then(|m| m.as_str());
    let id = obj.get("id");
    let Some(method) = method else {
        // 没有 method：客户端对服务端请求的响应（本服务不发请求，忽略）
        return if id.is_some() && obj.contains_key("result") || obj.contains_key("error") {
            None
        } else {
            Some(error(id.unwrap_or(&Value::Null), -32600, "缺少 method"))
        };
    };
    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = id else {
        return None; // 通知：initialized、cancelled 之类，无需处理
    };

    let outcome: Result<Value, (i64, String)> = match method {
        "initialize" => Ok(initialize(ctx, &params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools::list(ctx.mode) })),
        "tools/call" => Ok(call_tool(ctx, &params)),
        "resources/list" => Ok(extras::resources_list(ctx)),
        "resources/templates/list" => Ok(extras::resource_templates()),
        "resources/read" => extras::resources_read(ctx, &params),
        "prompts/list" => Ok(extras::prompts_list()),
        "prompts/get" => extras::prompts_get(&params),
        _ => Err((-32601, format!("不支持的方法：{method}"))),
    };
    Some(match outcome {
        Ok(r) => result(id, r),
        Err((code, msg)) => error(id, code, &msg),
    })
}

fn initialize(ctx: &Ctx, params: &Value) -> Value {
    let wanted = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("");
    let version = SUPPORTED.iter().find(|v| **v == wanted).unwrap_or(&SUPPORTED[0]);
    json!({
        "protocolVersion": version,
        "capabilities": {
            "tools": { "listChanged": false },
            "resources": { "subscribe": false, "listChanged": false },
            "prompts": { "listChanged": false },
        },
        "serverInfo": { "name": "devbox", "title": "DevBox", "version": env!("CARGO_PKG_VERSION") },
        "instructions": extras::instructions(ctx),
    })
}

fn call_tool(ctx: &Ctx, params: &Value) -> Value {
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let started = Instant::now();

    let outcome = match tools::find(name) {
        None => Err(format!("没有名为「{name}」的工具。用 tools/list 查看可用工具")),
        Some(def) if !def.risk.allowed_in(ctx.mode) => Err(format!(
            "当前 MCP 权限档位是「{}」，不允许「{}」这类操作（需要「{}」档）。这是用户在 DevBox 的「MCP」页面设定的，请把需求告诉用户，由用户决定是否调高档位",
            mode_label(ctx.mode),
            def.title,
            mode_label(def.risk.min_mode()),
        )),
        Some(_) => run_guarded(ctx, name, &args),
    };

    ctx.state.mcp.record(Activity {
        ts: chrono::Local::now().format("%H:%M:%S").to_string(),
        tool: name.to_string(),
        target: tools::target_of(&args),
        ok: outcome.is_ok(),
        ms: started.elapsed().as_millis() as u64,
        error: outcome.as_ref().err().map(|e| e.chars().take(200).collect()),
        client: ctx.client.clone(),
    });

    match outcome {
        Ok(out) => {
            let mut v = json!({ "content": [{ "type": "text", "text": out.text }] });
            if !out.data.is_null() {
                v["structuredContent"] = out.data;
            }
            v
        }
        // 业务失败走 isError，让模型能看到原因并自行调整；协议错误才用 JSON-RPC error
        Err(msg) => json!({ "content": [{ "type": "text", "text": msg }], "isError": true }),
    }
}

/// 处理函数里万一 panic，不要把整个请求线程带走，而是报成一次失败的调用
fn run_guarded(ctx: &Ctx, name: &str, args: &Value) -> Result<tools::ToolOut, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tools::call(ctx, name, args)))
        .unwrap_or_else(|_| Err("工具内部出错（已记录），请重试或换个方式".into()))
}

pub fn mode_label(mode: McpMode) -> &'static str {
    match mode {
        McpMode::Readonly => "只读",
        McpMode::Standard => "标准",
        McpMode::Full => "完全",
    }
}
