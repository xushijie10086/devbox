//! 给 agent 看的日志查询：去 ANSI 颜色、识别级别、关键词过滤、上下文、按字数截断。
//!
//! agent 的上下文很贵：一股脑把 5000 行日志塞回去既浪费又容易淹没重点，
//! 所以这里提供「只看错误」「只看含某关键词的行（带前后几行）」「只取上次之后新增的行」这些筛法，
//! 并且总字数超限时保留最新的部分，同时告诉调用者被截掉了多少。

use crate::models::LogLine;
use serde::Serialize;

/// 单行最长字符数，超出的部分截断（比如压缩后的前端 bundle 报错会有几万字符一行）
const MAX_LINE_CHARS: usize = 2000;

/// 去掉 ANSI 转义序列；回车重绘（进度条）只保留最后一次的内容
pub fn strip_ansi(raw: &str) -> String {
    let raw = match raw.rfind('\r') {
        // 行尾的 \r 不算重绘（Windows 的 \r\n 已经被读线程去掉，这里只是保险）
        Some(i) if i + 1 < raw.len() => &raw[i + 1..],
        Some(i) => &raw[..i],
        None => raw,
    };
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            // CSI：ESC [ 参数 中间字节 终止字节(0x40-0x7E)
            Some('[') => {
                chars.next();
                for n in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&n) {
                        break;
                    }
                }
            }
            // OSC：ESC ] ... BEL 或 ESC \
            Some(']') => {
                chars.next();
                while let Some(n) = chars.next() {
                    if n == '\u{7}' {
                        break;
                    }
                    if n == '\u{1b}' {
                        chars.next();
                        break;
                    }
                }
            }
            // 其它两字节转义
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

/// 日志级别
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn parse(s: &str) -> Result<Option<Level>, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "all" | "any" => Ok(None),
            "warn" | "warning" => Ok(Some(Level::Warn)),
            "error" | "err" => Ok(Some(Level::Error)),
            other => Err(format!("level 只能是 all / warn / error，收到「{other}」")),
        }
    }
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '.'
}

/// 整词匹配（区分大小写）：前后不能紧挨着字母数字或下划线
fn has_word(text: &str, word: &str) -> bool {
    let mut from = 0;
    while let Some(i) = text[from..].find(word) {
        let start = from + i;
        let end = start + word.len();
        let before_ok = text[..start].chars().next_back().map_or(true, |c| !(c.is_ascii_alphanumeric() || c == '_'));
        let after_ok = text[end..].chars().next().map_or(true, |c| !(c.is_ascii_alphanumeric() || c == '_'));
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

/// 形如 java.lang.NullPointerException: / TypeError: 的异常名（后面跟冒号、空白或行尾）
fn has_exception_name(text: &str) -> bool {
    let mut word_start: Option<usize> = None;
    let check = |s: &str, end: usize, start: usize| -> bool {
        let w = &s[start..end];
        let after = s[end..].chars().next();
        (w.ends_with("Exception") || w.ends_with("Error")) && after.map_or(true, |c| c == ':' || c.is_whitespace())
    };
    for (i, c) in text.char_indices() {
        if is_ident(c) {
            word_start.get_or_insert(i);
        } else if let Some(s) = word_start.take() {
            if check(text, i, s) {
                return true;
            }
        }
    }
    word_start.is_some_and(|s| check(text, text.len(), s))
}

fn starts_with_ci(text: &str, prefix: &str) -> bool {
    text.len() >= prefix.len() && text.is_char_boundary(prefix.len()) && text[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// 判断一行日志的级别（输入已去掉 ANSI）。规则与界面里的日志查看器一致：不看 stderr 本身，
/// 因为很多工具把普通信息也写到 stderr
pub fn detect_level(text: &str) -> Level {
    let t = text.trim_start();
    let error = ["ERROR", "FATAL", "SEVERE", "CRITICAL"].iter().any(|w| has_word(text, w))
        || has_exception_name(text)
        || text.contains("Caused by:")
        || text.contains("Traceback (most recent call last)")
        || text.contains("npm ERR!")
        || starts_with_ci(t, "error:")
        || starts_with_ci(t, "error[")
        || starts_with_ci(t, "panic:")
        || starts_with_ci(t, "fatal:")
        || text.contains('✖')
        || text.contains('✘');
    if error {
        return Level::Error;
    }
    let warn = has_word(text, "WARN")
        || has_word(text, "WARNING")
        || starts_with_ci(t, "warning:")
        || starts_with_ci(t, "warning[")
        || text.contains("npm WARN")
        || text.contains('⚠');
    if warn {
        Level::Warn
    } else {
        Level::Info
    }
}

/// DevBox 自己写的系统消息（▶ 启动、✖ 退出…）：只认它自己的前缀符号，内容里的 ERROR 字样不算
pub fn line_level(stream: &str, plain: &str) -> Level {
    if stream == "system" {
        let t = plain.trim_start();
        if t.starts_with('✖') || t.starts_with('✘') {
            return Level::Error;
        }
        if t.starts_with('⚠') {
            return Level::Warn;
        }
        return Level::Info;
    }
    detect_level(plain)
}

/// 堆栈续行（"    at com.foo.Bar(Bar.java:12)"），算作上一条错误的一部分
pub fn is_stack_line(plain: &str) -> bool {
    let t = plain.trim_start();
    let indented = plain.len() != t.len();
    indented && (t.starts_with("at ") && t.len() > 3 || t.starts_with("... ") && (t.ends_with(" more") || t.contains("common frames omitted")))
}

/// 查询条件
#[derive(Clone, Debug, Default)]
pub struct Query {
    /// 取末尾多少行（过滤之后）
    pub tail: usize,
    /// 只要序号大于它的行（增量拉取）
    pub since_seq: Option<u64>,
    /// 最低级别；None 表示不限
    pub min_level: Option<Level>,
    /// 关键词，多个用 | 分隔，命中任意一个即可；不区分大小写
    pub grep: Vec<String>,
    /// 排除含这些关键词的行
    pub exclude: Vec<String>,
    /// "stdout" | "stderr" | "system"
    pub stream: Option<String>,
    /// 命中行前后各带几行上下文（只在有 grep 或 level 过滤时有意义）
    pub context: usize,
    /// 返回内容的总字符数上限，超出时保留最新的
    pub max_chars: usize,
}

/// 把 "a|b| c" 拆成 ["a","b","c"]（小写、去空）
pub fn split_terms(s: &str) -> Vec<String> {
    s.split('|').map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty()).collect()
}

#[derive(Serialize, Clone, Debug)]
pub struct OutLine {
    pub seq: u64,
    pub ts: String,
    pub stream: String,
    pub level: Level,
    pub text: String,
}

#[derive(Serialize, Debug)]
pub struct QueryResult {
    pub lines: Vec<OutLine>,
    /// 满足条件的行数（截断之前）
    pub matched: usize,
    /// 被 tail / max_chars 截掉的行数
    pub dropped: usize,
    /// 缓冲里当前最大的序号；下次带 since_seq=这个值 就只拿新增的
    pub last_seq: u64,
    /// 缓冲里还留着多少行
    pub buffered: usize,
}

pub fn run(all: &[LogLine], q: &Query) -> QueryResult {
    let last_seq = all.last().map(|l| l.seq).unwrap_or(0);
    // 先把每行去 ANSI、定级别
    let rows: Vec<OutLine> = all
        .iter()
        .filter(|l| q.since_seq.map_or(true, |s| l.seq > s))
        .filter(|l| q.stream.as_deref().map_or(true, |s| l.stream == s))
        .map(|l| {
            let plain = strip_ansi(&l.text);
            OutLine { seq: l.seq, ts: l.ts.clone(), stream: l.stream.clone(), level: line_level(&l.stream, &plain), text: plain }
        })
        .collect();

    let hit = |r: &OutLine| -> bool {
        let lower = r.text.to_lowercase();
        if q.exclude.iter().any(|t| lower.contains(t.as_str())) {
            return false;
        }
        if let Some(min) = q.min_level {
            if r.level < min {
                return false;
            }
        }
        q.grep.is_empty() || q.grep.iter().any(|t| lower.contains(t.as_str()))
    };
    let filtering = q.min_level.is_some() || !q.grep.is_empty();

    let hits: Vec<bool> = rows.iter().map(hit).collect();
    let matched = hits.iter().filter(|h| **h).count();
    let mut keep = hits.clone();
    if filtering {
        for (i, hit) in hits.iter().enumerate() {
            if *hit && q.context > 0 {
                let lo = i.saturating_sub(q.context);
                let hi = (i + q.context).min(rows.len() - 1);
                for k in keep.iter_mut().take(hi + 1).skip(lo) {
                    *k = true;
                }
            }
        }
        // 错误后面紧跟的堆栈行一并带上，否则只看到异常名看不到出处
        for i in 1..rows.len() {
            if !keep[i] && is_stack_line(&rows[i].text) && keep[i - 1] && (rows[i - 1].level == Level::Error || is_stack_line(&rows[i - 1].text)) {
                keep[i] = true;
            }
        }
    }

    let mut picked: Vec<OutLine> = rows.into_iter().zip(keep).filter(|(_, k)| *k).map(|(r, _)| r).collect();
    let total = picked.len();
    if q.tail > 0 && picked.len() > q.tail {
        picked.drain(..picked.len() - q.tail);
    }
    for r in picked.iter_mut() {
        if r.text.chars().count() > MAX_LINE_CHARS {
            let cut: String = r.text.chars().take(MAX_LINE_CHARS).collect();
            let extra = r.text.chars().count() - MAX_LINE_CHARS;
            r.text = format!("{cut}…(省略 {extra} 字符)");
        }
    }
    // 总字数超限：从最旧的开始丢
    if q.max_chars > 0 {
        let mut used = 0usize;
        let mut first = picked.len();
        for (i, r) in picked.iter().enumerate().rev() {
            used += r.text.chars().count() + 24;
            if used > q.max_chars && first != picked.len() {
                break;
            }
            first = i;
        }
        picked.drain(..first);
    }
    QueryResult { dropped: total - picked.len(), lines: picked, matched, last_seq, buffered: all.len() }
}

/// 渲染成给人 / 模型读的文本：`#序号 时间 [级别标记] 内容`
pub fn render(lines: &[OutLine]) -> String {
    let mut s = String::new();
    for l in lines {
        let mark = match (l.stream.as_str(), l.level) {
            ("system", _) => " [devbox]",
            (_, Level::Error) => " [ERROR]",
            (_, Level::Warn) => " [WARN]",
            _ => "",
        };
        s.push_str(&format!("#{} {}{} {}\n", l.seq, l.ts, mark, l.text));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(seq: u64, stream: &str, text: &str) -> LogLine {
        LogLine { seq, ts: "10:00:00".into(), stream: stream.into(), text: text.into() }
    }

    fn sample() -> Vec<LogLine> {
        vec![
            line(1, "system", "▶ 启动: npm run dev [ERROR 只是命令里的字样]"),
            line(2, "stdout", "\u{1b}[32mready\u{1b}[0m in 300ms"),
            line(3, "stdout", "2026-01-01 INFO  server started"),
            line(4, "stderr", "2026-01-01 WARN  deprecated api"),
            line(5, "stderr", "java.lang.NullPointerException: boom"),
            line(6, "stderr", "    at com.foo.Bar.run(Bar.java:12)"),
            line(7, "stderr", "    at com.foo.Main.main(Main.java:3)"),
            line(8, "stdout", "back to normal"),
            line(9, "system", "✖ 进程已退出：退出码 1"),
        ]
    }

    #[test]
    fn 去掉颜色和控制序列() {
        assert_eq!(strip_ansi("\u{1b}[31;1mred\u{1b}[0m text"), "red text");
        assert_eq!(strip_ansi("\u{1b}]0;title\u{7}hello"), "hello");
        assert_eq!(strip_ansi("\u{1b}]8;;http://x\u{1b}\\link\u{1b}]8;;\u{1b}\\"), "link");
        assert_eq!(strip_ansi("plain 中文"), "plain 中文");
    }

    #[test]
    fn 回车重绘只留最后一次() {
        assert_eq!(strip_ansi("10%\r50%\r100% done"), "100% done");
        assert_eq!(strip_ansi("line\r"), "line");
    }

    #[test]
    fn 识别错误和警告级别() {
        assert_eq!(detect_level("2026 ERROR something"), Level::Error);
        assert_eq!(detect_level("java.lang.NullPointerException: x"), Level::Error);
        assert_eq!(detect_level("TypeError: cannot read"), Level::Error);
        assert_eq!(detect_level("Caused by: java.io.IOException"), Level::Error);
        assert_eq!(detect_level("error[E0308]: mismatched"), Level::Error);
        assert_eq!(detect_level("npm ERR! code ELIFECYCLE"), Level::Error);
        assert_eq!(detect_level("WARN deprecated"), Level::Warn);
        assert_eq!(detect_level("npm WARN old"), Level::Warn);
        assert_eq!(detect_level("warning: unused variable"), Level::Warn);
        assert_eq!(detect_level("server started on 3000"), Level::Info);
        // 单词的一部分不算
        assert_eq!(detect_level("TERRORIST WARNINGS"), Level::Info);
        assert_eq!(detect_level("errors=0"), Level::Info);
    }

    #[test]
    fn 系统消息只看前缀符号() {
        assert_eq!(line_level("system", "▶ 启动: x [ERROR]"), Level::Info);
        assert_eq!(line_level("system", "✖ 进程已退出"), Level::Error);
        assert_eq!(line_level("system", "⚠ 注意"), Level::Warn);
    }

    #[test]
    fn 堆栈行识别() {
        assert!(is_stack_line("    at com.foo.Bar(Bar.java:1)"));
        assert!(is_stack_line("\t... 5 more"));
        assert!(!is_stack_line("at the start"));
        assert!(!is_stack_line("at "));
    }

    #[test]
    fn 默认返回全部并去掉颜色() {
        let r = run(&sample(), &Query { tail: 100, ..Default::default() });
        assert_eq!(r.lines.len(), 9);
        assert_eq!(r.lines[1].text, "ready in 300ms");
        assert_eq!(r.last_seq, 9);
        assert_eq!(r.matched, 9);
        assert_eq!(r.dropped, 0);
    }

    #[test]
    fn tail_只取末尾并报告丢弃数() {
        let r = run(&sample(), &Query { tail: 3, ..Default::default() });
        assert_eq!(r.lines.iter().map(|l| l.seq).collect::<Vec<_>>(), vec![7, 8, 9]);
        assert_eq!(r.dropped, 6);
    }

    #[test]
    fn since_seq_只要新增() {
        let r = run(&sample(), &Query { tail: 100, since_seq: Some(7), ..Default::default() });
        assert_eq!(r.lines.iter().map(|l| l.seq).collect::<Vec<_>>(), vec![8, 9]);
    }

    #[test]
    fn 按级别过滤_错误带上堆栈() {
        let r = run(&sample(), &Query { tail: 100, min_level: Some(Level::Error), ..Default::default() });
        assert_eq!(r.lines.iter().map(|l| l.seq).collect::<Vec<_>>(), vec![5, 6, 7, 9]);
        assert_eq!(r.matched, 2);
    }

    #[test]
    fn warn_包含_error() {
        let r = run(&sample(), &Query { tail: 100, min_level: Some(Level::Warn), ..Default::default() });
        assert_eq!(r.lines.iter().map(|l| l.seq).collect::<Vec<_>>(), vec![4, 5, 6, 7, 9]);
    }

    #[test]
    fn 关键词多选且不分大小写() {
        let q = Query { tail: 100, grep: split_terms("READY| deprecated "), ..Default::default() };
        let r = run(&sample(), &q);
        assert_eq!(r.lines.iter().map(|l| l.seq).collect::<Vec<_>>(), vec![2, 4]);
    }

    #[test]
    fn 上下文行() {
        let q = Query { tail: 100, grep: split_terms("deprecated"), context: 1, ..Default::default() };
        let r = run(&sample(), &q);
        // 上下文带到了第 5 行（一个错误），它后面的堆栈行也一并跟上
        assert_eq!(r.lines.iter().map(|l| l.seq).collect::<Vec<_>>(), vec![3, 4, 5, 6, 7]);
        assert_eq!(r.matched, 1);
    }

    #[test]
    fn 排除关键词() {
        let q = Query { tail: 100, exclude: split_terms("at com.foo|ready"), ..Default::default() };
        let r = run(&sample(), &q);
        assert!(r.lines.iter().all(|l| !l.text.contains("at com.foo") && !l.text.contains("ready")));
        assert_eq!(r.lines.len(), 6);
    }

    #[test]
    fn 按流过滤() {
        let q = Query { tail: 100, stream: Some("stderr".into()), ..Default::default() };
        assert_eq!(run(&sample(), &q).lines.len(), 4);
    }

    #[test]
    fn 超长行与总字数截断() {
        let long = "x".repeat(5000);
        let all = vec![line(1, "stdout", &long), line(2, "stdout", "tail line")];
        let r = run(&all, &Query { tail: 10, ..Default::default() });
        assert!(r.lines[0].text.contains("省略 3000 字符"));

        let many: Vec<LogLine> = (1..=100).map(|i| line(i, "stdout", &format!("line number {i} with some padding text"))).collect();
        let r = run(&many, &Query { tail: 100, max_chars: 600, ..Default::default() });
        assert!(r.lines.len() < 100 && !r.lines.is_empty());
        assert_eq!(r.lines.last().unwrap().seq, 100, "超限时保留最新的");
        assert_eq!(r.dropped, 100 - r.lines.len());
    }

    #[test]
    fn 渲染带级别标记() {
        let r = run(&sample(), &Query { tail: 100, ..Default::default() });
        let text = render(&r.lines);
        assert!(text.contains("#5 10:00:00 [ERROR] java.lang.NullPointerException: boom"));
        assert!(text.contains("#9 10:00:00 [devbox] ✖ 进程已退出"));
        assert!(text.contains("#2 10:00:00 ready in 300ms"));
    }

    #[test]
    fn level_参数解析() {
        assert_eq!(Level::parse("all").unwrap(), None);
        assert_eq!(Level::parse("ERROR").unwrap(), Some(Level::Error));
        assert_eq!(Level::parse("warning").unwrap(), Some(Level::Warn));
        assert!(Level::parse("fatal").is_err());
    }
}
