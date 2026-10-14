use crate::models::ServiceInfo;
use std::path::Path;
use std::process::Command;

/// 「服务」页依赖 Homebrew services，只有 macOS 支持；其它平台前端会隐藏这一页，这里是兜底
fn require_supported() -> Result<(), String> {
    if crate::platform::HAS_SERVICES {
        Ok(())
    } else {
        Err("当前系统不支持「服务」（它依赖 macOS 的 Homebrew services）".into())
    }
}

/// 定位 brew 可执行文件（兼容 Apple Silicon 与 Intel）
fn brew_bin() -> Option<String> {
    for cand in ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"] {
        if Path::new(cand).exists() {
            return Some(cand.to_string());
        }
    }
    // 兜底交给 PATH
    Some("brew".to_string())
}

/// 列出 brew 管理的服务
#[tauri::command]
pub fn list_services() -> Result<Vec<ServiceInfo>, String> {
    require_supported()?;
    let brew = brew_bin().ok_or("未找到 Homebrew，请先安装 brew")?;
    let output = Command::new(&brew)
        .args(["services", "list"])
        .output()
        .map_err(|e| format!("执行 brew 失败: {e}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut result = Vec::new();
    for line in text.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }
        result.push(ServiceInfo {
            name: parts[0].to_string(),
            status: parts.get(1).unwrap_or(&"unknown").to_string(),
            user: parts.get(2).map(|s| s.to_string()),
            file: parts.get(3).map(|s| s.to_string()),
        });
    }
    Ok(result)
}

#[tauri::command]
pub fn start_service(name: String) -> Result<(), String> {
    require_supported()?;
    run_brew_service("start", &name)
}

#[tauri::command]
pub fn stop_service(name: String) -> Result<(), String> {
    require_supported()?;
    run_brew_service("stop", &name)
}

#[tauri::command]
pub fn restart_service(name: String) -> Result<(), String> {
    require_supported()?;
    run_brew_service("restart", &name)
}

fn run_brew_service(action: &str, name: &str) -> Result<(), String> {
    let brew = brew_bin().ok_or("未找到 Homebrew")?;
    let output = Command::new(&brew)
        .args(["services", action, name])
        .output()
        .map_err(|e| format!("执行 brew 失败: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}
