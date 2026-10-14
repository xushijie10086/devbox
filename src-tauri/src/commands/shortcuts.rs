//! 「打开……」类的快捷操作。各平台的具体做法在 `platform` 里，这里只负责参数整理。

/// 用编辑器打开项目目录（默认 VSCode: `code`）
#[tauri::command]
pub fn open_in_editor(path: String, editor: Option<String>) -> Result<(), String> {
    let editor = editor.filter(|e| !e.trim().is_empty()).unwrap_or_else(|| "code".into());
    crate::platform::open_in_editor(&editor, &path)
}

/// 在默认浏览器打开地址
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    crate::platform::open_url(&url)
}

/// 在终端中打开目录
#[tauri::command]
pub fn open_terminal(path: String) -> Result<(), String> {
    crate::platform::open_terminal(&path)
}

/// 在文件管理器（访达 / 资源管理器 / Files）中显示：目录就打开它；文件则尽量定位并选中该文件。
/// 命令名沿用 reveal_in_finder，前端无需改动。
#[tauri::command]
pub fn reveal_in_finder(path: String) -> Result<(), String> {
    crate::platform::reveal(&path)
}
