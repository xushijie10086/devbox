//! MCP 服务的集成测试：直接把 HTTP 请求喂给 handle_http，覆盖鉴权、协议、权限档位和各类工具。

use super::*;
use crate::models::{Config, McpMode, McpSettings};
use crate::testutil::{listen_command, sleep_command, workdir};
use crate::commands::process;
use tiny_http::Method;

const TOKEN: &str = "test-token-0123456789abcdef-0123456789abcdef";

fn state_with(projects: Vec<Value>) -> AppState {
    let projects = projects.into_iter().map(|p| serde_json::from_value(p).unwrap()).collect();
    let st = AppState::for_test(Config { projects, ..Default::default() });
    *st.mcp.token.lock().unwrap() = TOKEN.to_string();
    st.mcp.port.store(8765, Ordering::SeqCst);
    st
}

fn proj(id: &str, name: &str) -> Value {
    json!({ "id": id, "name": name, "path": workdir(), "start_command": sleep_command(30) })
}

fn set_mode(st: &AppState, mode: McpMode) {
    st.config.lock().unwrap().mcp.mode = mode;
}

fn auth_headers() -> Vec<(String, String)> {
    vec![("host".into(), "127.0.0.1:8765".into()), ("authorization".into(), format!("Bearer {TOKEN}"))]
}

fn post_with(st: &AppState, headers: &[(String, String)], body: &str) -> (u16, Value) {
    let r = handle_http(st, &Method::Post, "/mcp", headers, body);
    let v = if r.body.is_empty() { Value::Null } else { serde_json::from_str(&r.body).unwrap() };
    (r.status, v)
}

fn post(st: &AppState, body: Value) -> (u16, Value) {
    post_with(st, &auth_headers(), &body.to_string())
}

fn rpc(st: &AppState, method: &str, params: Value) -> Value {
    let (status, v) = post(st, json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }));
    assert_eq!(status, 200, "{v}");
    v
}

/// 调一个工具，返回 (是否报错, 文本, 结构化数据)
fn tool(st: &AppState, name: &str, args: Value) -> (bool, String, Value) {
    let v = rpc(st, "tools/call", json!({ "name": name, "arguments": args }));
    let r = &v["result"];
    assert!(r.is_object(), "{v}");
    (r["isError"] == json!(true), r["content"][0]["text"].as_str().unwrap_or("").to_string(), r["structuredContent"].clone())
}

fn ok_tool(st: &AppState, name: &str, args: Value) -> (String, Value) {
    let (err, text, data) = tool(st, name, args);
    assert!(!err, "{name} 不应失败：{text}");
    (text, data)
}

fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port()
}

// ---------------------------------------------------------------- 鉴权与网络层

#[test]
fn health_不需要鉴权() {
    let st = state_with(vec![]);
    let r = handle_http(&st, &Method::Get, "/health", &[], "");
    assert_eq!(r.status, 200);
    assert!(r.body.contains("devbox"));
}

#[test]
fn 没有或错误的_token_一律_401() {
    let st = state_with(vec![]);
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }).to_string();
    let host = ("host".to_string(), "127.0.0.1:8765".to_string());
    assert_eq!(post_with(&st, std::slice::from_ref(&host), &body).0, 401);
    assert_eq!(post_with(&st, &[host.clone(), ("authorization".into(), "Bearer nope".into())], &body).0, 401);
    // 长度相同但内容不同
    let almost = TOKEN.replace('0', "9");
    assert_eq!(post_with(&st, &[host.clone(), ("authorization".into(), format!("Bearer {almost}"))], &body).0, 401);
    // 两种带法都认
    assert_eq!(post_with(&st, &[host.clone(), ("authorization".into(), format!("Bearer {TOKEN}"))], &body).0, 200);
    assert_eq!(post_with(&st, &[host, ("x-devbox-token".into(), TOKEN.into())], &body).0, 200);
}

#[test]
fn token_还没生成时拒绝所有请求() {
    let st = state_with(vec![]);
    st.mcp.token.lock().unwrap().clear();
    let h = vec![("host".to_string(), "127.0.0.1".to_string()), ("authorization".to_string(), "Bearer ".to_string())];
    assert_eq!(post_with(&st, &h, "{}").0, 401);
}

#[test]
fn 防_dns_重绑定_host_和_origin_必须是本机() {
    let st = state_with(vec![]);
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }).to_string();
    let with = |host: &str, origin: Option<&str>| {
        let mut h = vec![("host".to_string(), host.to_string()), ("authorization".to_string(), format!("Bearer {TOKEN}"))];
        if let Some(o) = origin {
            h.push(("origin".into(), o.into()));
        }
        post_with(&st, &h, &body).0
    };
    assert_eq!(with("127.0.0.1:8765", None), 200);
    assert_eq!(with("localhost:8765", None), 200);
    assert_eq!(with("[::1]:8765", None), 200);
    assert_eq!(with("evil.example.com", None), 403);
    assert_eq!(with("evil.example.com:8765", None), 403);
    assert_eq!(with("127.0.0.1:8765", Some("http://localhost:5173")), 200);
    assert_eq!(with("127.0.0.1:8765", Some("https://evil.example.com")), 403);
    assert_eq!(with("127.0.0.1:8765", Some("http://127.0.0.1.evil.com")), 403);
}

#[test]
fn 只接受_post_且路径固定() {
    let st = state_with(vec![]);
    let h = auth_headers();
    assert_eq!(handle_http(&st, &Method::Get, "/mcp", &h, "").status, 405);
    assert_eq!(handle_http(&st, &Method::Delete, "/mcp", &h, "").status, 405);
    assert_eq!(handle_http(&st, &Method::Post, "/other", &h, "{}").status, 404);
    // 旧版桥接接口已移除
    assert_eq!(handle_http(&st, &Method::Post, "/projects/list", &h, "{}").status, 404);
}

#[test]
fn 总开关关闭后拒绝调用() {
    let st = state_with(vec![]);
    st.config.lock().unwrap().mcp = McpSettings { enabled: false, mode: McpMode::Full };
    let (status, v) = post(&st, json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }));
    assert_eq!(status, 403);
    assert!(v["error"].as_str().unwrap().contains("关闭"));
    st.config.lock().unwrap().mcp.enabled = true;
    assert_eq!(post(&st, json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" })).0, 200);
}

// ---------------------------------------------------------------- 协议

#[test]
fn initialize_协商版本并带说明() {
    let st = state_with(vec![]);
    let v = rpc(&st, "initialize", json!({ "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } }));
    assert_eq!(v["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(v["result"]["serverInfo"]["name"], "devbox");
    assert!(v["result"]["capabilities"]["tools"].is_object());
    assert!(v["result"]["capabilities"]["resources"].is_object());
    assert!(v["result"]["capabilities"]["prompts"].is_object());
    let ins = v["result"]["instructions"].as_str().unwrap();
    assert!(ins.contains("devbox_overview") && ins.contains("标准"));

    // 不认识的版本 → 给最新的
    let v = rpc(&st, "initialize", json!({ "protocolVersion": "1999-01-01" }));
    assert_eq!(v["result"]["protocolVersion"], "2025-06-18");
}

#[test]
fn 通知返回_202_批量请求逐条应答() {
    let st = state_with(vec![]);
    let (status, _) = post(&st, json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    assert_eq!(status, 202);

    let (status, v) = post(
        &st,
        json!([
            { "jsonrpc": "2.0", "id": 1, "method": "ping" },
            { "jsonrpc": "2.0", "method": "notifications/initialized" },
            { "jsonrpc": "2.0", "id": "b", "method": "tools/list" },
        ]),
    );
    assert_eq!(status, 200);
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 2, "通知不产生应答");
    assert_eq!(arr[0]["id"], 1);
    assert_eq!(arr[1]["id"], "b");

    // 全是通知的批量 → 202
    let (status, _) = post(&st, json!([{ "jsonrpc": "2.0", "method": "notifications/initialized" }]));
    assert_eq!(status, 202);
}

#[test]
fn 非法请求给出标准错误码() {
    let st = state_with(vec![]);
    let (status, v) = post_with(&st, &auth_headers(), "{不是json");
    assert_eq!(status, 400);
    assert_eq!(v["error"]["code"], -32700);

    let (_, v) = post(&st, json!({ "jsonrpc": "2.0", "id": 7, "method": "no/such" }));
    assert_eq!(v["error"]["code"], -32601);
    assert_eq!(v["id"], 7);

    let (_, v) = post(&st, json!([]));
    assert_eq!(v["error"]["code"], -32600);
    let (_, v) = post(&st, json!("字符串"));
    assert_eq!(v["error"]["code"], -32600);
}

// ---------------------------------------------------------------- 工具注册表与权限档位

fn names(st: &AppState) -> Vec<String> {
    rpc(st, "tools/list", json!({}))["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
}

#[test]
fn 工具列表随档位收窄() {
    let st = state_with(vec![]);
    set_mode(&st, McpMode::Readonly);
    let ro = names(&st);
    set_mode(&st, McpMode::Standard);
    let std = names(&st);
    set_mode(&st, McpMode::Full);
    let full = names(&st);

    assert!(ro.len() < std.len() && std.len() < full.len(), "{} {} {}", ro.len(), std.len(), full.len());
    assert_eq!(full.len(), tools::all().len());
    assert!(ro.contains(&"devbox_project_logs".to_string()));
    assert!(!ro.contains(&"devbox_start_project".to_string()));
    assert!(std.contains(&"devbox_start_project".to_string()));
    assert!(!std.contains(&"devbox_kill_port".to_string()));
    assert!(full.contains(&"devbox_kill_port".to_string()));
}

#[test]
fn 每个工具的定义都完整且名字唯一() {
    let mut seen = std::collections::HashSet::new();
    for t in tools::all() {
        assert!(t.name.starts_with("devbox_"), "{}", t.name);
        assert!(seen.insert(t.name), "重名：{}", t.name);
        assert!(!t.title.is_empty() && t.description.chars().count() >= 10, "{}", t.name);
        assert_eq!(t.schema["type"], "object", "{}", t.name);
        assert!(t.schema["properties"].is_object(), "{}", t.name);
        // required 里的每一项都得在 properties 里
        for r in t.schema["required"].as_array().unwrap() {
            assert!(t.schema["properties"].get(r.as_str().unwrap()).is_some(), "{} 的 required {r} 不在 properties 里", t.name);
        }
        // 每个参数都要有说明，模型靠它理解怎么填
        for (k, p) in t.schema["properties"].as_object().unwrap() {
            assert!(p.get("description").is_some(), "{}.{k} 缺少 description", t.name);
        }
    }
}

#[test]
fn 工具标注与风险等级一致() {
    let st = state_with(vec![]);
    set_mode(&st, McpMode::Full);
    let list = rpc(&st, "tools/list", json!({}))["result"]["tools"].clone();
    for t in list.as_array().unwrap() {
        let def = tools::find(t["name"].as_str().unwrap()).unwrap();
        let a = &t["annotations"];
        assert_eq!(a["readOnlyHint"], json!(def.risk == tools::Risk::Read), "{}", def.name);
        assert_eq!(a["destructiveHint"], json!(def.risk == tools::Risk::Danger), "{}", def.name);
        assert_eq!(a["openWorldHint"], json!(false));
        assert!(t["inputSchema"].is_object());
    }
}

#[test]
fn 注册的每个工具都有对应实现() {
    // 用空参数在完全档下调一遍：可以因为缺参数失败，但不能是「没有实现」
    let st = state_with(vec![]);
    set_mode(&st, McpMode::Full);
    for t in tools::all() {
        let (_, text, _) = tool(&st, t.name, json!({}));
        assert!(!text.contains("没有实现"), "{}：{text}", t.name);
    }
}

#[test]
fn 只读档拒绝一切写操作且不会执行() {
    let st = state_with(vec![proj("a", "alpha")]);
    set_mode(&st, McpMode::Readonly);
    for name in ["devbox_start_project", "devbox_stop_project", "devbox_clear_logs", "devbox_stop_all", "devbox_kill_port", "devbox_delete_project"] {
        let (err, text, _) = tool(&st, name, json!({ "project": "alpha", "confirm": true, "port": 1 }));
        assert!(err, "{name} 应被拒绝");
        assert!(text.contains("权限档位") && text.contains("只读"), "{name}：{text}");
    }
    assert_eq!(st.config.lock().unwrap().projects.len(), 1, "项目不应被删除");
    assert!(st.procs.lock().unwrap().is_empty(), "不应启动任何进程");
}

#[test]
fn 标准档拒绝危险操作() {
    let st = state_with(vec![proj("a", "alpha")]);
    set_mode(&st, McpMode::Standard);
    for name in ["devbox_kill_port", "devbox_edit_hosts", "devbox_save_hosts", "devbox_add_project", "devbox_update_project", "devbox_delete_project", "devbox_git_pull", "devbox_git_checkout"] {
        let (err, text, _) = tool(&st, name, json!({ "project": "alpha", "confirm": true, "port": 1, "path": workdir(), "action": "add", "hostname": "x.local", "branch": "main", "entries": [] }));
        assert!(err && text.contains("完全"), "{name}：{text}");
    }
    assert_eq!(st.config.lock().unwrap().projects.len(), 1);
    assert_eq!(st.config.lock().unwrap().projects[0].name, "alpha");
}

#[test]
fn 未知工具报业务错误而不是协议错误() {
    let st = state_with(vec![]);
    let (err, text, _) = tool(&st, "devbox_nope", json!({}));
    assert!(err && text.contains("没有名为"));
}

// ---------------------------------------------------------------- 项目查找

#[test]
fn 项目名查找_宽松但不含糊() {
    let st = state_with(vec![proj("id-1", "Backend API"), proj("id-2", "Web Frontend"), proj("id-3", "Web Admin")]);
    let find = |k: &str| tools::lookup_project(&st, k).map(|p| p.id);
    assert_eq!(find("id-2").unwrap(), "id-2");
    assert_eq!(find("Backend API").unwrap(), "id-1");
    assert_eq!(find("backend api").unwrap(), "id-1", "忽略大小写");
    assert_eq!(find("backend").unwrap(), "id-1", "唯一片段");
    let e = find("web").unwrap_err();
    assert!(e.contains("多个") && e.contains("Web Frontend") && e.contains("Web Admin"), "{e}");
    let e = find("nothing").unwrap_err();
    assert!(e.contains("现有项目") && e.contains("Backend API"), "{e}");
    // 兼容旧版参数名
    assert_eq!(tools::find_project(&st, &json!({ "name": "Web Admin" })).unwrap().id, "id-3");
    assert_eq!(tools::find_project(&st, &json!({ "id": "id-1" })).unwrap().id, "id-1");
    assert!(tools::find_project(&st, &json!({})).unwrap_err().contains("缺少参数 project"));
}

#[test]
fn 参数解析宽松接受字符串数字() {
    let v = json!({ "a": "42", "b": 7, "c": "", "d": "true", "e": true, "f": " x " });
    let a = tools::A(&v);
    assert_eq!(a.num("a"), Some(42));
    assert_eq!(a.num("b"), Some(7));
    assert_eq!(a.str("c"), None, "空串当没传");
    assert!(a.bool("d", false) && a.bool("e", false) && !a.bool("missing", false));
    assert_eq!(a.str("f"), Some("x"));
    assert_eq!(a.clamp("a", 1, 1, 10), 10);
    assert_eq!(a.clamp("missing", 5, 1, 10), 5);
}

// ---------------------------------------------------------------- 只读工具

#[test]
fn 总览_列表_详情() {
    let st = state_with(vec![
        json!({ "id": "a", "name": "alpha", "path": workdir(), "start_command": "x", "group": "G1", "kind": "backend", "port": 18080, "env": { "DB_PASSWORD": "s3cret-value" } }),
        json!({ "id": "b", "name": "beta", "path": workdir(), "start_command": "y", "kind": "frontend", "depends_on": ["a"] }),
    ]);
    st.config.lock().unwrap().project_groups = vec!["G1".into()];

    let (text, data) = ok_tool(&st, "devbox_overview", json!({}));
    assert!(text.contains("共 2 个项目") && text.contains("【G1】") && text.contains("【未分组】"), "{text}");
    assert_eq!(data["total"], 2);
    assert_eq!(data["running"], 0);

    let (_, data) = ok_tool(&st, "devbox_list_projects", json!({ "kind": "frontend" }));
    assert_eq!(data["count"], 1);
    assert_eq!(data["projects"][0]["name"], "beta");
    assert_eq!(data["projects"][0]["depends_on"], json!(["alpha"]));
    let (_, data) = ok_tool(&st, "devbox_list_projects", json!({ "group": "G1" }));
    assert_eq!(data["projects"][0]["name"], "alpha");
    let (_, data) = ok_tool(&st, "devbox_list_projects", json!({ "group": "未分组" }));
    assert_eq!(data["projects"][0]["name"], "beta");
    let (_, data) = ok_tool(&st, "devbox_list_projects", json!({ "running": true }));
    assert_eq!(data["count"], 0);
    let (_, data) = ok_tool(&st, "devbox_list_projects", json!({ "query": "ALP" }));
    assert_eq!(data["count"], 1);

    // 详情：环境变量只给名字，绝不泄露值
    let (text, data) = ok_tool(&st, "devbox_get_project", json!({ "project": "alpha" }));
    assert_eq!(data["env_var_names"], json!(["DB_PASSWORD"]));
    assert!(!text.contains("s3cret-value") && !data.to_string().contains("s3cret-value"));
    for name in ["devbox_overview", "devbox_list_projects", "devbox_project_status"] {
        let (t, d) = ok_tool(&st, name, json!({}));
        assert!(!t.contains("s3cret-value") && !d.to_string().contains("s3cret-value"), "{name} 泄露了环境变量");
    }

    let (_, data) = ok_tool(&st, "devbox_list_groups", json!({}));
    let groups = data["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0]["group"], "G1");
}

#[test]
fn 期望端口被外部进程占用时详情会指出() {
    let port = free_port();
    let st = state_with(vec![json!({ "id": "a", "name": "alpha", "path": workdir(), "start_command": "x", "port": port })]);
    let mut other = crate::testutil::spawn_listener(port);
    let (text, data) = ok_tool(&st, "devbox_get_project", json!({ "project": "alpha" }));
    assert!(text.contains("被占用"), "{text}");
    assert!(data["port_holders"].as_array().is_some_and(|a| !a.is_empty()));

    let (text, data) = ok_tool(&st, "devbox_check_port", json!({ "port": port }));
    assert!(text.contains("被占用"), "{text}");
    assert_eq!(data["free"], false);
    let (text, _) = ok_tool(&st, "devbox_check_port", json!({ "project": "alpha" }));
    assert!(text.contains("被占用"), "{text}");
    let _ = other.kill();
    let _ = other.wait();

    let (text, data) = ok_tool(&st, "devbox_check_port", json!({ "port": free_port() }));
    assert!(text.contains("空闲") && data["free"] == true);
    let (err, text, _) = tool(&st, "devbox_check_port", json!({ "port": 99999 }));
    assert!(err && text.contains("1-65535"));
    let (err, _, _) = tool(&st, "devbox_check_port", json!({}));
    assert!(err);
}

#[test]
fn 日志工具_过滤_增量_省略提示() {
    let st = state_with(vec![proj("a", "alpha")]);
    let buf = st.log_buffer("a");
    buf.push("stdout", "\u{1b}[32mserver started\u{1b}[0m".into());
    buf.push("stderr", "WARN deprecated".into());
    buf.push("stderr", "java.lang.IllegalStateException: boom".into());
    buf.push("stderr", "    at com.foo.Bar.run(Bar.java:1)".into());
    buf.push("stdout", "request done".into());

    let (text, data) = ok_tool(&st, "devbox_project_logs", json!({ "project": "alpha" }));
    assert!(text.contains("server started") && !text.contains('\u{1b}'), "颜色代码应被去掉：{text}");
    assert!(text.contains("[ERROR] java.lang.IllegalStateException"));
    assert_eq!(data["next_since"], 5);
    assert_eq!(data["shown"], 5);

    let (text, data) = ok_tool(&st, "devbox_project_logs", json!({ "project": "alpha", "level": "error" }));
    assert!(text.contains("IllegalStateException") && text.contains("at com.foo.Bar.run"), "错误要带堆栈：{text}");
    assert!(!text.contains("request done"));
    assert_eq!(data["matched"], 1);

    let (text, _) = ok_tool(&st, "devbox_project_logs", json!({ "project": "alpha", "grep": "deprecated|nothing", "context": 1 }));
    assert!(text.contains("deprecated") && text.contains("server started") && text.contains("IllegalStateException"), "{text}");
    assert!(!text.contains("request done"));

    // 增量：只拿 since_seq 之后的
    buf.push("stdout", "later line".into());
    let (text, data) = ok_tool(&st, "devbox_project_logs", json!({ "project": "alpha", "since_seq": 5 }));
    assert!(text.contains("later line") && !text.contains("server started"));
    assert_eq!(data["next_since"], 6);

    // tail 截断要说明
    let (text, data) = ok_tool(&st, "devbox_project_logs", json!({ "project": "alpha", "tail": 2 }));
    assert_eq!(data["shown"], 2);
    assert!(text.contains("省略"), "{text}");

    let (err, text, _) = tool(&st, "devbox_project_logs", json!({ "project": "alpha", "level": "fatal" }));
    assert!(err && text.contains("level"));

    let (text, _) = ok_tool(&st, "devbox_project_logs", json!({ "project": "alpha", "grep": "zzz-not-there" }));
    assert!(text.contains("没有符合条件"));
}

#[test]
fn 清空日志() {
    let st = state_with(vec![proj("a", "alpha")]);
    st.log_buffer("a").push("stdout", "hello".into());
    ok_tool(&st, "devbox_clear_logs", json!({ "project": "alpha" }));
    let (_, data) = ok_tool(&st, "devbox_project_logs", json!({ "project": "alpha" }));
    assert_eq!(data["buffered"], 0);
}

#[test]
fn 等待日志_项目没运行时立刻返回() {
    let st = state_with(vec![proj("a", "alpha")]);
    st.log_buffer("a").push("system", "✖ 进程已退出：退出码 1".into());
    let t = std::time::Instant::now();
    let (text, data) = ok_tool(&st, "devbox_wait_for_log", json!({ "project": "alpha", "pattern": "started", "timeout_secs": 30 }));
    assert!(t.elapsed().as_secs() < 5, "不该白等到超时");
    assert_eq!(data["exited"], true);
    assert!(text.contains("没有在运行") && text.contains("退出码 1"), "{text}");

    let (err, text, _) = tool(&st, "devbox_wait_for_log", json!({ "project": "alpha" }));
    assert!(err && text.contains("pattern"));
}

#[test]
fn 等待日志_运行中等到匹配与超时() {
    let st = state_with(vec![proj("a", "alpha")]);
    process::start_project_inner("a", &st).unwrap();
    let buf = st.log_buffer("a");

    std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(400));
            buf.push("stdout", "noise".into());
            buf.push("stdout", "Server READY on 3000".into());
        });
        let t = std::time::Instant::now();
        let (text, data) = ok_tool(&st, "devbox_wait_for_log", json!({ "project": "alpha", "pattern": "ready on|listening", "timeout_secs": 10 }));
        assert_eq!(data["matched"], true, "{text}");
        assert!(text.contains("Server READY on 3000"));
        assert!(t.elapsed().as_secs() < 8);
    });

    // 等级别：新出现错误
    std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(300));
            buf.push("stderr", "TypeError: x is undefined".into());
        });
        let (text, data) = ok_tool(&st, "devbox_wait_for_log", json!({ "project": "alpha", "level": "error", "timeout_secs": 10 }));
        assert_eq!(data["matched"], true, "{text}");
    });

    // 超时：进程还在，但没等到
    let (text, data) = ok_tool(&st, "devbox_wait_for_log", json!({ "project": "alpha", "pattern": "never-appears", "timeout_secs": 1 }));
    assert_eq!(data["timed_out"], true, "{text}");

    let _ = process::stop_project_inner("a", &st);
}

#[test]
fn 最近崩溃记录() {
    let st = state_with(vec![proj("a", "alpha")]);
    let (text, data) = ok_tool(&st, "devbox_recent_exits", json!({}));
    assert!(text.contains("还没有") && data["exits"].as_array().unwrap().is_empty());
    st.exit_history.lock().unwrap().push(crate::models::ExitEvent {
        id: "a".into(),
        name: "alpha".into(),
        ts: "10:00:00".into(),
        message: "进程已退出：退出码 1\n最后输出：\nboom".into(),
    });
    let (text, data) = ok_tool(&st, "devbox_recent_exits", json!({}));
    assert!(text.contains("alpha") && text.contains("boom"), "{text}");
    assert_eq!(data["exits"].as_array().unwrap().len(), 1);
    let (text, _) = ok_tool(&st, "devbox_overview", json!({}));
    assert!(text.contains("最近的崩溃"), "{text}");
}

// ---------------------------------------------------------------- 启停

#[test]
fn 启动_重复启动_停止_重启() {
    let st = state_with(vec![proj("a", "alpha")]);
    let (text, data) = ok_tool(&st, "devbox_start_project", json!({ "project": "alpha" }));
    assert_eq!(data["project"]["running"], true, "{text}");
    assert!(data["project"]["pid"].as_u64().unwrap() > 0);

    let (text, data) = ok_tool(&st, "devbox_start_project", json!({ "project": "alpha" }));
    assert!(text.contains("已经在运行"), "{text}");
    assert_eq!(data["outcome"], "already_running");

    let (text, data) = ok_tool(&st, "devbox_restart_project", json!({ "project": "alpha" }));
    assert_eq!(data["project"]["running"], true, "{text}");

    let (text, data) = ok_tool(&st, "devbox_stop_project", json!({ "project": "alpha" }));
    assert!(text.contains("已停止") && data["was_running"] == true);
    let (text, data) = ok_tool(&st, "devbox_stop_project", json!({ "project": "alpha" }));
    assert!(text.contains("本来就没有"), "{text}");
    assert_eq!(data["was_running"], false);

    let (_, data) = ok_tool(&st, "devbox_project_status", json!({ "project": "alpha" }));
    assert_eq!(data["projects"][0]["running"], false);
}

#[test]
fn 配置了端口的项目启动后端口监听() {
    let port = free_port();
    let st = state_with(vec![json!({ "id": "a", "name": "alpha", "path": workdir(), "start_command": listen_command(port), "port": port })]);
    let (text, data) = ok_tool(&st, "devbox_start_project", json!({ "project": "alpha" }));
    assert_eq!(data["project"]["port_listening"], true, "{text}");
    assert!(text.contains("已监听"));
    let (_, data) = ok_tool(&st, "devbox_list_ports", json!({ "query": &port.to_string() }));
    assert!(data["count"].as_u64().unwrap() >= 1);
    ok_tool(&st, "devbox_stop_project", json!({ "project": "alpha" }));
}

#[test]
fn 启动失败时带回原因() {
    let st = state_with(vec![json!({ "id": "a", "name": "alpha", "path": workdir(), "start_command": "" })]);
    let (err, text, _) = tool(&st, "devbox_start_project", json!({ "project": "alpha" }));
    assert!(err && text.contains("启动命令为空"), "{text}");
}

#[test]
fn 项目组启停与全部停止() {
    let st = state_with(vec![
        json!({ "id": "a", "name": "alpha", "path": workdir(), "start_command": sleep_command(30), "group": "G" }),
        json!({ "id": "b", "name": "beta", "path": workdir(), "start_command": sleep_command(30), "group": "G" }),
        json!({ "id": "c", "name": "gamma", "path": workdir(), "start_command": sleep_command(30) }),
    ]);
    let (text, data) = ok_tool(&st, "devbox_start_group", json!({ "group": "G" }));
    assert!(text.contains("2 个成功"), "{text}");
    assert_eq!(data["results"].as_array().unwrap().len(), 2);
    assert_eq!(process::running_ids(&st).len(), 2);

    // 再启动一次：已在运行的跳过
    let (_, data) = ok_tool(&st, "devbox_start_group", json!({ "group": "G" }));
    assert!(data["results"].as_array().unwrap().iter().all(|r| r["result"] == "already_running"));

    ok_tool(&st, "devbox_start_project", json!({ "project": "gamma" }));
    let (text, _) = ok_tool(&st, "devbox_stop_group", json!({ "group": "G" }));
    assert!(text.contains("2 个项目"), "{text}");
    assert_eq!(process::running_ids(&st), vec!["c".to_string()]);

    let (text, data) = ok_tool(&st, "devbox_stop_all", json!({}));
    assert_eq!(data["stopped"], 1, "{text}");

    let (err, text, _) = tool(&st, "devbox_start_group", json!({ "group": "不存在" }));
    assert!(err && text.contains("不存在"));
}

// ---------------------------------------------------------------- 脚本

#[test]
fn 只允许运行识别出的脚本() {
    let dir = std::env::temp_dir().join(format!("devbox-mcp-script-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("package.json"), r#"{"name":"x","scripts":{"build":"echo built","lint":"echo linted"}}"#).unwrap();
    let st = state_with(vec![json!({ "id": "a", "name": "alpha", "path": dir.to_string_lossy(), "start_command": "x" })]);

    let (text, data) = ok_tool(&st, "devbox_list_scripts", json!({ "project": "alpha" }));
    assert!(text.contains("build") && text.contains("lint"), "{text}");
    assert!(data["scripts"].as_array().unwrap().len() >= 2);

    // 任意命令不能执行
    let marker = dir.join("pwned");
    let evil = format!("touch {}", marker.display());
    let (err, text, _) = tool(&st, "devbox_run_script", json!({ "project": "alpha", "script": evil }));
    assert!(err && text.contains("不是") && text.contains("可用脚本"), "{text}");
    assert!(!marker.exists());

    // 识别出的脚本可以运行（结果取决于本机有没有 npm，这里只关心没有被「不是脚本」拦住）
    let (_, text, _) = tool(&st, "devbox_run_script", json!({ "project": "alpha", "script": "BUILD", "wait_secs": 20 }));
    assert!(!text.contains("不是") || !text.contains("可用脚本"), "{text}");

    let (err, text, _) = tool(&st, "devbox_cancel_script", json!({ "project": "alpha" }));
    assert!(err && text.contains("没有正在运行"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- 完全档

#[test]
fn 结束进程拒绝自己和系统进程() {
    let st = state_with(vec![]);
    set_mode(&st, McpMode::Full);
    let (err, text, _) = tool(&st, "devbox_kill_port", json!({ "pid": std::process::id() }));
    assert!(err && text.contains("拒绝"), "{text}");
    let (err, text, _) = tool(&st, "devbox_kill_port", json!({ "pid": 1 }));
    assert!(err && text.contains("拒绝"), "{text}");
    let (err, text, _) = tool(&st, "devbox_kill_port", json!({}));
    assert!(err && text.contains("port 或 pid"));
    let (text, _) = ok_tool(&st, "devbox_kill_port", json!({ "port": free_port() }));
    assert!(text.contains("空闲"), "{text}");
}

#[test]
fn 结束外部进程占用的端口() {
    let st = state_with(vec![]);
    set_mode(&st, McpMode::Full);
    let port = free_port();
    let mut other = crate::testutil::spawn_listener(port);
    let (text, data) = ok_tool(&st, "devbox_kill_port", json!({ "port": port }));
    assert!(text.contains("已结束"), "{text}");
    assert_eq!(data["killed"].as_array().unwrap().len(), 1);
    let _ = other.wait();
    assert!(!crate::commands::ports::who_holds(&st, port).iter().any(|h| h.pid == other.id()));
}

#[test]
fn hosts_记录进入系统文件前要校验() {
    use crate::models::HostEntry;
    let e = |ip: &str, hosts: &[&str], comment: Option<&str>| HostEntry {
        ip: ip.into(),
        hostnames: hosts.iter().map(|s| s.to_string()).collect(),
        enabled: true,
        comment: comment.map(|s| s.to_string()),
    };
    assert!(write::validate_host_entry(&e("127.0.0.1", &["api.local", "*.dev_x-1"], Some("备注"))).is_ok());
    assert!(write::validate_host_entry(&e("::1", &["a.local"], None)).is_ok());
    assert!(write::validate_host_entry(&e("not-an-ip", &["a.local"], None)).is_err());
    assert!(write::validate_host_entry(&e("127.0.0.1\n0.0.0.0", &["a.local"], None)).is_err());
    assert!(write::validate_host_entry(&e("127.0.0.1", &[], None)).is_err());
    assert!(write::validate_host_entry(&e("127.0.0.1", &["a.local\n10.0.0.1 evil.com"], None)).is_err());
    assert!(write::validate_host_entry(&e("127.0.0.1", &["a b"], None)).is_err());
    assert!(write::validate_host_entry(&e("127.0.0.1", &["a.local # x"], None)).is_err());
    assert!(write::validate_host_entry(&e("127.0.0.1", &["a.local"], Some("x\nevil"))).is_err());

    let st = state_with(vec![]);
    set_mode(&st, McpMode::Full);
    let (err, text, _) = tool(&st, "devbox_save_hosts", json!({ "entries": [{ "ip": "1.2.3.4", "hostnames": ["bad host"], "enabled": true }] }));
    assert!(err && text.contains("不合法"), "{text}");
    let (err, text, _) = tool(&st, "devbox_save_hosts", json!({ "entries": "oops" }));
    assert!(err && text.contains("格式"), "{text}");
}

#[test]
fn 增改删项目() {
    let dir = std::env::temp_dir().join(format!("devbox-mcp-add-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = state_with(vec![proj("base", "base-svc")]);
    set_mode(&st, McpMode::Full);

    let (err, text, _) = tool(&st, "devbox_add_project", json!({ "path": "relative/dir" }));
    assert!(err && text.contains("绝对路径"));

    let (text, data) = ok_tool(
        &st,
        "devbox_add_project",
        json!({
            "path": dir.to_string_lossy(), "name": "新服务", "start_command": "echo hi", "kind": "backend", "port": 18123,
            "group": "后端", "env": { "A": "1", "N": 2 }, "depends_on": ["base-svc"], "auto_restart": true,
        }),
    );
    assert!(text.contains("已添加"), "{text}");
    assert_eq!(data["project"]["depends_on"], json!(["base-svc"]));
    {
        let cfg = st.config.lock().unwrap();
        let p = cfg.projects.iter().find(|p| p.name == "新服务").unwrap();
        assert!(!p.id.is_empty() && p.auto_restart && p.port == Some(18123));
        assert_eq!(p.env.get("N").map(|s| s.as_str()), Some("2"));
        assert_eq!(p.group.as_deref(), Some("后端"));
        assert!(cfg.project_groups.contains(&"后端".to_string()), "分组应被自动登记");
    }

    // 重名
    let (err, text, _) = tool(&st, "devbox_add_project", json!({ "path": dir.to_string_lossy(), "name": "新服务", "start_command": "x" }));
    assert!(err && text.contains("已经有"), "{text}");
    // 依赖不存在
    let (err, _, _) = tool(&st, "devbox_add_project", json!({ "path": dir.to_string_lossy(), "name": "另一个", "start_command": "x", "depends_on": ["不存在"] }));
    assert!(err);

    // 修改：只动传了的字段
    let (text, data) = ok_tool(&st, "devbox_update_project", json!({ "project": "新服务", "port": 18124, "url": "http://localhost:18124", "env": { "ONLY": "x" } }));
    assert!(text.contains("port") && text.contains("env"), "{text}");
    assert_eq!(data["restart_needed"], false);
    {
        let cfg = st.config.lock().unwrap();
        let p = cfg.projects.iter().find(|p| p.name == "新服务").unwrap();
        assert_eq!(p.port, Some(18124));
        assert_eq!(p.start_command, "echo hi", "没传的字段保持不变");
        assert_eq!(p.env.len(), 1);
        assert_eq!(p.depends_on.len(), 1);
    }
    // 清除可选字段
    ok_tool(&st, "devbox_update_project", json!({ "project": "新服务", "port": 0, "url": "", "group": "" }));
    {
        let cfg = st.config.lock().unwrap();
        let p = cfg.projects.iter().find(|p| p.name == "新服务").unwrap();
        assert!(p.port.is_none() && p.url.is_none() && p.group.is_none());
    }
    let (err, text, _) = tool(&st, "devbox_update_project", json!({ "project": "新服务" }));
    assert!(err && text.contains("没有传"));
    let (err, text, _) = tool(&st, "devbox_update_project", json!({ "project": "新服务", "name": "BASE-SVC" }));
    assert!(err && text.contains("已经有"), "名字不区分大小写：{text}");
    let (err, _, _) = tool(&st, "devbox_update_project", json!({ "project": "新服务", "depends_on": ["新服务"] }));
    assert!(err, "不能依赖自己");
    let (err, _, _) = tool(&st, "devbox_update_project", json!({ "project": "新服务", "kind": "weird" }));
    assert!(err);

    // 删除要确认
    let (err, text, _) = tool(&st, "devbox_delete_project", json!({ "project": "新服务" }));
    assert!(err && text.contains("confirm"));
    assert_eq!(st.config.lock().unwrap().projects.len(), 2);
    ok_tool(&st, "devbox_delete_project", json!({ "project": "新服务", "confirm": true }));
    assert_eq!(st.config.lock().unwrap().projects.len(), 1);
    assert!(dir.exists(), "磁盘上的代码不动");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 切换环境() {
    let dir = std::env::temp_dir().join(format!("devbox-mcp-env-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".env.staging"), "PORT=3001\n").unwrap();
    std::fs::write(dir.join(".env.production"), "PORT=3002\n").unwrap();
    std::fs::write(dir.join("package.json"), r#"{"devDependencies":{"vite":"^5"},"scripts":{"dev":"vite"}}"#).unwrap();
    let st = state_with(vec![json!({ "id": "a", "name": "alpha", "path": dir.to_string_lossy(), "start_command": "npm run dev" })]);

    let (text, data) = ok_tool(&st, "devbox_project_envs", json!({ "project": "alpha" }));
    let profiles = data["projects"][0]["profiles"].as_array().unwrap();
    assert!(profiles.len() >= 2, "{text}");
    let first = profiles[0]["name"].as_str().unwrap().to_string();

    let (text, data) = ok_tool(&st, "devbox_set_env", json!({ "project": "alpha", "env": first }));
    assert!(text.contains("已切换"), "{text}");
    assert_eq!(data["restart_needed"], false);
    assert_eq!(st.config.lock().unwrap().projects[0].active_env.as_deref(), Some(first.as_str()));

    let (err, text, _) = tool(&st, "devbox_set_env", json!({ "project": "alpha", "env": "不存在的环境" }));
    assert!(err && text.contains("没有识别到"), "{text}");
    ok_tool(&st, "devbox_set_env", json!({ "project": "alpha" }));
    assert!(st.config.lock().unwrap().projects[0].active_env.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- 调用记录、资源、提示词

#[test]
fn 每次调用都留下记录() {
    let st = state_with(vec![proj("a", "alpha")]);
    ok_tool(&st, "devbox_project_status", json!({ "project": "alpha" }));
    tool(&st, "devbox_stop_project", json!({ "project": "不存在" }));
    set_mode(&st, McpMode::Readonly);
    tool(&st, "devbox_stop_project", json!({ "project": "alpha" }));

    let act: Vec<_> = st.mcp.activity.lock().unwrap().iter().cloned().collect();
    assert_eq!(act.len(), 3);
    assert_eq!((act[0].tool.as_str(), act[0].ok, act[0].target.as_deref()), ("devbox_project_status", true, Some("alpha")));
    assert!(!act[1].ok && act[1].error.as_ref().unwrap().contains("找不到项目"));
    assert!(!act[2].ok && act[2].error.as_ref().unwrap().contains("权限档位"), "被拒绝的调用也要留痕");
    assert_eq!(st.mcp.calls.load(Ordering::Relaxed), 3);

    let info = info_of(&st);
    assert_eq!(info.calls_total, 3);
    assert_eq!(info.url, "http://127.0.0.1:8765/mcp");
    assert_eq!(info.mode, McpMode::Readonly);
    assert!(info.tools.iter().any(|t| t.name == "devbox_start_project" && !t.allowed));
    assert!(info.tools.iter().any(|t| t.name == "devbox_project_logs" && t.allowed));
}

#[test]
fn 调用记录有上限() {
    let st = state_with(vec![]);
    for i in 0..(MAX_ACTIVITY + 25) {
        st.mcp.record(Activity { ts: "t".into(), tool: format!("t{i}"), target: None, ok: true, ms: 0, error: None, client: "c".into() });
    }
    let q = st.mcp.activity.lock().unwrap();
    assert_eq!(q.len(), MAX_ACTIVITY);
    assert_eq!(q.front().unwrap().tool, "t25", "丢弃最旧的");
}

#[test]
fn 资源可列出可读取() {
    let st = state_with(vec![proj("a", "my app")]);
    st.log_buffer("a").push("stdout", "hello from app".into());

    let v = rpc(&st, "resources/list", json!({}));
    let uris: Vec<&str> = v["result"]["resources"].as_array().unwrap().iter().map(|r| r["uri"].as_str().unwrap()).collect();
    assert!(uris.contains(&"devbox://overview") && uris.contains(&"devbox://projects"));
    assert!(uris.contains(&"devbox://logs/my%20app"), "{uris:?}");

    let v = rpc(&st, "resources/read", json!({ "uri": "devbox://logs/my%20app" }));
    assert!(v["result"]["contents"][0]["text"].as_str().unwrap().contains("hello from app"));
    let v = rpc(&st, "resources/read", json!({ "uri": "devbox://overview" }));
    assert!(v["result"]["contents"][0]["text"].as_str().unwrap().contains("my app"));
    let v = rpc(&st, "resources/read", json!({ "uri": "devbox://projects" }));
    let parsed: Value = serde_json::from_str(v["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(parsed["count"], 1);

    let v = rpc(&st, "resources/read", json!({ "uri": "devbox://nope" }));
    assert_eq!(v["error"]["code"], -32002);
    let v = rpc(&st, "resources/read", json!({ "uri": "devbox://logs/not-there" }));
    assert_eq!(v["error"]["code"], -32002);
    let v = rpc(&st, "resources/read", json!({}));
    assert_eq!(v["error"]["code"], -32602);
    let v = rpc(&st, "resources/templates/list", json!({}));
    assert_eq!(v["result"]["resourceTemplates"][0]["uriTemplate"], "devbox://logs/{project}");
}

#[test]
fn 提示词可列出可获取() {
    let st = state_with(vec![]);
    let v = rpc(&st, "prompts/list", json!({}));
    let list = v["result"]["prompts"].as_array().unwrap();
    assert!(list.len() >= 4);
    for p in list {
        let required: Vec<_> = p["arguments"].as_array().unwrap().iter().filter(|a| a["required"] == true).map(|a| a["name"].as_str().unwrap()).collect();
        let args: serde_json::Map<String, Value> = required.iter().map(|k| (k.to_string(), json!("demo"))).collect();
        let v = rpc(&st, "prompts/get", json!({ "name": p["name"], "arguments": args }));
        let text = v["result"]["messages"][0]["content"]["text"].as_str().unwrap_or("");
        assert!(text.contains("devbox_") || text.contains("DevBox"), "{}：{v}", p["name"]);
    }
    let v = rpc(&st, "prompts/get", json!({ "name": "diagnose_project", "arguments": {} }));
    assert_eq!(v["error"]["code"], -32602);
    let v = rpc(&st, "prompts/get", json!({ "name": "nope" }));
    assert_eq!(v["error"]["code"], -32602);
}

/// 文本里出现的所有 devbox_xxx 名字
fn mentioned_tools(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("devbox_") {
        let tail = &rest[i..];
        let end = tail.find(|c: char| !(c.is_ascii_lowercase() || c == '_')).unwrap_or(tail.len());
        out.push(tail[..end].trim_end_matches('_').to_string());
        rest = &tail[end..];
    }
    out
}

#[test]
fn 说明和提示词里提到的工具都真实存在() {
    let st = state_with(vec![]);
    set_mode(&st, McpMode::Full);
    let mut texts = vec![rpc(&st, "initialize", json!({}))["result"]["instructions"].as_str().unwrap().to_string()];
    let list = rpc(&st, "prompts/list", json!({}));
    for p in list["result"]["prompts"].as_array().unwrap() {
        let v = rpc(&st, "prompts/get", json!({ "name": p["name"], "arguments": { "project": "x", "port": "1", "group": "g" } }));
        texts.push(v["result"]["messages"][0]["content"]["text"].as_str().unwrap().to_string());
    }
    for t in tools::all() {
        texts.push(t.description.to_string());
    }
    for text in texts {
        for name in mentioned_tools(&text) {
            assert!(tools::find(&name).is_some(), "文字里提到了不存在的工具 {name}：{text}");
        }
    }
}

#[test]
fn 使用说明里列全了所有工具() {
    let readme = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../devbox-mcp/README.md")).expect("devbox-mcp/README.md");
    for t in tools::all() {
        assert!(readme.contains(t.name), "devbox-mcp/README.md 没有介绍工具 {}", t.name);
    }
}

// ---------------------------------------------------------------- 持久化

#[test]
fn bridge_文件保存端口和_token_重启后沿用() {
    let st = state_with(vec![]);
    st.mcp.port.store(8771, Ordering::SeqCst);
    write_bridge_file(&st);
    let (port, token) = previous_bridge(&st);
    assert_eq!(port, Some(8771));
    assert_eq!(token.as_deref(), Some(TOKEN));

    let text = std::fs::read_to_string(bridge_path(&st)).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["mcp_url"], "http://127.0.0.1:8771/mcp");
    assert_eq!(v["base_url"], "http://127.0.0.1:8771");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(bridge_path(&st)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "含 token 的文件不能让别的用户读");
    }

    // 太短的 token 视为损坏，重新生成
    std::fs::write(bridge_path(&st), r#"{"port": 8771, "token": "short"}"#).unwrap();
    assert_eq!(previous_bridge(&st), (Some(8771), None));
    std::fs::write(bridge_path(&st), "不是 json").unwrap();
    assert_eq!(previous_bridge(&st), (None, None));
    assert!(new_token().len() >= 32 && new_token() != new_token());
}

#[test]
fn 设置的序列化与旧配置兼容() {
    let cfg: Config = serde_json::from_str(r#"{"projects": []}"#).unwrap();
    assert_eq!(cfg.mcp, McpSettings { enabled: true, mode: McpMode::Standard }, "旧配置没有 mcp 字段：默认开启 + 标准档");
    let cfg: Config = serde_json::from_str(r#"{"mcp": {"mode": "readonly"}}"#).unwrap();
    assert!(cfg.mcp.enabled && cfg.mcp.mode == McpMode::Readonly);
    let json = serde_json::to_string(&McpSettings { enabled: false, mode: McpMode::Full }).unwrap();
    assert_eq!(json, r#"{"enabled":false,"mode":"full"}"#);
}

#[test]
fn 主机名判断() {
    for h in ["127.0.0.1", "127.0.0.1:8765", "localhost", "localhost:3000", "[::1]", "[::1]:8765"] {
        assert!(is_local_host(h), "{h}");
    }
    for h in ["example.com", "127.0.0.1.evil.com", "evil.com:8765", "10.0.0.1", ""] {
        assert!(!is_local_host(h), "{h}");
    }
    assert!(origin_is_local("http://localhost:5173"));
    assert!(origin_is_local("http://127.0.0.1:8765/"));
    assert!(!origin_is_local("https://evil.com"));
    assert!(!origin_is_local("null"));
    assert!(constant_time_eq("abc", "abc") && !constant_time_eq("abc", "abd") && !constant_time_eq("abc", "ab"));
}

// ---------------------------------------------------------------- 真实 socket

/// 用 mock 应用起一个真正的 HTTP 服务（随机端口），返回端口
fn spawn_real_server(st: AppState) -> (u16, tauri::App<tauri::test::MockRuntime>) {
    use tauri::Manager;
    let app = tauri::test::mock_app();
    app.manage(st);
    let server = Server::http(("127.0.0.1", 0)).unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    app.state::<AppState>().mcp.port.store(port, Ordering::SeqCst);
    let handle = app.handle().clone();
    std::thread::spawn(move || run_loop(handle, server));
    (port, app)
}

/// 最朴素的 HTTP/1.1 客户端：发一个请求，读到连接关闭，返回 (状态码, 响应头文本, 响应体)
fn http(port: u16, method: &str, path: &str, headers: &[(&str, String)], body: &str) -> (u16, String, String) {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(20))).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).unwrap();
    s.write_all(body.as_bytes()).unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    (status, head.to_string(), body.to_string())
}

fn auth() -> Vec<(&'static str, String)> {
    vec![("Authorization", format!("Bearer {TOKEN}")), ("Content-Type", "application/json".into())]
}

#[test]
fn 真实_socket_鉴权与正常调用() {
    let (port, _app) = spawn_real_server(state_with(vec![proj("a", "alpha")]));

    let (status, _, body) = http(port, "GET", "/health", &[], "");
    assert_eq!(status, 200);
    assert!(body.contains("\"name\":\"devbox\""));

    let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }).to_string();
    let (status, head, _) = http(port, "POST", "/mcp", &[], &ping);
    assert_eq!(status, 401);
    assert!(head.to_lowercase().contains("www-authenticate: bearer"), "{head}");

    let (status, head, body) = http(port, "POST", "/mcp", &auth(), &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }).to_string());
    assert_eq!(status, 200, "{body}");
    assert!(head.to_lowercase().contains("application/json"), "{head}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert!(v["result"]["tools"].as_array().unwrap().len() >= 20);

    // 通过 X-DevBox-Token 也行；通知得到 202 且没有内容
    let (status, _, body) = http(port, "POST", "/mcp", &[("X-DevBox-Token", TOKEN.into())], &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string());
    assert_eq!((status, body.as_str()), (202, ""));

    // 真正调一个工具
    let call = json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "devbox_overview", "arguments": {} } }).to_string();
    let (status, _, body) = http(port, "POST", "/mcp", &auth(), &call);
    assert_eq!(status, 200);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert!(v["result"]["content"][0]["text"].as_str().unwrap().contains("alpha"));

    // 伪造的 Host / Origin 即使 token 对也拒绝
    let (status, _, _) = http(port, "POST", "/mcp", &[("Authorization", format!("Bearer {TOKEN}")), ("Origin", "https://evil.example".into())], &ping);
    assert_eq!(status, 403);
}

#[test]
fn 真实_socket_请求体过大被拒绝() {
    let (port, _app) = spawn_real_server(state_with(vec![]));
    let big = "x".repeat(MAX_BODY as usize + 10);
    let (status, _, _) = http(port, "POST", "/mcp", &auth(), &big);
    assert_eq!(status, 413);
}

#[test]
fn 真实_socket_慢请求不会堵住别的请求() {
    let st = state_with(vec![proj("a", "alpha")]);
    process::start_project_inner("a", &st).unwrap();
    let (port, app) = spawn_real_server(st);

    let slow = std::thread::spawn(move || {
        let call = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "devbox_wait_for_log", "arguments": { "project": "alpha", "pattern": "never-appears-xyz", "timeout_secs": 3 } } }).to_string();
        let t = std::time::Instant::now();
        let (status, _, body) = http(port, "POST", "/mcp", &auth(), &call);
        (status, body, t.elapsed())
    });
    std::thread::sleep(std::time::Duration::from_millis(400));
    // 慢请求还在等的时候，别的请求应该马上得到应答
    let t = std::time::Instant::now();
    let (status, _, _) = http(port, "POST", "/mcp", &auth(), &json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" }).to_string());
    assert_eq!(status, 200);
    assert!(t.elapsed().as_millis() < 1500, "被慢请求堵住了：{:?}", t.elapsed());

    let (status, body, took) = slow.join().unwrap();
    assert_eq!(status, 200);
    assert!(took.as_secs() >= 2, "{took:?}");
    assert!(body.contains("timed_out"), "{body}");

    use tauri::Manager;
    let _ = process::stop_project_inner("a", app.state::<AppState>().inner());
}

#[test]
fn 真实_socket_stdio_转发器端到端() {
    // 本机有 node 才跑：把真正的 devbox-mcp/index.js 接到真正的服务上
    let (port, _app) = spawn_real_server(state_with(vec![proj("a", "alpha")]));
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../devbox-mcp/index.js");
    let out = std::process::Command::new("node")
        .arg(script)
        .arg("--check")
        .env("DEVBOX_MCP_URL", format!("http://127.0.0.1:{port}/mcp"))
        .env("DEVBOX_MCP_TOKEN", TOKEN)
        .output();
    let Ok(out) = out else { return };
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("已连接 devbox") && stdout.contains("devbox_project_logs"), "{stdout}");

    // 错误的 token：转发器给出能看懂的提示，退出码非 0
    let bad = std::process::Command::new("node").arg(script).arg("--check").env("DEVBOX_MCP_URL", format!("http://127.0.0.1:{port}/mcp")).env("DEVBOX_MCP_TOKEN", "wrong").output().unwrap();
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("token"), "{}", String::from_utf8_lossy(&bad.stderr));
}

/// 手动联调用：保持一个带示例项目的真实服务运行一段时间，供官方 MCP 客户端 / 其它 agent 连接。
/// `DEVBOX_SERVE_PORT=8799 DEVBOX_SERVE_SECS=120 cargo test --lib 手动_保持服务运行 -- --ignored --nocapture`
#[test]
#[ignore]
fn 手动_保持服务运行() {
    use tauri::Manager;
    let port: u16 = std::env::var("DEVBOX_SERVE_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8799);
    let secs: u64 = std::env::var("DEVBOX_SERVE_SECS").ok().and_then(|p| p.parse().ok()).unwrap_or(120);
    let st = state_with(vec![proj("a", "alpha"), json!({ "id": "b", "name": "beta", "path": workdir(), "start_command": sleep_command(300), "group": "G" })]);
    let app = tauri::test::mock_app();
    app.manage(st);
    let server = Server::http(("127.0.0.1", port)).unwrap();
    app.state::<AppState>().mcp.port.store(port, Ordering::SeqCst);
    let handle = app.handle().clone();
    std::thread::spawn(move || run_loop(handle, server));
    println!("MCP 服务已就绪：http://127.0.0.1:{port}/mcp  token={TOKEN}");
    std::thread::sleep(std::time::Duration::from_secs(secs));
    process::stop_all(app.state::<AppState>().inner());
}
