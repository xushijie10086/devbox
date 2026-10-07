//! 项目日志仓：内存环形缓冲 + 落盘。
//!
//! - 每行日志有单调递增的序号 `seq`，前端只拉取比上次更新的行，不必每次整体重传、重绘
//! - 同时追加写入 `<配置目录>/logs/<项目>.log`（每行 `日期 时间<TAB>流<TAB>内容`），
//!   超过 MAX_FILE_BYTES 轮转成 `.log.1`；应用重启后载入最近的历史
//! - 「清空」同时清掉内存和文件，并让 epoch 加一，前端据此知道要整体重置

use crate::models::LogLine;
use chrono::Local;
use serde::Serialize;
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// 内存里每个项目最多保留的行数
pub const MAX_LOG_LINES: usize = 5000;
/// 单个日志文件的大小上限，超过就轮转
pub const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// 重启后从文件载入的历史行数上限
const HISTORY_LINES: usize = 300;
/// 载入历史时最多读文件末尾这么多字节
const HISTORY_BYTES: u64 = 256 * 1024;

pub type LogBuffer = Arc<LogStore>;

/// 一次拉取的结果
#[derive(Serialize, Clone, Debug)]
pub struct LogChunk {
    /// 当前纪元；「清空」会让它加一。前端发现与自己记的不一致，就丢掉旧内容重新开始
    pub epoch: u64,
    pub lines: Vec<LogLine>,
}

struct FileSink {
    path: Option<PathBuf>,
    file: Option<File>,
    size: u64,
}

pub struct LogStore {
    lines: Mutex<VecDeque<LogLine>>,
    next_seq: AtomicU64,
    epoch: AtomicU64,
    sink: Mutex<FileSink>,
}

impl LogStore {
    /// path 为 None 时只保存在内存里（测试用）
    pub fn new(path: Option<PathBuf>) -> Self {
        let store = LogStore {
            lines: Mutex::new(VecDeque::with_capacity(256)),
            next_seq: AtomicU64::new(1),
            epoch: AtomicU64::new(0),
            sink: Mutex::new(FileSink { path: path.clone(), file: None, size: 0 }),
        };
        if let Some(p) = &path {
            store.load_history(p);
        }
        store
    }

    fn make_line(&self, ts: String, stream: &str, text: String) -> LogLine {
        LogLine { seq: self.next_seq.fetch_add(1, Ordering::SeqCst), ts, stream: stream.to_string(), text }
    }

    /// 从上次运行留下的文件里载入最近一批历史，末尾加一条分隔线。这些行不会再写回文件。
    fn load_history(&self, path: &PathBuf) {
        let Ok(mut f) = File::open(path) else { return };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        self.sink.lock().unwrap().size = len;
        let start = len.saturating_sub(HISTORY_BYTES);
        if f.seek(SeekFrom::Start(start)).is_err() {
            return;
        }
        let mut buf = Vec::new();
        if f.read_to_end(&mut buf).is_err() {
            return;
        }
        let text = String::from_utf8_lossy(&buf);
        let mut it = text.lines();
        if start > 0 {
            it.next(); // 从文件中间开始读，第一行可能是半行
        }
        let today = Local::now().format("%Y-%m-%d").to_string();
        let parsed: Vec<(String, String, String)> = it
            .filter_map(|l| {
                let mut p = l.splitn(3, '\t');
                let (when, stream, text) = (p.next()?, p.next()?, p.next()?);
                matches!(stream, "stdout" | "stderr" | "system").then(|| (when.to_string(), stream.to_string(), text.to_string()))
            })
            .collect();
        let skip = parsed.len().saturating_sub(HISTORY_LINES);
        let mut q = self.lines.lock().unwrap();
        for (when, stream, text) in parsed.into_iter().skip(skip) {
            // 今天的只显示时间，更早的带上日期
            let ts = match when.split_once(' ') {
                Some((d, t)) if d == today => t.to_string(),
                Some((d, t)) => format!("{} {t}", d.get(5..).unwrap_or(d)),
                None => when,
            };
            let line = self.make_line(ts, &stream, text);
            q.push_back(line);
        }
        if !q.is_empty() {
            let sep = self.make_line(
                Local::now().format("%H:%M:%S").to_string(),
                "system",
                "── 以上是应用重启前的历史日志 ──".to_string(),
            );
            q.push_back(sep);
        }
    }

    /// 追加一条日志。含换行的文本按行拆开，每行一条
    pub fn push(&self, stream: &str, text: String) {
        if text.contains('\n') {
            for l in text.split('\n') {
                self.push(stream, l.trim_end_matches('\r').to_string());
            }
            return;
        }
        let now = Local::now();
        let line = self.make_line(now.format("%H:%M:%S").to_string(), stream, text);
        self.write_to_file(&now.format("%Y-%m-%d %H:%M:%S").to_string(), &line);
        let mut q = self.lines.lock().unwrap();
        if q.len() >= MAX_LOG_LINES {
            q.pop_front();
        }
        q.push_back(line);
    }

    fn write_to_file(&self, when: &str, line: &LogLine) {
        let mut sink = self.sink.lock().unwrap();
        let Some(path) = sink.path.clone() else { return };
        if sink.file.is_none() {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            sink.file = OpenOptions::new().create(true).append(true).open(&path).ok();
        }
        // 超过上限：当前文件改名为 .log.1（覆盖旧的），重新开始写
        if sink.size >= MAX_FILE_BYTES {
            sink.file = None;
            let _ = std::fs::rename(&path, rotated_path(&path));
            sink.file = OpenOptions::new().create(true).append(true).open(&path).ok();
            sink.size = 0;
        }
        let record = format!("{when}\t{}\t{}\n", line.stream, line.text);
        if let Some(f) = sink.file.as_mut() {
            if f.write_all(record.as_bytes()).is_ok() {
                sink.size += record.len() as u64;
            }
        }
    }

    /// 全部日志（拷贝）
    pub fn snapshot(&self) -> Vec<LogLine> {
        self.lines.lock().unwrap().iter().cloned().collect()
    }

    /// 在持有锁的情况下读取日志（避免整体拷贝）
    pub fn with_lines<R>(&self, f: impl FnOnce(&VecDeque<LogLine>) -> R) -> R {
        f(&self.lines.lock().unwrap())
    }

    /// 增量拉取：after 为客户端已见过的最大序号。纪元不一致（刚清空过）或没带 after 时返回全部
    pub fn chunk(&self, after: Option<u64>, epoch: Option<u64>) -> LogChunk {
        let cur = self.epoch.load(Ordering::SeqCst);
        let q = self.lines.lock().unwrap();
        let lines = match after {
            Some(a) if epoch == Some(cur) => q.iter().filter(|l| l.seq > a).cloned().collect(),
            _ => q.iter().cloned().collect(),
        };
        LogChunk { epoch: cur, lines }
    }

    /// 清空内存和文件
    pub fn clear(&self) {
        self.lines.lock().unwrap().clear();
        self.epoch.fetch_add(1, Ordering::SeqCst);
        let mut sink = self.sink.lock().unwrap();
        sink.file = None;
        sink.size = 0;
        if let Some(p) = &sink.path {
            let _ = std::fs::remove_file(p);
            let _ = std::fs::remove_file(rotated_path(p));
        }
    }

    /// 日志文件路径（用于「在访达显示」）
    pub fn path(&self) -> Option<PathBuf> {
        self.sink.lock().unwrap().path.clone()
    }
}

fn rotated_path(path: &PathBuf) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".1");
    PathBuf::from(s)
}

/// 项目 id 转成安全的文件名
pub fn file_name_for(id: &str) -> String {
    let safe: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    format!("{safe}.log")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("devbox-logstore-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn texts(s: &LogStore) -> Vec<String> {
        s.snapshot().into_iter().map(|l| l.text).collect()
    }

    #[test]
    fn sequence_numbers_increase_and_chunks_are_incremental() {
        let s = LogStore::new(None);
        s.push("stdout", "a".into());
        s.push("stderr", "b".into());
        let first = s.chunk(None, None);
        assert_eq!(first.lines.iter().map(|l| l.seq).collect::<Vec<_>>(), vec![1, 2]);
        s.push("stdout", "c".into());
        let next = s.chunk(Some(2), Some(first.epoch));
        assert_eq!(next.lines.len(), 1);
        assert_eq!(next.lines[0].text, "c");
        assert!(s.chunk(Some(3), Some(first.epoch)).lines.is_empty(), "没有新行");
    }

    #[test]
    fn multiline_text_is_split_into_lines() {
        let s = LogStore::new(None);
        s.push("system", "第一行\r\n第二行\n第三行".into());
        assert_eq!(texts(&s), vec!["第一行", "第二行", "第三行"]);
    }

    #[test]
    fn ring_buffer_keeps_only_the_latest_lines_but_seq_keeps_growing() {
        let s = LogStore::new(None);
        for i in 0..(MAX_LOG_LINES + 10) {
            s.push("stdout", format!("l{i}"));
        }
        let all = s.snapshot();
        assert_eq!(all.len(), MAX_LOG_LINES);
        assert_eq!(all[0].text, "l10");
        assert_eq!(all.last().unwrap().seq, (MAX_LOG_LINES + 10) as u64);
        // 客户端落后太多（想要的行已被挤掉）：给它现有的、比它新的行
        assert_eq!(s.chunk(Some(3), Some(0)).lines.len(), MAX_LOG_LINES);
    }

    #[test]
    fn clear_bumps_epoch_and_forces_a_full_reset() {
        let s = LogStore::new(None);
        s.push("stdout", "旧".into());
        let before = s.chunk(None, None);
        s.clear();
        s.push("stdout", "新".into());
        // 客户端还拿着旧纪元：必须拿到全部（而不是按序号算增量），才能整体重置
        let c = s.chunk(Some(1), Some(before.epoch));
        assert_ne!(c.epoch, before.epoch);
        assert_eq!(c.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), vec!["新"]);
        assert!(c.lines[0].seq > 1, "序号不回退");
    }

    #[test]
    fn lines_are_persisted_with_tabs_and_unicode_intact() {
        let d = tmp("persist");
        let p = d.join("p.log");
        let s = LogStore::new(Some(p.clone()));
        s.push("stdout", "含\t制表符 和 中文 ✔".into());
        s.push("stderr", "[ERROR] 失败".into());
        let text = std::fs::read_to_string(&p).unwrap();
        let rows: Vec<Vec<&str>> = text.lines().map(|l| l.splitn(3, '\t').collect()).collect();
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0][1], rows[0][2]), ("stdout", "含\t制表符 和 中文 ✔"));
        assert_eq!((rows[1][1], rows[1][2]), ("stderr", "[ERROR] 失败"));
        assert!(rows[0][0].len() == 19 && rows[0][0].contains(' '), "完整时间戳: {}", rows[0][0]);
    }

    #[test]
    fn history_is_loaded_after_restart_with_a_separator_and_not_rewritten() {
        let d = tmp("history");
        let p = d.join("p.log");
        {
            let s = LogStore::new(Some(p.clone()));
            s.push("stdout", "上次的输出 1".into());
            s.push("stderr", "上次的输出 2".into());
        }
        let size_before = std::fs::metadata(&p).unwrap().len();
        let s = LogStore::new(Some(p.clone())); // 模拟应用重启
        let t = texts(&s);
        assert_eq!(t, vec!["上次的输出 1", "上次的输出 2", "── 以上是应用重启前的历史日志 ──"]);
        assert_eq!(std::fs::metadata(&p).unwrap().len(), size_before, "载入历史不会再写回文件");
        // 新日志接在分隔线后面，序号继续递增
        s.push("stdout", "这次的输出".into());
        let all = s.snapshot();
        assert_eq!(all.last().unwrap().text, "这次的输出");
        assert!(all.windows(2).all(|w| w[0].seq < w[1].seq));
        // 历史行的时间戳是今天，只显示时分秒
        assert_eq!(all[0].ts.len(), 8, "{}", all[0].ts);
    }

    #[test]
    fn old_history_shows_its_date_and_garbage_lines_are_skipped() {
        let d = tmp("olddate");
        let p = d.join("p.log");
        std::fs::write(&p, "2020-01-02 03:04:05\tstdout\t很久以前\nnot a log line\n2020-01-02 03:04:06\tweird\t未知流\n").unwrap();
        let s = LogStore::new(Some(p));
        let all = s.snapshot();
        assert_eq!(all[0].ts, "01-02 03:04:05", "更早的日期要带上月日");
        assert_eq!(all[0].text, "很久以前");
        assert_eq!(all.len(), 2, "坏行和未知流被忽略，只剩 1 行历史 + 分隔线");
    }

    #[test]
    fn history_is_capped_to_the_most_recent_lines() {
        let d = tmp("cap");
        let p = d.join("p.log");
        {
            let s = LogStore::new(Some(p.clone()));
            for i in 0..(HISTORY_LINES + 50) {
                s.push("stdout", format!("行{i}"));
            }
        }
        let s = LogStore::new(Some(p));
        let t = texts(&s);
        assert_eq!(t.len(), HISTORY_LINES + 1);
        assert_eq!(t[0], "行50", "只保留最近的 {HISTORY_LINES} 行");
    }

    #[test]
    fn file_rotates_when_it_grows_past_the_limit() {
        let d = tmp("rotate");
        let p = d.join("p.log");
        let s = LogStore::new(Some(p.clone()));
        s.sink.lock().unwrap().size = MAX_FILE_BYTES; // 假装已经写满
        std::fs::write(&p, "旧内容\n").unwrap();
        s.push("stdout", "轮转之后的第一行".into());
        let rotated = rotated_path(&p);
        assert!(rotated.exists(), "旧文件应被改名为 .log.1");
        assert!(std::fs::read_to_string(&rotated).unwrap().contains("旧内容"));
        let fresh = std::fs::read_to_string(&p).unwrap();
        assert!(fresh.contains("轮转之后的第一行") && !fresh.contains("旧内容"), "{fresh}");
    }

    #[test]
    fn clear_removes_the_files_too() {
        let d = tmp("clearfile");
        let p = d.join("p.log");
        let s = LogStore::new(Some(p.clone()));
        s.push("stdout", "x".into());
        std::fs::write(rotated_path(&p), "old").unwrap();
        s.clear();
        assert!(!p.exists() && !rotated_path(&p).exists());
        s.push("stdout", "y".into());
        assert_eq!(std::fs::read_to_string(&p).unwrap().lines().count(), 1, "清空后重新开始写");
        // 清空后重启不会载入已清掉的历史
        assert_eq!(texts(&LogStore::new(Some(p))).first().map(|s| s.as_str()), Some("y"));
    }

    #[test]
    fn file_names_are_sanitized() {
        assert_eq!(file_name_for("3f2a-uuid_1"), "3f2a-uuid_1.log");
        assert_eq!(file_name_for("../../etc/passwd"), "______etc_passwd.log");
        assert_eq!(file_name_for("项目/一"), "____.log", "非 ASCII 字符和路径分隔符都被替换");
    }
}
