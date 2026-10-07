//! 运行时版本管理：探测本机已安装的 Node / JDK，并在启动项目时把选定版本注入环境。
//!
//! 不依赖 nvm 之类的 shell 函数（GUI 应用启动的 `sh -lc` 往往读不到它们），
//! 而是直接扫描各版本管理器的安装目录，启动时把对应 bin 目录放到 PATH 最前面。

use crate::models::Project;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct RuntimeVersion {
    /// 展示用版本号，如 "20.11.0"、"17.0.9"
    pub version: String,
    /// 来自哪里，如 "nvm"、"Eclipse Adoptium · SDKMAN"
    pub source: String,
    /// 生效路径：Node 为 bin 目录，JDK 为 JAVA_HOME
    pub path: String,
}

#[derive(Serialize, Default, Debug)]
pub struct Runtimes {
    pub node: Vec<RuntimeVersion>,
    pub java: Vec<RuntimeVersion>,
}

/// 列出本机已安装的 Node / JDK 版本（新版本在前）
#[tauri::command(async)]
pub fn list_runtimes() -> Runtimes {
    installed()
}

pub fn installed() -> Runtimes {
    let home = dirs::home_dir().unwrap_or_default();
    let mut node = scan_node(&home);
    for v in scan_brew_node() {
        if !node.iter().any(|n| n.version == v.version) {
            node.push(v);
        }
    }
    sort_desc(&mut node);
    Runtimes { node, java: scan_java(Path::new("/"), &home) }
}

// ---------- 启动时注入 ----------

/// 生成要拼在启动命令前面的 shell 前缀：把选定的 Node / JDK 放到 PATH 最前。
/// 必须写在命令里而不是设成进程环境变量：登录 shell 读 /etc/profile 时
/// 会重排 PATH，写在命令前缀里才能保证在它之后生效。
pub fn shell_prelude(project: &Project) -> Result<String, String> {
    let mut front: Vec<String> = Vec::new();
    let mut exports = String::new();

    if let Some(n) = &project.node {
        if !Path::new(&n.path).join("node").exists() {
            return Err(format!(
                "所选 Node {} 已不存在（{}），请编辑项目重新选择版本",
                n.version, n.path
            ));
        }
        front.push(n.path.clone());
    }
    if let Some(j) = &project.java {
        if !Path::new(&j.path).join("bin").join("java").exists() {
            return Err(format!(
                "所选 JDK {} 已不存在（{}），请编辑项目重新选择版本",
                j.version, j.path
            ));
        }
        exports.push_str(&format!("export JAVA_HOME={}; ", sh_quote(&j.path)));
        front.push(format!("{}/bin", j.path));
    }
    if front.is_empty() {
        return Ok(String::new());
    }
    Ok(format!(
        "export PATH={}:\"$PATH\"; {}",
        sh_quote(&front.join(":")),
        exports
    ))
}

/// 一句话描述项目用了哪些指定版本，写进项目日志
pub fn describe(project: &Project) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(n) = &project.node {
        parts.push(format!("Node {}", n.version));
    }
    if let Some(j) = &project.java {
        parts.push(format!("JDK {}", j.version));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// ---------- Node 探测 ----------

fn scan_node(home: &Path) -> Vec<RuntimeVersion> {
    let nvm_root = std::env::var_os("NVM_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".nvm"));
    // (来源, 目录, 版本目录到 bin 的相对路径)
    let roots: Vec<(&str, PathBuf, &str)> = vec![
        ("nvm", nvm_root.join("versions/node"), "bin"),
        ("fnm", home.join(".local/share/fnm/node-versions"), "installation/bin"),
        ("fnm", home.join("Library/Application Support/fnm/node-versions"), "installation/bin"),
        ("fnm", home.join(".fnm/node-versions"), "installation/bin"),
        ("Volta", home.join(".volta/tools/image/node"), "bin"),
        ("asdf", home.join(".asdf/installs/nodejs"), "bin"),
        ("n", PathBuf::from("/usr/local/n/versions/node"), "bin"),
    ];

    let mut found: Vec<RuntimeVersion> = Vec::new();
    for (source, root, rel_bin) in roots {
        for dir in sub_dirs(&root) {
            let bin = dir.join(rel_bin);
            if !bin.join("node").exists() {
                continue;
            }
            let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            let version = name.trim_start_matches('v').to_string();
            if version.is_empty() || found.iter().any(|f| f.version == version) {
                continue; // 同一版本多处安装，只保留优先级靠前的来源
            }
            found.push(RuntimeVersion {
                version,
                source: source.to_string(),
                path: bin.to_string_lossy().into_owned(),
            });
        }
    }
    sort_desc(&mut found);
    found
}

/// Homebrew 的 node@XX：目录名里没有完整版本号，要执行一下 `node --version`
fn scan_brew_node() -> Vec<RuntimeVersion> {
    let mut out = Vec::new();
    for prefix in ["/opt/homebrew/opt", "/usr/local/opt"] {
        for dir in sub_dirs(Path::new(prefix)) {
            let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            if name != "node" && !name.starts_with("node@") {
                continue;
            }
            let bin = dir.join("bin");
            let Ok(o) = std::process::Command::new(bin.join("node")).arg("--version").output() else {
                continue;
            };
            let version = String::from_utf8_lossy(&o.stdout).trim().trim_start_matches('v').to_string();
            if !version.is_empty() && !out.iter().any(|r: &RuntimeVersion| r.version == version) {
                out.push(RuntimeVersion {
                    version,
                    source: "Homebrew".into(),
                    path: bin.to_string_lossy().into_owned(),
                });
            }
        }
    }
    out
}

// ---------- JDK 探测 ----------

/// `root` 为文件系统根（生产环境传 "/"，测试里传临时目录以模拟）
fn scan_java(root: &Path, home: &Path) -> Vec<RuntimeVersion> {
    // (来源, 父目录, 版本目录到 JAVA_HOME 的相对路径)
    let sys = |p: &str| root.join(p.trim_start_matches('/'));
    let mut roots: Vec<(String, PathBuf, &str)> = vec![
        ("系统".into(), sys("/Library/Java/JavaVirtualMachines"), "Contents/Home"),
        ("系统".into(), home.join("Library/Java/JavaVirtualMachines"), "Contents/Home"),
        ("SDKMAN".into(), home.join(".sdkman/candidates/java"), ""),
        ("asdf".into(), home.join(".asdf/installs/java"), ""),
        ("系统".into(), sys("/usr/lib/jvm"), ""),
    ];
    // Homebrew 的 openjdk / openjdk@17：名字带版本，位于 opt 下
    for prefix in ["/opt/homebrew/opt", "/usr/local/opt"] {
        for dir in sub_dirs(&sys(prefix)) {
            let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            if name == "openjdk" || name.starts_with("openjdk@") {
                roots.push(("Homebrew".into(), dir.join("libexec"), "openjdk.jdk/Contents/Home"));
            }
        }
    }

    let mut found: Vec<RuntimeVersion> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for (source, parent, rel_home) in roots {
        for dir in sub_dirs(&parent) {
            if dir.file_name().and_then(|n| n.to_str()) == Some("current") {
                continue; // SDKMAN 的 current 只是软链
            }
            let java_home = if rel_home.is_empty() { dir.clone() } else { dir.join(rel_home) };
            if !java_home.join("bin/java").exists() {
                continue;
            }
            // 软链指向同一个 JDK 时只记一次
            let canon = std::fs::canonicalize(&java_home).unwrap_or_else(|_| java_home.clone());
            if seen.contains(&canon) {
                continue;
            }
            seen.push(canon);

            let dir_name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            let (version, vendor) = read_release(&java_home, dir_name);
            let label = match vendor {
                Some(v) => format!("{v} · {source}"),
                None => source.clone(),
            };
            found.push(RuntimeVersion {
                version,
                source: label,
                path: java_home.to_string_lossy().into_owned(),
            });
        }
    }
    sort_desc(&mut found);
    found
}

/// 读取 JDK 目录下的 release 文件，拿到版本号与发行方；读不到就用目录名
fn read_release(java_home: &Path, fallback: &str) -> (String, Option<String>) {
    let text = std::fs::read_to_string(java_home.join("release")).unwrap_or_default();
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{key}=")))
            .map(|v| v.trim().trim_matches('"').to_string())
            .filter(|v| !v.is_empty())
    };
    (
        field("JAVA_VERSION").unwrap_or_else(|| fallback.to_string()),
        field("IMPLEMENTOR"),
    )
}

// ---------- 识别项目要求的版本 ----------

/// 项目声明的版本要求
#[derive(Debug, PartialEq, Clone)]
pub struct Requirement {
    pub wanted: String,
    pub source: String,
}

fn read_text(dir: &Path, file: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(file)).ok()
}

/// 读取 Node 版本要求，优先级：.nvmrc > .node-version > .tool-versions > package.json(volta / engines)
pub fn node_requirement(dir: &Path) -> Option<Requirement> {
    let clean = |raw: &str| -> Option<String> {
        let t = raw.trim().trim_start_matches('v').trim().to_string();
        let lower = t.to_lowercase();
        // lts/*、node、latest 之类没有具体版本号，无法据此选择
        let vague = t.is_empty() || lower.starts_with("lts") || ["node", "latest", "stable", "current", "*", "x"].contains(&lower.as_str());
        (!vague).then_some(t)
    };
    for file in [".nvmrc", ".node-version"] {
        if let Some(w) = read_text(dir, file).and_then(|t| t.lines().next().and_then(&clean)) {
            return Some(Requirement { wanted: w, source: file.into() });
        }
    }
    if let Some(text) = read_text(dir, ".tool-versions") {
        for line in text.lines() {
            let mut it = line.split_whitespace();
            if matches!(it.next(), Some("nodejs") | Some("node")) {
                if let Some(w) = it.next().and_then(&clean) {
                    return Some(Requirement { wanted: w, source: ".tool-versions".into() });
                }
            }
        }
    }
    if let Some(json) = read_text(dir, "package.json").and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) {
        if let Some(w) = json.pointer("/volta/node").and_then(|v| v.as_str()).and_then(&clean) {
            return Some(Requirement { wanted: w, source: "package.json (volta)".into() });
        }
        if let Some(w) = json.pointer("/engines/node").and_then(|v| v.as_str()).and_then(&clean) {
            return Some(Requirement { wanted: w, source: "package.json (engines)".into() });
        }
    }
    None
}

/// 读取 JDK 主版本要求，优先级：.sdkmanrc > .tool-versions > .java-version > pom.xml > Gradle
pub fn java_requirement(dir: &Path) -> Option<Requirement> {
    let found = |major: u32, src: &str| Some(Requirement { wanted: major.to_string(), source: src.into() });

    if let Some(text) = read_text(dir, ".sdkmanrc") {
        for line in text.lines() {
            if let Some(v) = line.trim().strip_prefix("java=") {
                if let Some(m) = java_major(v) {
                    return found(m, ".sdkmanrc");
                }
            }
        }
    }
    if let Some(text) = read_text(dir, ".tool-versions") {
        for line in text.lines() {
            let mut it = line.split_whitespace();
            if it.next() == Some("java") {
                if let Some(m) = it.next().and_then(java_major) {
                    return found(m, ".tool-versions");
                }
            }
        }
    }
    if let Some(m) = read_text(dir, ".java-version").and_then(|t| t.lines().next().and_then(java_major)) {
        return found(m, ".java-version");
    }
    if let Some(pom) = read_text(dir, "pom.xml") {
        for tag in ["java.version", "maven.compiler.release", "maven.compiler.source", "jdk.version", "maven.compiler.target"] {
            if let Some(m) = xml_tag(&pom, tag).as_deref().and_then(java_major) {
                return found(m, "pom.xml");
            }
        }
    }
    for file in ["build.gradle", "build.gradle.kts"] {
        if let Some(text) = read_text(dir, file) {
            // JavaLanguageVersion.of(17) / jvmToolchain(17) / sourceCompatibility = '1.8' / JavaVersion.VERSION_1_8
            for key in ["JavaLanguageVersion.of(", "jvmToolchain(", "sourceCompatibility", "targetCompatibility", "jvmTarget"] {
                if let Some(i) = text.find(key) {
                    let rest = &text[i + key.len()..];
                    let rest = rest.split('\n').next().unwrap_or("");
                    let rest = rest.replace("JavaVersion.VERSION_", "").replace('_', ".");
                    // 数字前面还有 "= '" 之类的语法符号，截到第一个数字再解析
                    let rest = rest.find(|c: char| c.is_ascii_digit()).map_or("", |i| &rest[i..]);
                    if let Some(m) = java_major(rest) {
                        return found(m, file);
                    }
                }
            }
        }
    }
    None
}

fn xml_tag(s: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let i = s.find(&open)? + open.len();
    let j = s[i..].find(&format!("</{tag}>"))?;
    Some(s[i..i + j].trim().to_string())
}

/// 把各种写法的 JDK 版本归一成主版本：1.8 → 8，17.0.9 → 17，corretto-8.402 → 8，temurin-17.0.9 → 17
/// 找不到数字（如 ${java.version}）返回 None
pub fn java_major(raw: &str) -> Option<u32> {
    // 先去掉发行方名字：按 '-' 分段，取第一个以数字开头的段
    //（openjdk64-17.0.1 里的 64 属于名字，不能当版本号）
    let ver = raw.split('-').find(|seg| seg.starts_with(|c: char| c.is_ascii_digit()))?;
    let nums: Vec<u32> = ver
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .take(2)
        .filter_map(|s| s.parse().ok())
        .collect();
    match nums.as_slice() {
        [1, minor, ..] => Some(*minor), // 旧式 1.8 / 1.7
        [major, ..] => Some(*major),
        [] => None,
    }
}

// ---- 一个够用的 semver 范围匹配（engines.node 常见写法）----

/// 解析 "20" / "20.11" / "20.11.0" / "18.x"；缺失或通配的部分为 None
fn parse_partial(s: &str) -> Option<[Option<u64>; 3]> {
    let s = s.trim().trim_start_matches('v');
    if s.is_empty() {
        return None;
    }
    let mut out = [None; 3];
    for (i, part) in s.split('.').take(3).enumerate() {
        let part = part.split(|c: char| !c.is_ascii_alphanumeric() && c != '*').next().unwrap_or("");
        out[i] = match part {
            "x" | "X" | "*" | "" => None,
            p => Some(p.parse::<u64>().ok()?),
        };
    }
    out[0]?; // 主版本必须是具体数字
    Some(out)
}

fn full(v: &str) -> Option<[u64; 3]> {
    let p = parse_partial(v)?;
    Some([p[0]?, p[1].unwrap_or(0), p[2].unwrap_or(0)])
}

fn satisfies_comparator(ver: [u64; 3], comp: &str) -> bool {
    let (op, rest) = ["<=", ">=", "<", ">", "=", "^", "~"]
        .iter()
        .find_map(|op| comp.strip_prefix(op).map(|r| (*op, r)))
        .unwrap_or(("", comp));
    let Some(p) = parse_partial(rest) else { return false };
    let base = [p[0].unwrap_or(0), p[1].unwrap_or(0), p[2].unwrap_or(0)];
    match op {
        "" | "=" => (0..3).all(|i| p[i].map_or(true, |x| x == ver[i])),
        ">=" => ver >= base,
        // ">17" 表示 >=18；">17.2" 表示 >=17.3
        ">" => match (p[1], p[2]) {
            (None, _) => ver[0] > base[0],
            (Some(_), None) => (ver[0], ver[1]) > (base[0], base[1]),
            _ => ver > base,
        },
        "<" => ver < base,
        "<=" => match (p[1], p[2]) {
            (None, _) => ver[0] <= base[0],
            (Some(_), None) => (ver[0], ver[1]) <= (base[0], base[1]),
            _ => ver <= base,
        },
        "^" => {
            ver >= base
                && if base[0] > 0 {
                    ver[0] == base[0]
                } else if base[1] > 0 || p[1].is_some() {
                    ver[0] == 0 && ver[1] == base[1]
                } else {
                    ver[0] == 0 && ver[1] == 0 && ver[2] == base[2]
                }
        }
        "~" => ver >= base && ver[0] == base[0] && p[1].map_or(true, |m| ver[1] == m),
        _ => false,
    }
}

/// 版本是否满足范围，支持 `||`、空格分隔的与条件、连字符范围（16 - 20）
pub fn version_satisfies(version: &str, spec: &str) -> bool {
    let Some(ver) = full(version) else { return false };
    spec.split("||").any(|group| {
        // 先把 ">= 16" 这种操作符后带空格的写法并拢，并识别 "a - b"
        let mut toks: Vec<String> = Vec::new();
        for t in group.split_whitespace() {
            match toks.last_mut() {
                Some(last) if matches!(last.as_str(), ">=" | "<=" | ">" | "<" | "=" | "^" | "~") => last.push_str(t),
                _ => toks.push(t.to_string()),
            }
        }
        let mut comps: Vec<String> = Vec::new();
        let mut i = 0;
        while i < toks.len() {
            if i + 2 < toks.len() && toks[i + 1] == "-" {
                comps.push(format!(">={}", toks[i]));
                comps.push(format!("<={}", toks[i + 2]));
                i += 3;
            } else {
                comps.push(toks[i].clone());
                i += 1;
            }
        }
        !comps.is_empty() && comps.iter().all(|c| satisfies_comparator(ver, c))
    })
}

/// 从本机已安装的 Node 里挑一个满足要求的：取「满足要求的最低主版本」（最保守，
/// 老项目在高版本 Node 上常有兼容问题），同一主版本里取最新。
pub fn pick_node(spec: &str, installed: &[RuntimeVersion]) -> Option<RuntimeVersion> {
    let ok: Vec<&RuntimeVersion> = installed.iter().filter(|r| version_satisfies(&r.version, spec)).collect();
    let low = ok.iter().filter_map(|r| version_key(&r.version).first().copied()).min()?;
    ok.into_iter()
        .filter(|r| version_key(&r.version).first().copied() == Some(low))
        .max_by_key(|r| version_key(&r.version))
        .cloned()
}

/// 从本机已安装的 JDK 里挑主版本一致的，同主版本取最新
pub fn pick_java(major: u32, installed: &[RuntimeVersion]) -> Option<RuntimeVersion> {
    installed
        .iter()
        .filter(|r| java_major(&r.version) == Some(major))
        .max_by_key(|r| version_key(&r.version))
        .cloned()
}

/// 综合：读取项目要求并在本机已安装版本里匹配
pub fn suggest_node(dir: &Path, installed: &[RuntimeVersion]) -> Option<crate::models::RuntimeSuggestion> {
    let req = node_requirement(dir)?;
    let matched = pick_node(&req.wanted, installed).map(|r| crate::models::RuntimeChoice { version: r.version, path: r.path });
    Some(crate::models::RuntimeSuggestion { wanted: req.wanted, source: req.source, matched })
}

pub fn suggest_java(dir: &Path, installed: &[RuntimeVersion]) -> Option<crate::models::RuntimeSuggestion> {
    let req = java_requirement(dir)?;
    let major: u32 = req.wanted.parse().ok()?;
    let matched = pick_java(major, installed).map(|r| crate::models::RuntimeChoice { version: r.version, path: r.path });
    Some(crate::models::RuntimeSuggestion { wanted: req.wanted, source: req.source, matched })
}

// ---------- 通用 ----------

fn sub_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// 版本号拆成数字序列用于排序；旧式 "1.8.0_392" 按 8 处理
fn version_key(v: &str) -> Vec<u64> {
    let mut nums: Vec<u64> = v
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    if nums.len() >= 2 && nums[0] == 1 {
        nums.remove(0);
    }
    nums
}

fn sort_desc(list: &mut [RuntimeVersion]) {
    list.sort_by(|a, b| version_key(&b.version).cmp(&version_key(&a.version)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("devbox-rt-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn script(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn project(extra: serde_json::Value) -> Project {
        let mut v = serde_json::json!({"id":"t","name":"t","path":"/tmp","start_command":"x"});
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn scans_node_from_several_managers_and_sorts_desc() {
        let home = tmp("node");
        script(&home.join(".nvm/versions/node/v18.19.1/bin/node"), "echo v18.19.1");
        script(&home.join(".nvm/versions/node/v20.11.0/bin/node"), "echo v20.11.0");
        script(&home.join(".local/share/fnm/node-versions/v22.1.0/installation/bin/node"), "echo");
        script(&home.join(".volta/tools/image/node/20.11.0/bin/node"), "echo"); // 与 nvm 重复，应被去重
        fs::create_dir_all(home.join(".nvm/versions/node/v16.0.0")).unwrap(); // 没有 bin/node，应忽略

        let got = scan_node(&home);
        let versions: Vec<_> = got.iter().map(|r| r.version.as_str()).collect();
        assert_eq!(versions, vec!["22.1.0", "20.11.0", "18.19.1"]);
        assert_eq!(got[1].source, "nvm", "重复版本保留优先级靠前的来源");
        assert!(got[0].path.ends_with("installation/bin"));
    }

    #[test]
    fn scans_java_reads_release_and_dedups_symlinks() {
        let root = tmp("java-root");
        let home = tmp("java-home");
        let jdk17 = root.join("usr/lib/jvm/java-17-openjdk");
        script(&jdk17.join("bin/java"), "echo");
        fs::write(jdk17.join("release"), "JAVA_VERSION=\"17.0.9\"\nIMPLEMENTOR=\"Eclipse Adoptium\"\n").unwrap();
        std::os::unix::fs::symlink(&jdk17, root.join("usr/lib/jvm/default-java")).unwrap();
        let jdk8 = home.join(".sdkman/candidates/java/8.0.392-tem");
        script(&jdk8.join("bin/java"), "echo");
        fs::write(jdk8.join("release"), "JAVA_VERSION=\"1.8.0_392\"\n").unwrap();
        std::os::unix::fs::symlink(&jdk8, home.join(".sdkman/candidates/java/current")).unwrap();
        // macOS 风格：Contents/Home
        let mac = root.join("Library/Java/JavaVirtualMachines/jdk-21.jdk/Contents/Home");
        script(&mac.join("bin/java"), "echo");
        fs::write(mac.join("release"), "JAVA_VERSION=\"21.0.1\"\n").unwrap();

        let got = scan_java(&root, &home);
        let versions: Vec<_> = got.iter().map(|r| r.version.as_str()).collect();
        assert_eq!(versions, vec!["21.0.1", "17.0.9", "1.8.0_392"], "软链不重复，新版本在前: {got:?}");
        assert_eq!(got[1].source, "Eclipse Adoptium · 系统");
        assert!(got[0].path.ends_with("Contents/Home"));
    }

    #[test]
    fn version_sorting_handles_legacy_java_numbers() {
        assert!(version_key("1.8.0_392") < version_key("11.0.2"));
        assert!(version_key("17.0.9") < version_key("21.0.1"));
        assert!(version_key("9.0.1") < version_key("17.0.1"));
    }

    #[test]
    fn prelude_is_empty_without_choice_and_quotes_paths() {
        assert_eq!(shell_prelude(&project(serde_json::json!({}))).unwrap(), "");

        let d = tmp("prelude");
        let bin = d.join("it's node/bin"); // 路径里带单引号，验证转义
        script(&bin.join("node"), "echo");
        let p = project(serde_json::json!({"node": {"version":"20.0.0","path": bin.to_string_lossy()}}));
        let pre = shell_prelude(&p).unwrap();
        assert!(pre.starts_with("export PATH='"), "{pre}");
        assert!(pre.contains("'\\''"), "单引号必须被转义: {pre}");
        assert!(pre.ends_with(":\"$PATH\"; "), "{pre}");
    }

    #[test]
    fn prelude_reports_missing_versions() {
        let p = project(serde_json::json!({"node": {"version":"18.0.0","path":"/no/such/bin"}}));
        let e = shell_prelude(&p).unwrap_err();
        assert!(e.contains("Node 18.0.0") && e.contains("已不存在"), "{e}");
        let p = project(serde_json::json!({"java": {"version":"17","path":"/no/such/jdk"}}));
        assert!(shell_prelude(&p).unwrap_err().contains("JDK 17"));
    }

    #[test]
    fn selected_versions_actually_take_effect_in_a_shell() {
        // 真正用 sh 跑一遍：PATH 里先找到我们的 node，JAVA_HOME 被设置
        let d = tmp("effect");
        script(&d.join("n/bin/node"), "echo fake-node-v99");
        script(&d.join("jdk/bin/java"), "echo fake-java-$JAVA_HOME");
        let p = project(serde_json::json!({
            "node": {"version":"99.0.0","path": d.join("n/bin").to_string_lossy()},
            "java": {"version":"21","path": d.join("jdk").to_string_lossy()},
        }));
        let cmd = format!("{}node; java", shell_prelude(&p).unwrap());
        let out = std::process::Command::new("/bin/sh").arg("-lc").arg(cmd).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("fake-node-v99"), "{text}");
        assert!(text.contains(&format!("fake-java-{}", d.join("jdk").display())), "{text}");
    }

    #[test]
    fn java_major_normalizes_all_spellings() {
        for (raw, want) in [("1.8", 8), ("1.8.0_392", 8), ("8", 8), ("17", 17), ("17.0.9", 17), ("corretto-8.402.08.1", 8),
            ("temurin-17.0.9", 17), ("openjdk64-17.0.1", 17), ("21.0.1-tem", 21), ("11.0.11+9", 11), ("1.7", 7)] {
            assert_eq!(java_major(raw), Some(want), "{raw}");
        }
        assert_eq!(java_major("${java.version}"), None);
        assert_eq!(java_major(""), None);
    }

    #[test]
    fn semver_ranges() {
        let yes = |v: &str, s: &str| assert!(version_satisfies(v, s), "{v} 应满足 {s}");
        let no = |v: &str, s: &str| assert!(!version_satisfies(v, s), "{v} 不应满足 {s}");
        yes("20.11.0", "20"); yes("20.11.0", "20.11"); yes("20.11.0", "20.11.0"); no("20.11.0", "20.5");
        yes("18.19.1", "18.x"); yes("18.19.1", "18.*"); no("19.0.0", "18.x");
        yes("18.0.0", ">=18"); no("16.9.0", ">=18"); yes("22.0.0", ">= 18");
        yes("18.19.1", ">=16 <21"); no("21.0.0", ">=16 <21"); no("15.0.0", ">=16 <21");
        yes("18.19.1", "^18.0.0"); no("19.0.0", "^18.0.0"); no("17.9.0", "^18.0.0"); yes("0.2.9", "^0.2.3"); no("0.3.0", "^0.2.3");
        yes("18.2.5", "~18.2.0"); no("18.3.0", "~18.2.0"); yes("18.9.0", "~18");
        yes("20.1.0", "^18 || ^20"); no("19.1.0", "^18 || ^20");
        yes("18.0.0", "16 - 20"); no("21.0.0", "16 - 20");
        yes("18.0.0", ">17"); no("17.9.0", ">17"); yes("20.0.0", "<=20"); yes("20.1.0", "<=20"); no("21.0.0", "<=20");
        no("20.0.0", "<20"); yes("19.9.9", "<20");
        no("abc", "20"); no("20.0.0", "garbage");
    }

    #[test]
    fn pick_node_prefers_lowest_satisfying_major_then_latest() {
        let inst = vec![
            RuntimeVersion { version: "22.1.0".into(), source: "t".into(), path: "/22".into() },
            RuntimeVersion { version: "20.11.0".into(), source: "t".into(), path: "/20".into() },
            RuntimeVersion { version: "20.5.1".into(), source: "t".into(), path: "/20old".into() },
            RuntimeVersion { version: "18.19.1".into(), source: "t".into(), path: "/18".into() },
        ];
        assert_eq!(pick_node(">=18", &inst).unwrap().path, "/18");
        assert_eq!(pick_node(">=19", &inst).unwrap().path, "/20", "同主版本取最新，而不是 20.5.1");
        assert_eq!(pick_node("^18 || ^20", &inst).unwrap().path, "/18");
        assert_eq!(pick_node("22", &inst).unwrap().path, "/22");
        assert!(pick_node("16", &inst).is_none());
        assert!(pick_node(">=99", &inst).is_none());
    }
}
