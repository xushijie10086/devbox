//! 系统通知：项目崩溃、从托盘启停项目的结果。
//!
//! 窗口在前台且聚焦时不发——应用内的提示已经够用，重复弹两次只会烦人。

use crate::models::StartOutcome;
use crate::state::{AppState, APP_HANDLE};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_notification::NotificationExt;

/// 通知正文的最大字符数
const BODY_MAX: usize = 140;

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// 项目意外退出：标题 + 正文（退出原因，再带最后一行输出）
pub fn crash_text(name: &str, reason: &str, tail: &[String]) -> (String, String) {
    let mut body = reason.to_string();
    if let Some(last) = tail.last() {
        body.push('\n');
        body.push_str(last.trim());
    }
    (format!("「{name}」意外退出"), truncate(&body, BODY_MAX))
}

/// 启动 / 停止结果：标题 + 正文
pub fn start_text(name: &str, result: &Result<StartOutcome, String>) -> (String, String) {
    match result {
        Ok(o) if o.level == "warning" => (
            format!("「{name}」已启动，需留意"),
            truncate(o.message.lines().next().unwrap_or(""), BODY_MAX),
        ),
        Ok(o) => (
            format!("「{name}」已启动"),
            truncate(o.message.lines().next().unwrap_or(""), BODY_MAX),
        ),
        Err(e) => {
            // 失败信息第一行是原因，后面是输出摘要：通知里带上原因和最后一行
            let mut lines = e.lines().filter(|l| !l.trim().is_empty());
            let reason = lines.next().unwrap_or("启动失败");
            let last = e.lines().rev().find(|l| !l.trim().is_empty() && *l != reason);
            let body = match last {
                Some(l) => format!("{reason}\n{}", l.trim()),
                None => reason.to_string(),
            };
            (format!("「{name}」启动失败"), truncate(&body, BODY_MAX))
        }
    }
}

pub fn enabled(state: &AppState) -> bool {
    !state.config.lock().unwrap().mute_notifications
}

fn window_focused(app: &AppHandle) -> bool {
    app.get_webview_window("main")
        .map_or(false, |w| w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false))
}

/// 发系统通知。force 为 true 时不管窗口是否在前台（用于托盘里的操作反馈：
/// 用户正是因为不想打开窗口才用托盘的）。失败只记日志，绝不影响主流程。
pub fn notify(state: &AppState, title: &str, body: &str, force: bool) {
    if !enabled(state) {
        return;
    }
    let Some(app) = APP_HANDLE.get() else { return };
    if !force && window_focused(app) {
        return;
    }
    eprintln!("[devbox] 系统通知: {title} — {}", body.replace('\n', " / "));
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        eprintln!("[devbox] 发送系统通知失败: {e}");
    }
}

/// 前端读取：系统通知是否开启
#[tauri::command]
pub fn get_notifications_enabled(state: State<AppState>) -> bool {
    enabled(&state)
}

/// 前端 / 托盘切换系统通知开关，持久化到配置
#[tauri::command]
pub fn set_notifications_enabled(enabled: bool, state: State<AppState>) -> Result<(), String> {
    state.config.lock().unwrap().mute_notifications = !enabled;
    state.persist()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_text_has_reason_and_last_output_line() {
        let tail = vec!["[INFO] x".to_string(), "[ERROR] No plugin found for prefix 'spring-boot'".to_string()];
        let (t, b) = crash_text("站群项目", "进程异常退出（退出码 1）", &tail);
        assert_eq!(t, "「站群项目」意外退出");
        assert!(b.starts_with("进程异常退出（退出码 1）\n[ERROR] No plugin found"), "{b}");
        // 没有输出时只有原因
        assert_eq!(crash_text("a", "被信号 9 终止", &[]).1, "被信号 9 终止");
    }

    #[test]
    fn long_bodies_are_truncated() {
        let tail = vec!["x".repeat(500)];
        let (_, b) = crash_text("a", "r", &tail);
        assert!(b.chars().count() <= BODY_MAX + 1 && b.ends_with('…'), "{}", b.chars().count());
    }

    #[test]
    fn start_text_covers_success_warning_failure() {
        let ok = Ok(StartOutcome::success("已启动，端口 8080 已就绪（用时 3.2s）".into()));
        assert_eq!(start_text("官网", &ok), ("「官网」已启动".into(), "已启动，端口 8080 已就绪（用时 3.2s）".into()));

        let warn = Ok(StartOutcome::warning("进程在运行，但 30 秒内端口 8080 仍未监听\n请查看日志".into()));
        let (t, b) = start_text("官网", &warn);
        assert!(t.contains("需留意") && b.contains("端口 8080") && !b.contains("请查看日志"), "{t} / {b}");

        let err: Result<StartOutcome, String> =
            Err("启动失败：命令未找到（退出码 127）\n最后输出：\n/bin/sh: 1: foo: not found".into());
        let (t, b) = start_text("官网", &err);
        assert_eq!(t, "「官网」启动失败");
        assert!(b.contains("命令未找到") && b.contains("foo: not found"), "{b}");
    }

    #[test]
    fn notify_without_app_handle_is_a_silent_noop() {
        // 单元测试环境没有 AppHandle：不能 panic
        let st = AppState::for_test(Default::default());
        notify(&st, "t", "b", true);
        assert!(enabled(&st));
        st.config.lock().unwrap().mute_notifications = true;
        assert!(!enabled(&st));
    }
}
