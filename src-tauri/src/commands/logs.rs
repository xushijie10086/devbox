use crate::logstore::LogChunk;
use crate::state::AppState;
use tauri::State;

/// 增量获取某项目的日志：after 是前端已见过的最大序号，epoch 是它记的纪元。
/// 纪元一致时只返回更新的行；刚「清空」过（纪元变了）则返回全部，前端据此整体重置。
#[tauri::command]
pub fn get_logs(project_id: String, after: Option<u64>, epoch: Option<u64>, state: State<AppState>) -> LogChunk {
    state.log_buffer(&project_id).chunk(after, epoch)
}

/// 清空某项目的日志（内存和落盘文件一起清）
#[tauri::command]
pub fn clear_logs(project_id: String, state: State<AppState>) {
    state.log_buffer(&project_id).clear();
}

/// 日志文件的路径，供「在访达显示」使用
#[tauri::command]
pub fn log_file_path(project_id: String, state: State<AppState>) -> Result<String, String> {
    let buf = state.log_buffer(&project_id);
    let p = buf.path().ok_or("没有日志文件")?;
    // 还没产生过日志时文件不存在：先确保目录和文件在，访达才能定位
    if !p.exists() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::File::create(&p);
    }
    Ok(p.to_string_lossy().into_owned())
}
