//! 脚本 / 构建任务：给每个项目一个「运行脚本」菜单（install、build、test 等一次性命令）。
//!
//! 和启动项目共用同一套环境（选定的 Node / JDK、项目环境变量），输出写进项目日志，
//! 每个项目同时只跑一个脚本任务，可以取消。

use crate::commands::process::{
    describe_exit, get_project, push_log, shell_command, spawn_reader, tail_since_start, TAIL_LINES,
};
use crate::models::StartOutcome;
use crate::state::{AppState, Job};
use serde::Serialize;
use std::path::Path;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, State};

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ScriptEntry {
    /// 展示名，如 "build"、"安装到本地仓库（跳过测试）"
    pub label: String,
    pub command: String,
    /// 来源，如 "package.json"、"pom.xml"
    pub source: String,
    /// 需要提醒的一句话，如「尚未安装依赖」
    pub hint: Option<String>,
}

fn entry(label: &str, command: impl Into<String>, source: &str, hint: Option<&str>) -> ScriptEntry {
    ScriptEntry {
        label: label.into(),
        command: command.into(),
        source: source.into(),
        hint: hint.map(|h| h.to_string()),
    }
}

/// package.json 里按「常用在前」排序的脚本名
const COMMON_ORDER: [&str; 11] =
    ["build", "test", "lint", "dev", "start", "serve", "preview", "format", "typecheck", "type-check", "check"];

/// 识别项目目录里能运行的脚本（只读文件，不执行任何东西）
pub fn detect_scripts(dir: &Path) -> Vec<ScriptEntry> {
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).ok();
    let mut out: Vec<ScriptEntry> = Vec::new();

    // ---- Node ----
    if let Some(pkg) = read("package.json").and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) {
        let pm = if dir.join("pnpm-lock.yaml").exists() {
            "pnpm"
        } else if dir.join("yarn.lock").exists() {
            "yarn"
        } else {
            "npm"
        };
        let hint = (!dir.join("node_modules").exists()).then_some("尚未安装依赖（没有 node_modules），建议先执行");
        out.push(entry("安装依赖", format!("{pm} install"), "package.json", hint));
        if let Some(scripts) = pkg.get("scripts").and_then(|v| v.as_object()) {
            let mut names: Vec<&String> = scripts
                .keys()
                // npm 的 pre / post 钩子会在主脚本前后自动执行，不必单列
                .filter(|k| {
                    !(k.starts_with("pre") && scripts.contains_key(&k[3..]))
                        && !(k.starts_with("post") && scripts.contains_key(&k[4..]))
                })
                .collect();
            let rank = |k: &str| COMMON_ORDER.iter().position(|c| *c == k).unwrap_or(usize::MAX);
            names.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.cmp(b)));
            for k in names.into_iter().take(25) {
                out.push(entry(k, format!("{pm} run {k}"), "package.json", None));
            }
        }
    }

    // ---- Maven ----
    if let Some(pom) = read("pom.xml") {
        let mvn = if dir.join("mvnw").exists() { "./mvnw" } else { "mvn" };
        let multi = pom.contains("<modules>");
        out.push(entry("清理并打包（跳过测试）", format!("{mvn} clean package -DskipTests"), "pom.xml", None));
        out.push(entry(
            "安装到本地仓库（跳过测试）",
            format!("{mvn} clean install -DskipTests"),
            "pom.xml",
            multi.then_some("多模块项目：先 install，才能在子模块里单独 mvn spring-boot:run"),
        ));
        out.push(entry("编译", format!("{mvn} compile"), "pom.xml", None));
        out.push(entry("运行测试", format!("{mvn} test"), "pom.xml", None));
        out.push(entry("清理", format!("{mvn} clean"), "pom.xml", None));
    }

    // ---- Gradle ----
    if dir.join("build.gradle").exists() || dir.join("build.gradle.kts").exists() {
        let g = if dir.join("gradlew").exists() { "./gradlew" } else { "gradle" };
        out.push(entry("构建（跳过测试）", format!("{g} build -x test"), "Gradle", None));
        out.push(entry("运行测试", format!("{g} test"), "Gradle", None));
        out.push(entry("清理", format!("{g} clean"), "Gradle", None));
    }

    // ---- Rust / Go / Python ----
    if dir.join("Cargo.toml").exists() {
        for (l, c) in [("构建", "cargo build"), ("构建（release）", "cargo build --release"), ("运行测试", "cargo test"), ("检查", "cargo check")] {
            out.push(entry(l, c, "Cargo.toml", None));
        }
    }
    if dir.join("go.mod").exists() {
        for (l, c) in [("构建", "go build ./..."), ("运行测试", "go test ./..."), ("整理依赖", "go mod tidy")] {
            out.push(entry(l, c, "go.mod", None));
        }
    }
    if dir.join("requirements.txt").exists() {
        out.push(entry("安装依赖", "pip install -r requirements.txt", "Python", None));
    }
    if read("pyproject.toml").is_some_and(|t| t.contains("[tool.poetry]")) {
        out.push(entry("安装依赖", "poetry install", "Python", None));
    }
    if dir.join("pytest.ini").exists() || dir.join("tests").is_dir() && dir.join("requirements.txt").exists() {
        out.push(entry("运行测试", "pytest", "Python", None));
    }

    // ---- Makefile ----
    if let Some(mk) = read("Makefile") {
        let mut n = 0;
        for line in mk.lines() {
            // 形如 `build:` / `test: deps`；跳过 .PHONY、模式规则、变量赋值
            let Some((target, rest)) = line.split_once(':') else { continue };
            let ok = !target.is_empty()
                && !target.starts_with('.')
                && !rest.starts_with('=')
                && target.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
            if ok && !out.iter().any(|e| e.source == "Makefile" && e.label == target) {
                out.push(entry(target, format!("make {target}"), "Makefile", None));
                n += 1;
                if n >= 15 {
                    break;
                }
            }
        }
    }
    out
}

/// 列出某个项目可运行的脚本
#[tauri::command]
pub fn list_scripts(id: String, state: State<AppState>) -> Result<Vec<ScriptEntry>, String> {
    let project = get_project(&state, &id).ok_or("找不到项目")?;
    if !Path::new(&project.path).is_dir() {
        return Err(format!("工作目录不存在或不是目录：{}", project.path));
    }
    Ok(detect_scripts(Path::new(&project.path)))
}

/// 运行一个脚本任务，等它结束并返回结果；可能很久（编译），所以放到阻塞线程里
#[tauri::command]
pub async fn run_script(id: String, command: String, label: String, app: AppHandle) -> Result<StartOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        run_script_blocking(&state, &id, &command, &label)
    })
    .await
    .map_err(|e| format!("内部错误: {e}"))?
}

/// 取消某个项目正在运行的脚本任务
#[tauri::command]
pub fn cancel_script(id: String, state: State<AppState>) -> Result<(), String> {
    cancel_job(&state, &id)
}

pub fn cancel_job(state: &AppState, id: &str) -> Result<(), String> {
    let pid = {
        let mut jobs = state.jobs.lock().unwrap();
        let job = jobs.get_mut(id).ok_or("没有正在运行的脚本")?;
        job.cancelled = true;
        job.pid
    };
    crate::commands::process::kill_tree(pid);
    Ok(())
}

/// 取消所有脚本任务（退出应用时用），返回取消的个数
pub fn cancel_all_jobs(state: &AppState) -> usize {
    let ids: Vec<String> = state.jobs.lock().unwrap().keys().cloned().collect();
    ids.iter().filter(|id| cancel_job(state, id).is_ok()).count()
}

pub fn run_script_blocking(state: &AppState, id: &str, command: &str, label: &str) -> Result<StartOutcome, String> {
    let project = get_project(state, id).ok_or("找不到项目")?;
    if command.trim().is_empty() {
        return Err("命令为空".into());
    }
    let label = if label.trim().is_empty() { command } else { label };

    // 先占位再启动，避免两次点击同时通过检查
    {
        let jobs = state.jobs.lock().unwrap();
        if let Some(j) = jobs.get(id) {
            return Err(format!("已有脚本「{}」在运行，请等它结束或先取消", j.label));
        }
    }
    let mut cmd = shell_command(&project, command)?;
    let mut child = cmd.spawn().map_err(|e| format!("无法执行脚本: {e}"))?;
    let pid = child.id();
    {
        let mut jobs = state.jobs.lock().unwrap();
        if jobs.contains_key(id) {
            drop(jobs);
            crate::commands::process::kill_tree(pid);
            let _ = child.kill();
            let _ = child.wait();
            return Err("已有脚本在运行".into());
        }
        jobs.insert(id.to_string(), Job { label: label.to_string(), pid, started_at: Instant::now(), cancelled: false });
    }

    let buf = state.log_buffer(id);
    push_log(&buf, "system", format!("▶ 脚本「{label}」: {command} (pid {pid})"));
    if let Some(rt) = crate::commands::runtime::describe(&project) {
        push_log(&buf, "system", format!("使用指定版本: {rt}"));
    }
    if let Some(out) = child.stdout.take() {
        spawn_reader(out, "stdout", buf.clone());
    }
    if let Some(err) = child.stderr.take() {
        spawn_reader(err, "stderr", buf.clone());
    }

    let started = Instant::now();
    let status = child.wait();
    let secs = started.elapsed().as_secs_f32();
    std::thread::sleep(Duration::from_millis(250)); // 等读线程把最后的输出写进日志
    let cancelled = state.jobs.lock().unwrap().remove(id).is_some_and(|j| j.cancelled);

    // (返回给前端的结果, 要发的系统通知)。用户主动取消不通知
    let (outcome, notice): (Result<StartOutcome, String>, Option<(String, String)>) = match status {
        _ if cancelled => {
            push_log(&buf, "system", format!("■ 脚本「{label}」已取消"));
            (Ok(StartOutcome::warning(format!("脚本「{label}」已取消"))), None)
        }
        Ok(st) if st.success() => {
            let m = format!("脚本「{label}」完成（用时 {secs:.1} 秒）");
            push_log(&buf, "system", format!("✔ {m}"));
            (
                Ok(StartOutcome::success(m)),
                Some((format!("「{}」脚本「{label}」完成", project.name), format!("用时 {secs:.1} 秒"))),
            )
        }
        Ok(st) => {
            use std::os::unix::process::ExitStatusExt;
            let reason = describe_exit(st.code(), st.signal());
            push_log(&buf, "system", format!("✖ 脚本「{label}」失败：{reason}"));
            let tail = tail_since_start(&buf, TAIL_LINES);
            let mut msg = format!("脚本「{label}」失败：{reason}");
            if !tail.is_empty() {
                msg.push_str("\n最后输出：\n");
                msg.push_str(&tail.join("\n"));
            }
            let last = tail.last().cloned().unwrap_or_default();
            (Err(msg), Some((format!("「{}」脚本「{label}」失败", project.name), format!("{reason}\n{last}"))))
        }
        Err(e) => (Err(format!("等待脚本结束失败: {e}")), None),
    };
    // 一次编译可能要好几分钟，用户多半已经切走了：窗口不在前台时发系统通知
    if let Some((title, body)) = notice {
        crate::notify::notify(state, &title, body.trim_end(), false);
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Config;

    fn dir(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("devbox-scripts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for (f, c) in files {
            let p = d.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, c).unwrap();
        }
        d
    }

    fn labels(v: &[ScriptEntry]) -> Vec<&str> {
        v.iter().map(|e| e.label.as_str()).collect()
    }

    #[test]
    fn node_scripts_ordered_with_package_manager_and_hooks_hidden() {
        let pkg = r#"{"scripts":{"zeta":"x","build":"vite build","dev":"vite","prebuild":"x","postbuild":"y","test":"vitest","lint":"eslint .","preinstall":"z","alpha":"a"}}"#;
        let d = dir("node", &[("package.json", pkg), ("pnpm-lock.yaml", "")]);
        let s = detect_scripts(&d);
        // 安装依赖在最前；常用脚本按固定顺序；其余按字母；prebuild / postbuild 被隐藏（主脚本会自动触发）
        assert_eq!(labels(&s), vec!["安装依赖", "build", "test", "lint", "dev", "alpha", "preinstall", "zeta"]);
        assert_eq!(s[0].command, "pnpm install");
        assert_eq!(s[1].command, "pnpm run build");
        assert!(s[0].hint.as_deref().unwrap().contains("node_modules"), "没有 node_modules 时提醒先安装");
        // 装过依赖就不再提醒；yarn / npm 的命令前缀
        std::fs::create_dir_all(d.join("node_modules")).unwrap();
        assert!(detect_scripts(&d)[0].hint.is_none());
        let d = dir("yarn", &[("package.json", r#"{"scripts":{"build":"x"}}"#), ("yarn.lock", "")]);
        assert_eq!(detect_scripts(&d)[1].command, "yarn run build");
        let d = dir("npm", &[("package.json", r#"{"scripts":{"build":"x"}}"#)]);
        assert_eq!(detect_scripts(&d)[1].command, "npm run build");
    }

    #[test]
    fn maven_uses_wrapper_and_warns_for_multi_module() {
        let d = dir("mvn", &[("pom.xml", "<project><modules><module>a</module></modules></project>"), ("mvnw", "#!/bin/sh")]);
        let s = detect_scripts(&d);
        let install = s.iter().find(|e| e.label.starts_with("安装到本地仓库")).unwrap();
        assert_eq!(install.command, "./mvnw clean install -DskipTests");
        assert!(install.hint.as_deref().unwrap().contains("多模块"));
        // 单模块、没有 wrapper
        let d = dir("mvn1", &[("pom.xml", "<project/>")]);
        let s = detect_scripts(&d);
        assert!(s.iter().all(|e| e.hint.is_none()));
        assert_eq!(s[0].command, "mvn clean package -DskipTests");
    }

    #[test]
    fn gradle_cargo_go_python_make() {
        let d = dir("gradle", &[("build.gradle", ""), ("gradlew", "")]);
        assert_eq!(detect_scripts(&d)[0].command, "./gradlew build -x test");
        let d = dir("cargo", &[("Cargo.toml", "")]);
        assert!(detect_scripts(&d).iter().any(|e| e.command == "cargo test"));
        let d = dir("go", &[("go.mod", "")]);
        assert!(detect_scripts(&d).iter().any(|e| e.command == "go test ./..."));
        let d = dir("py", &[("requirements.txt", "flask")]);
        assert_eq!(detect_scripts(&d)[0].command, "pip install -r requirements.txt");
        let mk = ".PHONY: build test\nCC := gcc\nbuild:\n\techo b\ntest: build\n\techo t\n%.o: %.c\n\tx\nrun-dev:\n\techo\nbuild:\n\tdup\n";
        let d = dir("make", &[("Makefile", mk)]);
        assert_eq!(labels(&detect_scripts(&d)), vec!["build", "test", "run-dev"], "跳过 .PHONY、变量、模式规则，且去重");
    }

    #[test]
    fn unknown_dir_has_no_scripts() {
        assert!(detect_scripts(&dir("empty", &[])).is_empty());
    }

    fn state(cmd_dir: &str) -> AppState {
        AppState::for_test(Config {
            projects: vec![serde_json::from_value(serde_json::json!({
                "id": "p", "name": "项目", "path": cmd_dir, "start_command": "sleep 30"
            }))
            .unwrap()],
            ..Default::default()
        })
    }

    fn logs(st: &AppState) -> String {
        st.log_buffer("p").snapshot().into_iter().map(|l| l.text).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn successful_script_reports_duration_and_logs_output() {
        let st = state("/tmp");
        let out = run_script_blocking(&st, "p", "echo 编译中; echo done", "build").unwrap();
        assert_eq!(out.level, "success");
        assert!(out.message.contains("脚本「build」完成"), "{}", out.message);
        let l = logs(&st);
        assert!(l.contains("编译中") && l.contains("✔ 脚本「build」完成"), "{l}");
        assert!(st.jobs.lock().unwrap().is_empty(), "结束后任务记录被清掉");
    }

    #[test]
    fn failing_script_reports_reason_and_last_output() {
        let st = state("/tmp");
        let err = run_script_blocking(&st, "p", "echo '[ERROR] 依赖下载失败' >&2; exit 4", "install").unwrap_err();
        assert!(err.contains("脚本「install」失败") && err.contains("退出码 4"), "{err}");
        assert!(err.contains("依赖下载失败"), "{err}");
        assert!(st.jobs.lock().unwrap().is_empty());
    }

    #[test]
    fn script_uses_the_selected_runtime_and_project_env() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("devbox-script-env-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("bin")).unwrap();
        std::fs::write(d.join("bin/node"), "#!/bin/sh\necho 用的是假node-$MY_FLAG\n").unwrap();
        std::fs::set_permissions(d.join("bin/node"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let st = AppState::for_test(Config {
            projects: vec![serde_json::from_value(serde_json::json!({
                "id": "p", "name": "项目", "path": "/tmp", "start_command": "x",
                "env": {"MY_FLAG": "xyz"},
                "node": {"version": "99.0.0", "path": d.join("bin").to_string_lossy()},
            }))
            .unwrap()],
            ..Default::default()
        });
        run_script_blocking(&st, "p", "node -v", "版本").unwrap();
        assert!(logs(&st).contains("用的是假node-xyz"), "{}", logs(&st));
    }

    #[test]
    fn only_one_job_per_project_and_cancel_works() {
        let st = std::sync::Arc::new(state("/tmp"));
        let st2 = st.clone();
        let t = std::thread::spawn(move || run_script_blocking(&st2, "p", "sleep 30", "长任务"));
        // 等任务登记
        for _ in 0..50 {
            if !st.jobs.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let err = run_script_blocking(&st, "p", "echo hi", "第二个").unwrap_err();
        assert!(err.contains("已有脚本「长任务」在运行"), "{err}");

        cancel_job(&st, "p").unwrap();
        let out = t.join().unwrap().unwrap();
        assert_eq!(out.level, "warning");
        assert!(out.message.contains("已取消"), "{}", out.message);
        assert!(st.jobs.lock().unwrap().is_empty());
        assert!(cancel_job(&st, "p").is_err(), "没有任务可取消");
    }

    #[test]
    fn bad_inputs_are_rejected() {
        let st = state("/tmp");
        assert!(run_script_blocking(&st, "nope", "echo", "x").unwrap_err().contains("找不到项目"));
        assert!(run_script_blocking(&st, "p", "  ", "x").unwrap_err().contains("命令为空"));
        let st = state("/no/such/dir");
        assert!(run_script_blocking(&st, "p", "echo", "x").unwrap_err().contains("工作目录不存在"));
    }

    #[test]
    fn status_exposes_the_running_job() {
        let st = std::sync::Arc::new(state("/tmp"));
        let st2 = st.clone();
        let t = std::thread::spawn(move || run_script_blocking(&st2, "p", "sleep 30", "build"));
        for _ in 0..50 {
            if !st.jobs.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let s = crate::commands::process::compute_statuses(&st);
        assert_eq!(s[0].job.as_deref(), Some("build"));
        assert!(s[0].job_secs.is_some());
        cancel_job(&st, "p").unwrap();
        let _ = t.join();
        assert!(crate::commands::process::compute_statuses(&st)[0].job.is_none());
    }
}
