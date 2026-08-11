use std::process::Command;

/// 用编辑器打开项目目录（默认 VSCode: `code`）
#[tauri::command]
pub fn open_in_editor(path: String, editor: Option<String>) -> Result<(), String> {
    let editor = editor.filter(|e| !e.trim().is_empty()).unwrap_or_else(|| "code".into());
    // 走登录 shell，确保能取到 code / cursor / idea 等命令的 PATH
    let cmd = format!("{editor} '{path}'");
    Command::new("/bin/sh")
        .arg("-lc")
        .arg(&cmd)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打开编辑器失败: {e}"))
}

/// 在默认浏览器打开地址
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    Command::new("open")
        .arg(&url)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打开浏览器失败: {e}"))
}

/// 在终端(Terminal.app)中打开目录
#[tauri::command]
pub fn open_terminal(path: String) -> Result<(), String> {
    Command::new("open")
        .args(["-a", "Terminal"])
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打开终端失败: {e}"))
}

/// 在访达中显示目录
#[tauri::command]
pub fn reveal_in_finder(path: String) -> Result<(), String> {
    Command::new("open")
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打开访达失败: {e}"))
}
