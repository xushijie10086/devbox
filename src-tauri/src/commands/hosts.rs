use crate::models::HostEntry;
use crate::platform::{hosts_path, write_hosts_elevated};

const BEGIN: &str = "# >>> devbox managed >>>";
const END: &str = "# <<< devbox managed <<<";

/// 读取 DevBox 管理块内的 hosts 记录
#[tauri::command]
pub fn get_hosts() -> Result<Vec<HostEntry>, String> {
    let content = std::fs::read_to_string(hosts_path()).map_err(|e| e.to_string())?;
    let block = extract_block(&content).unwrap_or_default();
    Ok(block.lines().filter_map(parse_line).collect())
}

/// 返回完整 hosts 原文（只读展示用）
#[tauri::command]
pub fn get_hosts_raw() -> Result<String, String> {
    std::fs::read_to_string(hosts_path()).map_err(|e| e.to_string())
}

/// 用给定记录重建 DevBox 管理块，块外内容原样保留。需要管理员授权。
#[tauri::command]
pub fn save_hosts(entries: Vec<HostEntry>) -> Result<(), String> {
    let current = std::fs::read_to_string(hosts_path()).map_err(|e| e.to_string())?;

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

    let new_content = match_line_endings(&current, replace_block(&current, &block));
    write_hosts_elevated(&new_content)
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

/// 文件原来用 CRLF（Windows 的 hosts 通常如此）就把整份内容统一成 CRLF，避免新旧换行混在一起
fn match_line_endings(original: &str, content: String) -> String {
    if original.contains("\r\n") {
        content.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        content
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: &str = "# >>> devbox managed >>>\n127.0.0.1\tapp.local\n# <<< devbox managed <<<";

    #[test]
    fn a_new_block_is_appended_and_an_existing_one_is_replaced_in_place() {
        let fresh = "127.0.0.1 localhost\n";
        let out = replace_block(fresh, BLOCK);
        assert!(out.starts_with("127.0.0.1 localhost\n\n# >>> devbox managed >>>"), "{out}");
        assert!(out.ends_with("# <<< devbox managed <<<\n"), "{out}");

        let old = format!("a\n{}\nz\n", "# >>> devbox managed >>>\n1.1.1.1\told\n# <<< devbox managed <<<");
        let out = replace_block(&old, BLOCK);
        assert_eq!(out, format!("a\n{BLOCK}\nz\n"), "块外内容原样保留，块内换成新的");
    }

    #[test]
    fn entries_round_trip_through_the_block() {
        let content = "# >>> devbox managed >>>\n127.0.0.1\tapp.local api.local  # 开发\n# 10.0.0.2\tstaging.local\n# <<< devbox managed <<<\n";
        let block = extract_block(content).unwrap();
        let entries: Vec<HostEntry> = block.lines().filter_map(parse_line).collect();
        assert_eq!(entries.len(), 2);
        assert_eq!((entries[0].ip.as_str(), entries[0].enabled), ("127.0.0.1", true));
        assert_eq!(entries[0].hostnames, ["app.local", "api.local"]);
        assert_eq!(entries[0].comment.as_deref(), Some("开发"));
        assert_eq!((entries[1].ip.as_str(), entries[1].enabled), ("10.0.0.2", false), "被注释掉的记录是「已停用」");
    }

    #[test]
    fn crlf_files_stay_crlf_and_lf_files_stay_lf() {
        // Windows 的 hosts 通常是 CRLF：新旧内容不能换行风格混在一起
        let crlf_original = "127.0.0.1 localhost\r\n";
        let merged = replace_block(crlf_original, BLOCK);
        let out = match_line_endings(crlf_original, merged);
        assert!(out.contains("\r\n") && !out.replace("\r\n", "").contains('\n'), "每个换行都必须是 CRLF: {out:?}");

        let lf_original = "127.0.0.1 localhost\n";
        let out = match_line_endings(lf_original, replace_block(lf_original, BLOCK));
        assert!(!out.contains('\r'), "{out:?}");
    }

    #[test]
    fn already_crlf_content_is_not_doubled() {
        let out = match_line_endings("a\r\n", "x\r\ny\nz".to_string());
        assert_eq!(out, "x\r\ny\r\nz");
    }
}
