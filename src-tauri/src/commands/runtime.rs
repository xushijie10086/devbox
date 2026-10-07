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
}
