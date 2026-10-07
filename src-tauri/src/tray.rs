//! 菜单栏托盘：列出所有项目，点一下启动 / 停止，不用打开窗口。
//!
//! 菜单内容由纯函数 `model` 根据状态算出，`refresh` 只在内容变化时才重建真正的菜单，
//! 所以巡检线程可以放心地每隔几秒调用。

use crate::commands::process::{running_ids, start_with_deps, stop_all, stop_project_inner};
use crate::models::StartOutcome;
use crate::notify;
use crate::state::AppState;
use std::sync::Mutex;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

/// 托盘里最多列出的项目数，再多菜单会长得没法用
const MAX_PROJECTS: usize = 40;

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Item,
    /// 带勾选状态的开关
    Check(bool),
    Separator,
    /// 分组标题：不可点
    Header,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub kind: Kind,
}

fn entry(id: &str, label: impl Into<String>, enabled: bool, kind: Kind) -> Entry {
    Entry { id: id.to_string(), label: label.into(), enabled, kind }
}

/// 根据当前状态算出托盘菜单的内容
pub fn model(state: &AppState) -> Vec<Entry> {
    let running = running_ids(state);
    let cfg = state.config.lock().unwrap();
    let mut out = vec![entry("show", "显示 DevBox", true, Kind::Item), entry("", "", false, Kind::Separator)];

    let item = |p: &crate::models::Project| {
        let on = running.contains(&p.id);
        entry(
            &format!("proj:{}", p.id),
            if on { format!("■ 停止 · {}", p.name) } else { format!("▶ 启动 · {}", p.name) },
            true,
            Kind::Item,
        )
    };

    if cfg.projects.is_empty() {
        out.push(entry("", "（还没有项目）", false, Kind::Header));
    } else {
        let mut shown = 0usize;
        let mut push_section = |title: Option<String>, list: Vec<&crate::models::Project>, out: &mut Vec<Entry>| {
            if list.is_empty() {
                return;
            }
            if let Some(t) = title {
                out.push(entry("", format!("— {t} —"), false, Kind::Header));
            }
            for p in list {
                if shown >= MAX_PROJECTS {
                    return;
                }
                out.push(item(p));
                shown += 1;
            }
        };
        if cfg.project_groups.is_empty() {
            push_section(None, cfg.projects.iter().collect(), &mut out);
        } else {
            for g in &cfg.project_groups {
                push_section(Some(g.clone()), cfg.projects.iter().filter(|p| p.group.as_deref() == Some(g)).collect(), &mut out);
            }
            push_section(Some("未分组".into()), cfg.projects.iter().filter(|p| p.group.is_none()).collect(), &mut out);
        }
        if cfg.projects.len() > shown {
            out.push(entry("", format!("…还有 {} 个项目，请打开窗口查看", cfg.projects.len() - shown), false, Kind::Header));
        }
    }

    out.push(entry("", "", false, Kind::Separator));
    out.push(entry("stop_all", format!("全部停止（{} 个运行中）", running.len()), !running.is_empty(), Kind::Item));
    out.push(entry("toggle_notify", "系统通知", true, Kind::Check(!cfg.mute_notifications)));
    out.push(entry("", "", false, Kind::Separator));
    out.push(entry("quit", "退出", true, Kind::Item));
    out
}

/// 托盘图标的悬停提示
pub fn tooltip(state: &AppState) -> String {
    match running_ids(state).len() {
        0 => "DevBox".to_string(),
        n => format!("DevBox · {n} 个项目运行中"),
    }
}

/// 点击某个项目：在运行就停止，没运行就启动。返回 (标题, 正文) 供通知使用
pub fn toggle_project(state: &AppState, id: &str) -> (String, String) {
    let name = state
        .config
        .lock()
        .unwrap()
        .projects
        .iter()
        .find(|p| p.id == id)
        .map(|p| p.name.clone())
        .unwrap_or_else(|| id.to_string());
    if running_ids(state).iter().any(|r| r == id) {
        return match stop_project_inner(id, state) {
            Ok(()) => (format!("「{name}」已停止"), String::new()),
            Err(e) => (format!("「{name}」停止失败"), e),
        };
    }
    let result: Result<StartOutcome, String> = start_with_deps(id, state);
    notify::start_text(&name, &result)
}

// ---------- 与系统托盘对接的薄层 ----------

static LAST_SIGNATURE: Mutex<String> = Mutex::new(String::new());

fn signature(entries: &[Entry], tip: &str) -> String {
    let mut s = tip.to_string();
    for e in entries {
        s.push_str(&format!("\n{}|{}|{}|{:?}", e.id, e.label, e.enabled, e.kind));
    }
    s
}

fn build_menu(app: &AppHandle, entries: &[Entry]) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    for e in entries {
        match &e.kind {
            Kind::Separator => menu.append(&PredefinedMenuItem::separator(app)?)?,
            Kind::Check(on) => menu.append(&CheckMenuItem::with_id(app, &e.id, &e.label, e.enabled, *on, None::<&str>)?)?,
            Kind::Item | Kind::Header => menu.append(&MenuItem::with_id(app, &e.id, &e.label, e.enabled, None::<&str>)?)?,
        }
    }
    Ok(menu)
}

/// 构建托盘图标（应用启动时调用一次）
pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let state = app.state::<AppState>();
    let entries = model(&state);
    let tip = tooltip(&state);
    let menu = build_menu(app, &entries)?;
    *LAST_SIGNATURE.lock().unwrap() = signature(&entries, &tip);

    let mut builder = TrayIconBuilder::with_id("main")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .tooltip(&tip)
        .on_menu_event(|app, event| on_menu_event(app, event.id().as_ref()));
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    Ok(())
}

/// 菜单内容有变化时才重建（项目启停、增删、改名、开关通知都会触发）
pub fn refresh(app: &AppHandle) {
    let state = app.state::<AppState>();
    let entries = model(&state);
    let tip = tooltip(&state);
    let sig = signature(&entries, &tip);
    {
        let mut last = LAST_SIGNATURE.lock().unwrap();
        if *last == sig {
            return;
        }
        *last = sig;
    }
    let handle = app.clone();
    // 菜单操作必须在主线程
    let _ = app.run_on_main_thread(move || {
        if let Some(tray) = handle.tray_by_id("main") {
            if let Ok(menu) = build_menu(&handle, &entries) {
                let _ = tray.set_menu(Some(menu));
            }
            let _ = tray.set_tooltip(Some(&tip));
        }
    });
}

fn on_menu_event(app: &AppHandle, id: &str) {
    match id {
        "show" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }
        "quit" => app.exit(0),
        "toggle_notify" => {
            let state = app.state::<AppState>();
            let now_muted = {
                let mut cfg = state.config.lock().unwrap();
                cfg.mute_notifications = !cfg.mute_notifications;
                cfg.mute_notifications
            };
            let _ = state.persist();
            let _ = now_muted;
            refresh(app);
        }
        "stop_all" => {
            let app = app.clone();
            std::thread::spawn(move || {
                let state = app.state::<AppState>();
                let n = stop_all(&state);
                notify::notify(&state, &format!("已停止 {n} 个项目"), "", true);
                refresh(&app);
            });
        }
        other => {
            if let Some(pid) = other.strip_prefix("proj:") {
                let (app, pid) = (app.clone(), pid.to_string());
                // 启动要等就绪，可能要几十秒：放到后台线程，别卡住托盘
                std::thread::spawn(move || {
                    let state = app.state::<AppState>();
                    let (title, body) = toggle_project(&state, &pid);
                    notify::notify(&state, &title, &body, true);
                    refresh(&app);
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Config;
    use std::time::Duration;

    fn cfg(groups: &[&str], projects: &[(&str, &str, Option<&str>)]) -> Config {
        Config {
            projects: projects
                .iter()
                .map(|(id, name, g)| {
                    serde_json::from_value(serde_json::json!({
                        "id": id, "name": name, "path": "/tmp", "start_command": "sleep 30", "group": g
                    }))
                    .unwrap()
                })
                .collect(),
            project_groups: groups.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn labels(m: &[Entry]) -> Vec<String> {
        m.iter().map(|e| if e.kind == Kind::Separator { "──".into() } else { e.label.clone() }).collect()
    }

    #[test]
    fn flat_list_without_groups_and_running_state() {
        let st = AppState::for_test(cfg(&[], &[("a", "官网", None), ("b", "订单服务", None)]));
        let m = model(&st);
        assert_eq!(
            labels(&m),
            vec!["显示 DevBox", "──", "▶ 启动 · 官网", "▶ 启动 · 订单服务", "──", "全部停止（0 个运行中）", "系统通知", "──", "退出"]
        );
        let stop_entry = m.iter().find(|e| e.id == "stop_all").unwrap();
        assert!(!stop_entry.enabled, "没有运行中的项目时「全部停止」置灰");
        assert_eq!(m.iter().find(|e| e.id == "proj:a").unwrap().id, "proj:a");

        // 启动一个：标签变成「停止」，全部停止可用，提示里带数量
        start_and_verify_quick(&st, "a");
        let m = model(&st);
        assert!(labels(&m).contains(&"■ 停止 · 官网".to_string()));
        assert!(m.iter().find(|e| e.id == "stop_all").unwrap().enabled);
        assert_eq!(tooltip(&st), "DevBox · 1 个项目运行中");
        stop_all(&st);
        assert_eq!(tooltip(&st), "DevBox");
    }

    fn start_and_verify_quick(st: &AppState, id: &str) {
        crate::commands::process::start_and_verify_with(id, st, Duration::from_millis(200), Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn grouped_menu_has_headers_and_ungrouped_section() {
        let st = AppState::for_test(cfg(
            &["前端", "后端", "空组"],
            &[("a", "官网", Some("前端")), ("b", "订单", Some("后端")), ("c", "脚本", None)],
        ));
        let l = labels(&model(&st));
        let pos = |s: &str| l.iter().position(|x| x == s).unwrap_or_else(|| panic!("{s} 不在 {l:?}"));
        assert!(pos("— 前端 —") < pos("▶ 启动 · 官网") && pos("▶ 启动 · 官网") < pos("— 后端 —"));
        assert!(pos("— 后端 —") < pos("▶ 启动 · 订单") && pos("▶ 启动 · 订单") < pos("— 未分组 —"));
        assert!(pos("— 未分组 —") < pos("▶ 启动 · 脚本"));
        assert!(!l.contains(&"— 空组 —".to_string()), "空分组不显示标题");
        let headers: Vec<_> = model(&st).into_iter().filter(|e| e.kind == Kind::Header).collect();
        assert!(headers.iter().all(|h| !h.enabled), "标题不可点");
    }

    #[test]
    fn empty_and_overlong_lists() {
        let st = AppState::for_test(Config::default());
        assert!(labels(&model(&st)).contains(&"（还没有项目）".to_string()));

        let many: Vec<(String, String)> = (0..45).map(|i| (format!("p{i}"), format!("项目{i}"))).collect();
        let projects: Vec<(&str, &str, Option<&str>)> = many.iter().map(|(a, b)| (a.as_str(), b.as_str(), None)).collect();
        let m = model(&AppState::for_test(cfg(&[], &projects)));
        assert_eq!(m.iter().filter(|e| e.id.starts_with("proj:")).count(), MAX_PROJECTS);
        assert!(m.iter().any(|e| e.label.contains("还有 5 个项目")));
    }

    #[test]
    fn notify_switch_reflects_config() {
        let st = AppState::for_test(Config::default());
        let check = |st: &AppState| model(st).into_iter().find(|e| e.id == "toggle_notify").unwrap().kind;
        assert_eq!(check(&st), Kind::Check(true), "默认开启");
        st.config.lock().unwrap().mute_notifications = true;
        assert_eq!(check(&st), Kind::Check(false));
    }

    #[test]
    fn signature_changes_only_when_content_changes() {
        let st = AppState::for_test(cfg(&[], &[("a", "官网", None)]));
        let s1 = signature(&model(&st), &tooltip(&st));
        assert_eq!(s1, signature(&model(&st), &tooltip(&st)), "内容没变，签名不变（不会重复重建菜单）");
        start_and_verify_quick(&st, "a");
        let s2 = signature(&model(&st), &tooltip(&st));
        assert_ne!(s1, s2);
        stop_all(&st);
        st.config.lock().unwrap().projects[0].name = "改名了".into();
        assert_ne!(s2, signature(&model(&st), &tooltip(&st)));
    }

    #[test]
    fn clicking_a_project_toggles_it_and_reports() {
        let st = AppState::for_test(cfg(&[], &[("a", "官网", None)]));
        // 第一次点：启动（没配端口，存活 2 秒视为已启动）
        let (title, body) = toggle_project(&st, "a");
        assert_eq!(title, "「官网」已启动", "{body}");
        assert_eq!(running_ids(&st), vec!["a"]);
        // 第二次点：停止
        let (title, _) = toggle_project(&st, "a");
        assert_eq!(title, "「官网」已停止");
        assert!(running_ids(&st).is_empty());
    }

    #[test]
    fn clicking_a_broken_project_reports_the_reason() {
        let mut c = cfg(&[], &[("a", "坏项目", None)]);
        c.projects[0].start_command = "echo 'Error: db down' >&2; exit 3".into();
        let st = AppState::for_test(c);
        let (title, body) = toggle_project(&st, "a");
        assert_eq!(title, "「坏项目」启动失败");
        assert!(body.contains("退出码 3") && body.contains("db down"), "{body}");
    }
}
