//! 只读工具：不改变任何东西。

use super::logq::{self, Level, Query};
use super::rpc::{mode_label, Ctx};
use super::tools::*;
use crate::commands::{envs, git, hosts, ports, process, projects, runtime, scripts, services, startcmd};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

fn to_value<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

pub fn overview(ctx: &Ctx, _args: &Value) -> Result<ToolOut, String> {
    let state = ctx.state;
    let (projects, groups) = {
        let cfg = state.config.lock().unwrap();
        (cfg.projects.clone(), cfg.project_groups.clone())
    };
    let st = statuses(state);
    let running = projects.iter().filter(|p| st.get(&p.id).is_some_and(|s| s.running)).count();

    let mut text = format!(
        "DevBox {} · 共 {} 个项目，{} 个运行中 · MCP 权限档位：{}\n",
        env!("CARGO_PKG_VERSION"),
        projects.len(),
        running,
        mode_label(ctx.mode)
    );
    // 按项目组分段显示，保持界面里的 tab 顺序
    let mut order: Vec<Option<String>> = groups.iter().cloned().map(Some).collect();
    for p in &projects {
        if !order.contains(&p.group) {
            order.push(p.group.clone());
        }
    }
    if projects.iter().any(|p| p.group.is_none()) && !order.contains(&None) {
        order.push(None);
    }
    for g in order {
        let members: Vec<_> = projects.iter().filter(|p| p.group == g).collect();
        if members.is_empty() {
            continue;
        }
        text.push_str(&format!("\n【{}】\n", g.as_deref().unwrap_or("未分组")));
        for p in members {
            text.push_str(&format!("{}\n", status_line(p, st.get(&p.id))));
        }
    }
    let exits: Vec<Value> = state.exit_history.lock().unwrap().iter().rev().take(5).map(to_value).collect();
    if !exits.is_empty() {
        text.push_str("\n最近的崩溃（详情见 devbox_recent_exits）：\n");
        for e in &exits {
            let first = e["message"].as_str().unwrap_or("").lines().next().unwrap_or("");
            text.push_str(&format!("  {} {} {}\n", e["ts"].as_str().unwrap_or(""), e["name"].as_str().unwrap_or(""), first));
        }
    }
    let items: Vec<Value> = projects.iter().map(|p| project_brief(p, st.get(&p.id), &projects)).collect();
    Ok(ToolOut::new(
        text,
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "mode": ctx.mode,
            "total": projects.len(),
            "running": running,
            "groups": groups,
            "projects": items,
            "recent_exits": exits,
        }),
    ))
}

pub fn list_projects(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let a = A(args);
    let state = ctx.state;
    let all = state.config.lock().unwrap().projects.clone();
    let st = statuses(state);
    let group = a.str("group");
    let kind = a.str("kind");
    let query = a.str("query").map(|q| q.to_lowercase());
    let want_running = args.get("running").and_then(|v| v.as_bool());

    let picked: Vec<_> = all
        .iter()
        .filter(|p| match group {
            None => true,
            Some("未分组") => p.group.is_none(),
            Some(g) => p.group.as_deref() == Some(g),
        })
        .filter(|p| kind.map_or(true, |k| p.kind == k))
        .filter(|p| want_running.map_or(true, |w| st.get(&p.id).is_some_and(|s| s.running) == w))
        .filter(|p| query.as_ref().map_or(true, |q| p.name.to_lowercase().contains(q) || p.path.to_lowercase().contains(q)))
        .collect();

    let text = if picked.is_empty() {
        "没有符合条件的项目".to_string()
    } else {
        picked.iter().map(|p| status_line(p, st.get(&p.id))).collect::<Vec<_>>().join("\n")
    };
    let items: Vec<Value> = picked.iter().map(|p| project_brief(p, st.get(&p.id), &all)).collect();
    Ok(ToolOut::new(text, json!({ "count": items.len(), "projects": items })))
}

pub fn get_project(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let state = ctx.state;
    let p = find_project(state, args)?;
    let all = state.config.lock().unwrap().projects.clone();
    let st = status_of(state, &p.id);
    let mut v = project_brief(&p, st.as_ref(), &all);

    // 环境变量只给名字，值常常是密码
    let mut env_keys: Vec<&String> = p.env.keys().collect();
    env_keys.sort();
    v["env_var_names"] = json!(env_keys);
    v["stop_command"] = json!(p.stop_command);
    v["auto_restart"] = json!(p.auto_restart);
    v["editor"] = json!(p.editor);
    v["runtime"] = json!(runtime::describe(&p));

    let branch = git::current_branch(&p.path);
    v["git"] = match &branch {
        Some(b) => {
            let g = state.git_status.lock().unwrap().get(&p.id).cloned();
            json!({
                "branch": b.name, "detached": b.detached,
                "ahead": g.as_ref().map(|g| g.ahead), "behind": g.as_ref().map(|g| g.behind),
                "dirty_files": g.as_ref().map(|g| g.dirty), "has_upstream": g.as_ref().map(|g| g.has_upstream),
            })
        }
        None => Value::Null,
    };

    let scan = envs::scan(std::path::Path::new(&p.path), &p.start_command);
    v["environments"] = json!({
        "active": p.active_env,
        "default": scan.default,
        "available": scan.profiles.iter().map(|e| json!({ "name": e.name, "source": e.source, "summary": e.summary })).collect::<Vec<_>>(),
    });

    let buf = state.log_buffer(&p.id);
    let (buffered, last_seq) = buf.with_lines(|q| (q.len(), q.back().map(|l| l.seq).unwrap_or(0)));
    v["logs"] = json!({ "buffered_lines": buffered, "last_seq": last_seq, "file": buf.path().map(|p| p.to_string_lossy().into_owned()) });

    // 没在运行、期望端口却被占了：说清楚是谁
    let running = st.as_ref().is_some_and(|s| s.running);
    let mut text = status_line(&p, st.as_ref());
    if let (Some(port), false) = (p.port, running) {
        let holders = ports::who_holds(state, port);
        if !holders.is_empty() {
            text.push_str(&format!("\n⚠ 期望端口 {port} 被占用：{}", ports::describe_holders(&holders)));
            v["port_holders"] = to_value(&holders);
        }
    }
    text.push_str(&format!("\n路径：{}\n启动命令：{}", p.path, p.start_command));
    if let Some(b) = &branch {
        text.push_str(&format!("\ngit 分支：{}", b.name));
    }
    if !scan.profiles.is_empty() {
        let names: Vec<&str> = scan.profiles.iter().map(|e| e.name.as_str()).collect();
        text.push_str(&format!("\n可选环境：{}（当前：{}）", names.join(" / "), p.active_env.as_deref().unwrap_or("默认")));
    }
    if !p.depends_on.is_empty() {
        text.push_str(&format!("\n依赖：{}", v["depends_on"].as_array().map_or(String::new(), |a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("、"))));
    }
    Ok(ToolOut::new(text, v))
}

pub fn project_status(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let state = ctx.state;
    let all = state.config.lock().unwrap().projects.clone();
    let st = statuses(state);
    let a = A(args);
    let picked: Vec<_> = if a.str("project").is_some() || a.str("id").is_some() || a.str("name").is_some() {
        vec![find_project(state, args)?]
    } else {
        all.clone()
    };
    let text = picked.iter().map(|p| status_line(p, st.get(&p.id))).collect::<Vec<_>>().join("\n");
    let items: Vec<Value> = picked.iter().map(|p| project_brief(p, st.get(&p.id), &all)).collect();
    Ok(ToolOut::new(if text.is_empty() { "还没有项目".into() } else { text }, json!({ "projects": items })))
}

/// 从参数里组装日志查询条件
fn log_query(a: &A, default_tail: usize) -> Result<Query, String> {
    Ok(Query {
        tail: a.clamp("tail", default_tail as u64, 1, 2000) as usize,
        since_seq: a.num("since_seq"),
        min_level: Level::parse(a.str("level").unwrap_or(""))?,
        grep: a.str("grep").map(logq::split_terms).unwrap_or_default(),
        exclude: a.str("exclude").map(logq::split_terms).unwrap_or_default(),
        stream: a.str("stream").map(|s| s.to_string()),
        context: a.clamp("context", 0, 0, 10) as usize,
        max_chars: a.clamp("max_chars", 30000, 1000, 100_000) as usize,
    })
}

pub fn project_logs(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let state = ctx.state;
    let p = find_project(state, args)?;
    let q = log_query(&A(args), 200)?;
    let all = state.log_buffer(&p.id).snapshot();
    let r = logq::run(&all, &q);
    let running = state.procs.lock().unwrap().contains_key(&p.id);

    let mut head = format!(
        "# {}（{}）缓冲 {} 行，匹配 {} 行，显示 {} 行",
        p.name,
        if running { "运行中" } else { "未运行" },
        r.buffered,
        r.matched,
        r.lines.len()
    );
    if r.dropped > 0 {
        head.push_str(&format!("，另有 {} 行较早的被省略（调大 tail / max_chars，或加 grep / level 缩小范围）", r.dropped));
    }
    head.push_str(&format!("\n# 下次增量拉取：since_seq={}\n", r.last_seq));
    let body = if r.lines.is_empty() { "（没有符合条件的日志）\n".to_string() } else { logq::render(&r.lines) };
    Ok(ToolOut::new(
        format!("{head}{body}"),
        json!({
            "project": p.name, "running": running, "buffered": r.buffered, "matched": r.matched,
            "shown": r.lines.len(), "dropped": r.dropped, "next_since": r.last_seq,
        }),
    ))
}

pub fn wait_for_log(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let state = ctx.state;
    let a = A(args);
    let p = find_project(state, args)?;
    let terms = a.str("pattern").map(logq::split_terms).unwrap_or_default();
    let min_level = Level::parse(a.str("level").unwrap_or(""))?;
    if terms.is_empty() && min_level.is_none() {
        return Err("需要 pattern（关键词）或 level（warn / error）至少一个".into());
    }
    let timeout = Duration::from_secs(a.clamp("timeout_secs", 30, 1, 120));
    let buf = state.log_buffer(&p.id);
    let mut since = a.num("since_seq").unwrap_or_else(|| buf.with_lines(|q| q.back().map(|l| l.seq).unwrap_or(0)));
    let started = Instant::now();

    loop {
        let fresh = buf.with_lines(|q| q.iter().filter(|l| l.seq > since).cloned().collect::<Vec<_>>());
        if let Some(l) = fresh.last() {
            since = l.seq;
        }
        let query = Query { tail: 20, min_level, grep: terms.clone(), max_chars: 20000, ..Default::default() };
        let r = logq::run(&fresh, &query);
        if r.matched > 0 {
            return Ok(ToolOut::new(
                format!("等到了（{} 毫秒）：\n{}# 下次增量拉取：since_seq={since}\n", started.elapsed().as_millis(), logq::render(&r.lines)),
                json!({ "matched": true, "waited_ms": started.elapsed().as_millis() as u64, "next_since": since }),
            ));
        }
        let running = state.procs.lock().unwrap().contains_key(&p.id);
        if !running {
            // 进程已经不在了，再等也不会有新日志（已退出的原因会在日志最后几行）
            let tail = logq::run(&buf.snapshot(), &Query { tail: 10, max_chars: 4000, ..Default::default() });
            return Ok(ToolOut::new(
                format!("项目「{}」没有在运行，没等到想要的日志。最后几行：\n{}", p.name, logq::render(&tail.lines)),
                json!({ "matched": false, "exited": true, "next_since": since }),
            ));
        }
        if started.elapsed() >= timeout {
            return Ok(ToolOut::new(
                format!("等了 {} 秒，项目仍在运行，但没出现想要的日志。可以加大 timeout_secs，或用 devbox_project_logs 看看现在输出了什么。", timeout.as_secs()),
                json!({ "matched": false, "timed_out": true, "next_since": since }),
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

pub fn recent_exits(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let limit = A(args).clamp("limit", 10, 1, 50) as usize;
    // 先让巡检回收刚退出的进程，保证最新的崩溃已经入记录
    process::compute_statuses(ctx.state);
    let exits: Vec<_> = ctx.state.exit_history.lock().unwrap().iter().rev().take(limit).cloned().collect();
    if exits.is_empty() {
        return Ok(ToolOut::new("DevBox 运行期间还没有项目崩溃", json!({ "exits": [] })));
    }
    let text = exits
        .iter()
        .map(|e| format!("[{}] {}\n{}", e.ts, e.name, e.message))
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok(ToolOut::new(text, json!({ "exits": exits.iter().map(to_value).collect::<Vec<_>>() })))
}

pub fn list_groups(ctx: &Ctx, _args: &Value) -> Result<ToolOut, String> {
    let (projects, groups) = {
        let cfg = ctx.state.config.lock().unwrap();
        (cfg.projects.clone(), cfg.project_groups.clone())
    };
    let st = statuses(ctx.state);
    let mut rows = Vec::new();
    let mut text = String::new();
    let mut names: Vec<Option<String>> = groups.into_iter().map(Some).collect();
    if projects.iter().any(|p| p.group.is_none()) {
        names.push(None);
    }
    for g in names {
        let members: Vec<_> = projects.iter().filter(|p| p.group == g).collect();
        let running = members.iter().filter(|p| st.get(&p.id).is_some_and(|s| s.running)).count();
        let label = g.clone().unwrap_or_else(|| "未分组".into());
        text.push_str(&format!("{label}：{} 个项目，{running} 个运行中（{}）\n", members.len(), members.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join("、")));
        rows.push(json!({ "group": label, "projects": members.iter().map(|p| p.name.clone()).collect::<Vec<_>>(), "running": running }));
    }
    if rows.is_empty() {
        text = "还没有项目组".into();
    }
    Ok(ToolOut::new(text, json!({ "groups": rows })))
}

pub fn list_scripts(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let p = find_project(ctx.state, args)?;
    let list = scripts::list_scripts_inner(&p.id, ctx.state)?;
    let text = if list.is_empty() {
        format!("「{}」目录里没有识别出可运行的脚本", p.name)
    } else {
        list.iter()
            .map(|e| format!("- {}: {}  ({}{})", e.label, e.command, e.source, e.hint.as_ref().map_or(String::new(), |h| format!("；{h}"))))
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(ToolOut::new(text, json!({ "project": p.name, "scripts": to_value(&list) })))
}

pub fn project_envs(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let state = ctx.state;
    let a = A(args);
    let one = if a.str("project").is_some() || a.str("id").is_some() || a.str("name").is_some() { Some(find_project(state, args)?) } else { None };
    let all = envs::project_envs_inner(state);
    let projects = state.config.lock().unwrap().projects.clone();
    let mut text = String::new();
    let mut rows = Vec::new();
    for e in all.iter().filter(|e| one.as_ref().map_or(true, |p| p.id == e.id)) {
        let name = projects.iter().find(|p| p.id == e.id).map(|p| p.name.clone()).unwrap_or_default();
        if e.scan.profiles.is_empty() && one.is_none() {
            continue; // 全量列表里略过没有可选环境的项目
        }
        text.push_str(&format!(
            "{name}：当前 {}，可选 {}{}\n",
            e.active.as_deref().unwrap_or("默认"),
            if e.scan.profiles.is_empty() { "无".to_string() } else { e.scan.profiles.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(" / ") },
            e.scan.default.as_ref().map_or(String::new(), |d| format!("（项目文件默认：{d}）")),
        ));
        for p in &e.scan.profiles {
            text.push_str(&format!("    {} — {}（{}）\n", p.name, p.summary, p.source));
        }
        for n in &e.scan.notes {
            text.push_str(&format!("    ⚠ {n}\n"));
        }
        rows.push(json!({
            "project": name, "active": e.active, "default": e.scan.default, "notes": e.scan.notes,
            "profiles": e.scan.profiles.iter().map(|x| json!({ "name": x.name, "source": x.source, "summary": x.summary, "vars": x.vars.keys().collect::<Vec<_>>() })).collect::<Vec<_>>(),
        }));
    }
    if text.is_empty() {
        text = "没有识别到可切换的环境".into();
    }
    Ok(ToolOut::new(text, json!({ "projects": rows })))
}

pub fn git_status(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let state = ctx.state;
    let a = A(args);
    let targets: Vec<_> = if a.str("project").is_some() || a.str("id").is_some() || a.str("name").is_some() {
        let p = find_project(state, args)?;
        git::refresh_status_of(state, &p.id); // 单个项目现算，保证是最新的
        vec![p]
    } else {
        state.config.lock().unwrap().projects.clone()
    };
    let cache = state.git_status.lock().unwrap().clone();
    let mut text = String::new();
    let mut rows = Vec::new();
    for p in &targets {
        let Some(b) = git::current_branch(&p.path) else { continue };
        let g = cache.get(&p.id);
        let mut line = format!("{}：{}", p.name, b.name);
        if let Some(g) = g {
            if g.has_upstream {
                line.push_str(&format!("，领先 {} 落后 {}", g.ahead, g.behind));
            } else {
                line.push_str("，未设置上游分支");
            }
            if g.dirty > 0 {
                line.push_str(&format!("，{} 个文件有未提交修改", g.dirty));
            }
        }
        text.push_str(&line);
        text.push('\n');
        rows.push(json!({
            "project": p.name, "branch": b.name, "detached": b.detached,
            "ahead": g.map(|g| g.ahead), "behind": g.map(|g| g.behind), "dirty_files": g.map(|g| g.dirty), "has_upstream": g.map(|g| g.has_upstream),
        }));
    }
    if rows.is_empty() {
        return Ok(ToolOut::new("这些项目都不在 git 仓库里", json!({ "projects": [] })));
    }
    Ok(ToolOut::new(text, json!({ "projects": rows })))
}

pub fn git_branches(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let p = find_project(ctx.state, args)?;
    let list = git::git_branches_inner(&p.id, ctx.state)?;
    let mut text = format!("{}：当前 {}，{} 个文件有未提交修改\n", p.name, list.current.as_ref().map_or("（无）".to_string(), |c| c.name.clone()), list.dirty);
    for b in &list.branches {
        text.push_str(&format!("{} {}{} {} — {} ({})\n", if b.current { "*" } else { " " }, b.name, if b.kind == "remote" { " [远程]" } else { "" }, b.hash, b.subject, b.date));
    }
    Ok(ToolOut::new(text, json!({ "project": p.name, "branches": to_value(&list) })))
}

pub fn list_runtimes(_ctx: &Ctx, _args: &Value) -> Result<ToolOut, String> {
    let rt = runtime::list_runtimes();
    let fmt = |title: &str, list: &[runtime::RuntimeVersion]| -> String {
        if list.is_empty() {
            return format!("{title}：未检测到\n");
        }
        let mut s = format!("{title}：\n");
        for v in list {
            s.push_str(&format!("  {}  ({})  {}\n", v.version, v.source, v.path));
        }
        s
    };
    let text = format!("{}{}", fmt("Node", &rt.node), fmt("JDK", &rt.java));
    Ok(ToolOut::new(text, to_value(&rt)))
}

pub fn list_ports(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let a = A(args);
    let all = a.str("scope") == Some("all");
    let mut list = if all { ports::list_ports()? } else { ports::list_project_ports_inner(ctx.state)? };
    if let Some(q) = a.str("query") {
        let q = q.to_lowercase();
        list.retain(|p| p.port.to_string() == q || p.process.to_lowercase().contains(&q) || p.project.as_ref().is_some_and(|n| n.to_lowercase().contains(&q)));
    }
    list.sort_by_key(|p| p.port);
    let text = if list.is_empty() {
        "没有符合条件的监听端口".to_string()
    } else {
        list.iter()
            .map(|p| format!(":{} {} (pid {}){}", p.port, p.process, p.pid, p.project.as_ref().map_or(String::new(), |n| format!(" ← 项目「{n}」"))))
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(ToolOut::new(text, json!({ "count": list.len(), "ports": to_value(&list) })))
}

pub fn check_port(ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let a = A(args);
    let (port, holders) = if let Some(port) = a.num("port") {
        let port = u16::try_from(port).map_err(|_| "端口号必须在 1-65535 之间".to_string())?;
        (Some(port), ports::who_holds(ctx.state, port))
    } else if a.str("project").is_some() || a.str("id").is_some() || a.str("name").is_some() {
        let p = find_project(ctx.state, args)?;
        let check = ports::check_project_port_inner(&p.id, ctx.state)?;
        if check.port.is_none() {
            return Ok(ToolOut::new(format!("项目「{}」没有登记端口", p.name), json!({ "port": null, "holders": [] })));
        }
        (check.port, check.holders)
    } else {
        return Err("需要 port 或 project 其中之一".into());
    };
    let port_n = port.unwrap_or(0);
    let text = if holders.is_empty() {
        format!("端口 {port_n} 空闲")
    } else {
        format!("端口 {port_n} 被占用：{}", ports::describe_holders(&holders))
    };
    Ok(ToolOut::new(text, json!({ "port": port, "free": holders.is_empty(), "holders": to_value(&holders) })))
}

pub fn get_hosts(_ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    if A(args).bool("raw", false) {
        let raw = hosts::get_hosts_raw()?;
        return Ok(ToolOut::new(raw.clone(), json!({ "raw": raw })));
    }
    let list = hosts::get_hosts()?;
    let text = if list.is_empty() {
        "DevBox 管理的 hosts 块里还没有记录".to_string()
    } else {
        list.iter()
            .map(|e| format!("{} {}\t{}{}", if e.enabled { "✓" } else { "✗" }, e.ip, e.hostnames.join(" "), e.comment.as_ref().map_or(String::new(), |c| format!("  # {c}"))))
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(ToolOut::new(text, json!({ "entries": to_value(&list) })))
}

pub fn list_services(_ctx: &Ctx, _args: &Value) -> Result<ToolOut, String> {
    let list = services::list_services()?;
    let text = if list.is_empty() {
        "没有 brew 服务".to_string()
    } else {
        list.iter().map(|s| format!("{} — {}", s.name, s.status)).collect::<Vec<_>>().join("\n")
    };
    Ok(ToolOut::new(text, json!({ "services": to_value(&list) })))
}

pub fn detect_project(_ctx: &Ctx, args: &Value) -> Result<ToolOut, String> {
    let path = A(args).req("path")?.to_string();
    let detected = projects::detect_project(path.clone())?;
    let candidates = startcmd::detect_start_commands(path.clone()).unwrap_or_default();
    let env_scan = envs::detect_environments(path, detected.start_command.clone()).ok();

    let mut text = format!(
        "识别结果：{}\n名称：{}\n类型：{}\n启动命令：{}\n端口：{}\n地址：{}",
        detected.summary,
        detected.name.as_deref().unwrap_or("（未识别）"),
        detected.kind.as_deref().unwrap_or("（未识别）"),
        detected.start_command.as_deref().unwrap_or("（未识别）"),
        detected.port.map_or("（未识别）".to_string(), |p| p.to_string()),
        detected.url.as_deref().unwrap_or("（未识别）"),
    );
    if candidates.len() > 1 {
        text.push_str("\n其它可用的启动命令：");
        for c in candidates.iter().skip(1).take(6) {
            text.push_str(&format!("\n  - {}（{}）", c.command, c.label));
        }
    }
    if let Some(e) = &env_scan {
        if !e.profiles.is_empty() {
            text.push_str(&format!("\n可选环境：{}", e.profiles.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(" / ")));
        }
    }
    for n in &detected.notes {
        text.push_str(&format!("\n⚠ {n}"));
    }
    Ok(ToolOut::new(
        text,
        json!({
            "detected": to_value(&detected),
            "start_commands": to_value(&candidates),
            "environments": env_scan.as_ref().map(to_value),
        }),
    ))
}
