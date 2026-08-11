use crate::models::LogLine;
use crate::state::AppState;
use tauri::State;

/// 获取某项目的日志缓冲
#[tauri::command]
pub fn get_logs(project_id: String, state: State<AppState>) -> Vec<LogLine> {
    let logs = state.logs.lock().unwrap();
    match logs.get(&project_id) {
        Some(buf) => buf.lock().unwrap().iter().cloned().collect(),
        None => Vec::new(),
    }
}

/// 清空某项目的日志
#[tauri::command]
pub fn clear_logs(project_id: String, state: State<AppState>) {
    let logs = state.logs.lock().unwrap();
    if let Some(buf) = logs.get(&project_id) {
        buf.lock().unwrap().clear();
    }
}
