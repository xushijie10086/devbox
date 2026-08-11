use crate::models::HostEntry;
use std::process::Command;

const HOSTS_PATH: &str = "/etc/hosts";
const BEGIN: &str = "# >>> devbox managed >>>";
const END: &str = "# <<< devbox managed <<<";

/// 读取 DevBox 管理块内的 hosts 记录
#[tauri::command]
pub fn get_hosts() -> Result<Vec<HostEntry>, String> {
    let content = std::fs::read_to_string(HOSTS_PATH).map_err(|e| e.to_string())?;
    let block = extract_block(&content).unwrap_or_default();
    Ok(block.lines().filter_map(parse_line).collect())
}

/// 返回完整 /etc/hosts 原文（只读展示用）
#[tauri::command]
pub fn get_hosts_raw() -> Result<String, String> {
    std::fs::read_to_string(HOSTS_PATH).map_err(|e| e.to_string())
}

/// 用给定记录重建 DevBox 管理块，块外内容原样保留。需要管理员授权。
#[tauri::command]
pub fn save_hosts(entries: Vec<HostEntry>) -> Result<(), String> {
    let current = std::fs::read_to_string(HOSTS_PATH).map_err(|e| e.to_string())?;

    let mut block = String::new();
    block.push_str(BEGIN);
    block.push('\n');
    for e in &entries {
        let prefix = if e.enabled { "" } else { "# " };
        let hosts = e.hostnames.join(" ");
        let comment = e
            .comment
            .as_ref()
            .filter(|c| !c.trim().is_empty())
            .map(|c| format!("  # {c}"))
            .unwrap_or_default();
        block.push_str(&format!("{prefix}{}\t{}{}\n", e.ip, hosts, comment));
    }
    block.push_str(END);

    let new_content = replace_block(&current, &block);
    write_hosts_as_admin(&new_content)
}

// ---------- 解析辅助 ----------

fn extract_block(content: &str) -> Option<String> {
    let start = content.find(BEGIN)?;
    let after = &content[start + BEGIN.len()..];
    let end_rel = after.find(END)?;
    Some(after[..end_rel].trim_matches('\n').to_string())
}

fn replace_block(content: &str, block: &str) -> String {
    if let (Some(s), Some(e)) = (content.find(BEGIN), content.find(END)) {
        let end = e + END.len();
        let mut out = String::new();
        out.push_str(&content[..s]);
        out.push_str(block);
        out.push_str(&content[end..]);
        out
    } else {
        let mut out = content.trim_end().to_string();
        out.push_str("\n\n");
        out.push_str(block);
        out.push('\n');
        out
    }
}

fn parse_line(line: &str) -> Option<HostEntry> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let enabled = !trimmed.starts_with('#');
    let body = trimmed.trim_start_matches('#').trim();
    if body.is_empty() {
        return None;
    }

    // 拆出行内注释
    let (data, comment) = match body.split_once('#') {
        Some((d, c)) => (d.trim(), Some(c.trim().to_string())),
        None => (body, None),
    };

    let mut tokens = data.split_whitespace();
    let ip = tokens.next()?.to_string();
    let hostnames: Vec<String> = tokens.map(|s| s.to_string()).collect();
    if hostnames.is_empty() {
        return None;
    }

    Some(HostEntry {
        ip,
        hostnames,
        enabled,
        comment,
    })
}

/// 借助 osascript 以管理员权限写入 /etc/hosts，并刷新 DNS 缓存
fn write_hosts_as_admin(content: &str) -> Result<(), String> {
    let tmp = std::env::temp_dir().join("devbox_hosts.tmp");
    std::fs::write(&tmp, content).map_err(|e| e.to_string())?;

    let shell = format!(
        "cp '{}' '{}' && dscacheutil -flushcache && killall -HUP mDNSResponder",
        tmp.display(),
        HOSTS_PATH
    );
    let script = format!("do shell script \"{}\" with administrator privileges", shell);

    let status = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .status()
        .map_err(|e| format!("调用 osascript 失败: {e}"))?;

    let _ = std::fs::remove_file(&tmp);

    if status.success() {
        Ok(())
    } else {
        Err("写入 /etc/hosts 被取消或失败（需要管理员授权）".into())
    }
}
