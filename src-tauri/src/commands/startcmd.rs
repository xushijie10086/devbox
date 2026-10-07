//! 自动获取启动命令：读项目目录里的配置文件，列出能用来启动项目的命令供选择。
//!
//! 只读文件、不执行任何东西。排在最前面的是推荐项；同一个目录可能同时有多种技术栈
//! （如前端 + docker-compose），所以返回的是候选列表而不是单个命令。

use serde::Serialize;
use std::fs;
use std::path::Path;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct StartCandidate {
    pub command: String,
    /// 展示名，如 "dev"、"Spring Boot（子模块 admin）"
    pub label: String,
    /// 来源，如 "package.json"、"pom.xml"
    pub source: String,
    /// 一句话说明或提醒，如脚本原文、「尚未安装依赖」
    pub note: Option<String>,
}

fn cand(command: impl Into<String>, label: impl Into<String>, source: &str, note: Option<String>) -> StartCandidate {
    StartCandidate { command: command.into(), label: label.into(), source: source.into(), note }
}

fn rd(dir: &Path, f: &str) -> Option<String> {
    fs::read_to_string(dir.join(f)).ok()
}

/// 一级子目录名（排序，跳过隐藏目录和依赖目录）
fn subdirs(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.') && n != "node_modules" && n != "target")
        .collect();
    v.sort();
    v
}

/// 目录里能识别出的启动命令，推荐项在前，按命令去重
pub fn start_candidates(dir: &Path) -> Vec<StartCandidate> {
    let mut out = Vec::new();
    node(dir, &mut out);
    maven(dir, &mut out);
    gradle(dir, &mut out);
    cargo(dir, &mut out);
    go(dir, &mut out);
    python(dir, &mut out);
    ruby_php_deno(dir, &mut out);
    compose(dir, &mut out);
    procfile(dir, &mut out);
    makefile(dir, &mut out);

    let mut seen = std::collections::HashSet::new();
    out.retain(|c| seen.insert(c.command.clone()));
    out
}

#[tauri::command(async)]
pub fn detect_start_commands(path: String) -> Result<Vec<StartCandidate>, String> {
    let dir = Path::new(&path);
    if !dir.is_dir() {
        return Err("目录不存在或不是文件夹".into());
    }
    Ok(start_candidates(dir))
}

// ---------------------------------------------------------------- Node

fn package_manager(dir: &Path) -> &'static str {
    if dir.join("pnpm-lock.yaml").exists() {
        "pnpm"
    } else if dir.join("yarn.lock").exists() {
        "yarn"
    } else if dir.join("bun.lockb").exists() || dir.join("bun.lock").exists() {
        "bun"
    } else {
        "npm"
    }
}

/// 看起来是「启动开发服务」的脚本名
fn is_start_like(k: &str) -> bool {
    matches!(k, "dev" | "serve" | "start" | "develop" | "preview")
        || ["dev:", "dev-", "start:", "start-", "serve:", "serve-"].iter().any(|p| k.starts_with(p))
}

fn node(dir: &Path, out: &mut Vec<StartCandidate>) {
    let Some(pkg) = rd(dir, "package.json").and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else {
        return;
    };
    let pm = package_manager(dir);
    let run = |s: &str| match pm {
        "npm" | "bun" => format!("{pm} run {s}"),
        _ => format!("{pm} {s}"),
    };
    let no_modules = !dir.join("node_modules").exists();
    let warn = "尚未安装依赖（没有 node_modules），请先安装";
    let join = |body: Option<String>| -> Option<String> {
        let parts: Vec<String> = body.into_iter().chain(no_modules.then(|| warn.to_string())).collect();
        (!parts.is_empty()).then(|| parts.join("；"))
    };
    let before = out.len();

    if let Some(scripts) = pkg.get("scripts").and_then(|v| v.as_object()) {
        // Tauri 项目：`dev` 脚本一般只起前端，真正启动桌面应用要用 tauri dev
        if scripts.contains_key("tauri") && dir.join("src-tauri").is_dir() {
            out.push(cand(run("tauri dev"), "tauri dev", "package.json", join(Some("启动 Tauri 桌面应用（含前端）".into()))));
        }
        let rank = |k: &str| ["dev", "serve", "start", "develop"].iter().position(|c| *c == k).unwrap_or(10);
        let mut names: Vec<&String> = scripts.keys().filter(|k| is_start_like(k)).collect();
        names.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.cmp(b)));
        for k in names.into_iter().take(8) {
            let body = scripts[k].as_str().map(|s| s.chars().take(80).collect::<String>());
            out.push(cand(run(k), k.as_str(), "package.json", join(body)));
        }
    }
    // 没有启动类脚本：退回到入口文件
    if out.len() == before {
        let main = pkg.get("main").and_then(|v| v.as_str()).filter(|m| dir.join(m).is_file()).map(|m| m.to_string());
        let entry = main.or_else(|| {
            ["server.js", "index.js", "app.js", "main.js"].iter().find(|f| dir.join(f).is_file()).map(|f| f.to_string())
        });
        if let Some(e) = entry {
            out.push(cand(format!("node {e}"), e.as_str(), "package.json", join(Some("没有 dev / start 脚本，直接运行入口文件".into()))));
        }
    }
}

// ---------------------------------------------------------------- Maven

/// 读出 pom 里直接声明的子模块
pub(crate) fn maven_modules(pom: &str) -> Vec<String> {
    let mut modules = Vec::new();
    let mut rest = pom;
    while let Some(i) = rest.find("<module>") {
        let after = &rest[i + "<module>".len()..];
        let Some(j) = after.find("</module>") else { break };
        modules.push(after[..j].trim().to_string());
        rest = &after[j..];
    }
    modules
}

fn maven(dir: &Path, out: &mut Vec<StartCandidate>) {
    let Some(pom) = rd(dir, "pom.xml") else { return };
    let mvn = if dir.join("mvnw").exists() { "./mvnw" } else { "mvn" };
    let modules = maven_modules(&pom);
    let boot = |p: &str| p.contains("spring-boot-maven-plugin");

    // 单模块，或根 pom 自带 spring-boot 插件：直接在根目录启动
    if modules.is_empty() || boot(&pom) {
        let (goal, label, note) = if boot(&pom) {
            ("spring-boot:run", "Spring Boot", None)
        } else if pom.contains("quarkus-maven-plugin") {
            ("quarkus:dev", "Quarkus", None)
        } else if pom.contains("jetty-maven-plugin") {
            ("jetty:run", "Jetty", None)
        } else if pom.contains("exec-maven-plugin") {
            ("exec:java", "exec:java", None)
        } else {
            ("spring-boot:run", "Spring Boot", Some("pom.xml 里没找到启动插件（spring-boot-maven-plugin），请确认启动方式".to_string()))
        };
        out.push(cand(format!("{mvn} {goal}"), label, "pom.xml", note));
        return;
    }

    // 多模块：根目录没有插件，直接 spring-boot:run 会报 No plugin found for prefix 'spring-boot'，
    // 要指定带插件的子模块；它依赖的兄弟模块必须已经安装到本地仓库
    let boots: Vec<&String> = modules.iter().filter(|m| rd(&dir.join(m), "pom.xml").is_some_and(|p| boot(&p))).collect();
    if boots.is_empty() {
        out.push(cand(
            format!("{mvn} spring-boot:run"),
            "Spring Boot",
            "pom.xml",
            Some("多模块项目，根目录和子模块都没找到 spring-boot 插件，请把工作目录改为要启动的子模块".into()),
        ));
    }
    for m in boots {
        out.push(cand(
            format!("{mvn} -pl {m} -am install -DskipTests && {mvn} -pl {m} spring-boot:run"),
            format!("Spring Boot · 子模块 {m}"),
            "pom.xml",
            Some("先安装它依赖的模块再启动，首次或依赖有改动时用；每次都会多一步构建，较慢".into()),
        ));
        out.push(cand(
            format!("{mvn} -pl {m} spring-boot:run"),
            format!("Spring Boot · 子模块 {m}（跳过安装）"),
            "pom.xml",
            Some("依赖模块已安装过时用，启动更快；没装过会报 Could not resolve dependencies".into()),
        ));
    }
}

// ---------------------------------------------------------------- Gradle

fn gradle_run_task(script: &str) -> Option<&'static str> {
    if script.contains("org.springframework.boot") && !script.contains("apply false") {
        Some("bootRun")
    } else if script.contains("quarkus") {
        Some("quarkusDev")
    } else if script.contains("'application'")
        || script.contains("\"application\"")
        || script.lines().any(|l| l.trim() == "application")
    {
        Some("run")
    } else {
        None
    }
}

fn gradle_script(dir: &Path) -> Option<String> {
    rd(dir, "build.gradle").or_else(|| rd(dir, "build.gradle.kts"))
}

fn gradle(dir: &Path, out: &mut Vec<StartCandidate>) {
    let root = gradle_script(dir);
    let has_settings = dir.join("settings.gradle").exists() || dir.join("settings.gradle.kts").exists();
    if root.is_none() && !has_settings {
        return;
    }
    let g = if dir.join("gradlew").exists() { "./gradlew" } else { "gradle" };
    if let Some(task) = root.as_deref().and_then(gradle_run_task) {
        out.push(cand(format!("{g} {task}"), task, "Gradle", None));
    }
    for sub in subdirs(dir) {
        if let Some(task) = gradle_script(&dir.join(&sub)).as_deref().and_then(gradle_run_task) {
            out.push(cand(format!("{g} :{sub}:{task}"), format!("{task} · 子项目 {sub}"), "Gradle", None));
        }
    }
}

// ---------------------------------------------------------------- Rust / Go

fn cargo(dir: &Path, out: &mut Vec<StartCandidate>) {
    let Some(toml) = rd(dir, "Cargo.toml") else { return };
    // 纯 workspace 根目录没有可运行的包
    if toml.contains("[workspace]") && !toml.contains("[package]") {
        out.push(cand(
            "cargo run",
            "cargo run",
            "Cargo.toml",
            Some("这是 workspace 根目录，请改成 cargo run -p <成员名>".into()),
        ));
    } else {
        out.push(cand("cargo run", "cargo run", "Cargo.toml", None));
    }
}

fn has_go_main(dir: &Path) -> bool {
    fs::read_dir(dir).into_iter().flatten().flatten().any(|e| {
        let name = e.file_name().to_string_lossy().to_string();
        name.ends_with(".go")
            && !name.ends_with("_test.go")
            && fs::read_to_string(e.path()).is_ok_and(|t| t.lines().any(|l| l.trim() == "package main"))
    })
}

fn go(dir: &Path, out: &mut Vec<StartCandidate>) {
    if !dir.join("go.mod").exists() {
        return;
    }
    if dir.join(".air.toml").exists() {
        out.push(cand("air", "air（热重载）", ".air.toml", None));
    }
    let before = out.len();
    if has_go_main(dir) {
        out.push(cand("go run .", "go run .", "go.mod", None));
    }
    let cmd = dir.join("cmd");
    for sub in subdirs(&cmd) {
        if has_go_main(&cmd.join(&sub)) {
            out.push(cand(format!("go run ./cmd/{sub}"), format!("cmd/{sub}"), "go.mod", None));
        }
    }
    if out.len() == before {
        out.push(cand("go run .", "go run .", "go.mod", Some("没找到 package main，请确认入口目录".into())));
    }
}

// ---------------------------------------------------------------- Python

fn python(dir: &Path, out: &mut Vec<StartCandidate>) {
    let req = rd(dir, "requirements.txt").unwrap_or_default();
    let pyproject = rd(dir, "pyproject.toml").unwrap_or_default();
    let pipfile = rd(dir, "Pipfile").unwrap_or_default();
    let manage = dir.join("manage.py").exists();
    if req.is_empty() && pyproject.is_empty() && pipfile.is_empty() && !manage {
        return;
    }
    let deps = format!("{req}\n{pyproject}\n{pipfile}").to_lowercase();

    // 解释器：优先用项目自己的环境，免得用到系统 Python 缺依赖
    let py: String = if pyproject.contains("[tool.poetry]") || dir.join("poetry.lock").exists() {
        "poetry run python".into()
    } else if dir.join("Pipfile").exists() {
        "pipenv run python".into()
    } else if dir.join("uv.lock").exists() {
        "uv run python".into()
    } else if dir.join(".venv/bin/python").exists() {
        ".venv/bin/python".into()
    } else if dir.join("venv/bin/python").exists() {
        "venv/bin/python".into()
    } else {
        "python3".into()
    };

    let before = out.len();
    if manage {
        out.push(cand(format!("{py} manage.py runserver"), "Django", "manage.py", None));
    }
    if deps.contains("fastapi") || deps.contains("uvicorn") {
        let module = [("main.py", "main"), ("app/main.py", "app.main"), ("src/main.py", "src.main"), ("app.py", "app")]
            .iter()
            .find(|(f, _)| dir.join(f).is_file())
            .map(|(_, m)| *m);
        if let Some(m) = module {
            out.push(cand(
                format!("{py} -m uvicorn {m}:app --reload"),
                "FastAPI (uvicorn)",
                "Python",
                Some("假定应用对象叫 app，不是的话请改命令里冒号后面的名字".into()),
            ));
        }
    }
    if deps.contains("flask") && (dir.join("app.py").is_file() || dir.join("wsgi.py").is_file()) {
        out.push(cand(format!("{py} -m flask run --debug"), "Flask", "Python", None));
    }
    // 没认出框架：退回到常见入口脚本
    if out.len() == before {
        if let Some(f) = ["main.py", "app.py", "run.py", "server.py"].iter().find(|f| dir.join(f).is_file()) {
            out.push(cand(format!("{py} {f}"), *f, "Python", None));
        }
    }
}

// ---------------------------------------------------------------- 其它

fn ruby_php_deno(dir: &Path, out: &mut Vec<StartCandidate>) {
    if dir.join("Gemfile").exists() && dir.join("bin/rails").exists() {
        out.push(cand("bin/rails server", "Rails", "Gemfile", None));
    }
    if dir.join("artisan").exists() {
        out.push(cand("php artisan serve", "Laravel", "artisan", None));
    }
    if let Some(tasks) = rd(dir, "deno.json")
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|j| j.get("tasks").and_then(|t| t.as_object()).cloned())
    {
        for k in ["dev", "start", "serve"] {
            if tasks.contains_key(k) {
                out.push(cand(format!("deno task {k}"), k, "deno.json", None));
            }
        }
    }
}

fn compose(dir: &Path, out: &mut Vec<StartCandidate>) {
    for f in ["docker-compose.yml", "docker-compose.yaml", "compose.yml", "compose.yaml"] {
        if dir.join(f).exists() {
            out.push(cand("docker compose up", "docker compose up", f, Some("启动 compose 里的全部服务".into())));
            return;
        }
    }
}

fn procfile(dir: &Path, out: &mut Vec<StartCandidate>) {
    let Some(text) = rd(dir, "Procfile") else { return };
    for line in text.lines().filter_map(|l| l.split_once(':')).take(4) {
        let (name, cmd) = (line.0.trim(), line.1.trim());
        if !cmd.is_empty() && !matches!(name, "release" | "postdeploy") {
            out.push(cand(cmd, format!("Procfile · {name}"), "Procfile", None));
        }
    }
}

fn makefile(dir: &Path, out: &mut Vec<StartCandidate>) {
    let Some(mk) = rd(dir, "Makefile") else { return };
    for t in ["run", "dev", "start", "serve", "up", "watch"] {
        if mk.lines().any(|l| l.split_once(':').is_some_and(|(a, b)| a == t && !b.starts_with('='))) {
            out.push(cand(format!("make {t}"), format!("make {t}"), "Makefile", None));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("devbox-startcmd-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        for (f, c) in files {
            let p = d.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, c).unwrap();
        }
        d
    }

    fn cmds(d: &Path) -> Vec<String> {
        start_candidates(d).into_iter().map(|c| c.command).collect()
    }

    const BOOT: &str = "<project><build><plugins><plugin><artifactId>spring-boot-maven-plugin</artifactId></plugin></plugins></build></project>";

    #[test]
    fn node_scripts_ranked_and_use_detected_package_manager() {
        let pkg = r#"{"scripts":{"build":"vite build","start":"node s.js","dev":"vite","dev:admin":"vite --mode admin","test":"jest","predev":"x"}}"#;
        let d = dir("node-yarn", &[("package.json", pkg), ("yarn.lock", "")]);
        assert_eq!(cmds(&d), ["yarn dev", "yarn start", "yarn dev:admin"], "只列启动类脚本，dev 在前，不含 build/test/predev");
        let d = dir("node-npm", &[("package.json", pkg)]);
        assert_eq!(cmds(&d)[0], "npm run dev");
        let d = dir("node-pnpm", &[("package.json", pkg), ("pnpm-lock.yaml", "")]);
        assert_eq!(cmds(&d)[0], "pnpm dev");
        let d = dir("node-bun", &[("package.json", pkg), ("bun.lockb", "")]);
        assert_eq!(cmds(&d)[0], "bun run dev");
    }

    #[test]
    fn node_warns_when_dependencies_missing_and_shows_script_body() {
        let d = dir("node-note", &[("package.json", r#"{"scripts":{"dev":"vite --host"}}"#)]);
        let note = start_candidates(&d)[0].note.clone().unwrap();
        assert!(note.contains("vite --host") && note.contains("node_modules"), "{note}");
        fs::create_dir_all(d.join("node_modules")).unwrap();
        assert_eq!(start_candidates(&d)[0].note.as_deref(), Some("vite --host"), "装了依赖就不再提醒");
    }

    #[test]
    fn tauri_project_prefers_tauri_dev() {
        let d = dir("tauri", &[
            ("package.json", r#"{"scripts":{"dev":"vite","tauri":"tauri"}}"#),
            ("src-tauri/Cargo.toml", "[package]\nname=\"x\""),
        ]);
        assert_eq!(cmds(&d)[0], "npm run tauri dev");
        assert!(cmds(&d).contains(&"npm run dev".to_string()));
    }

    #[test]
    fn node_without_scripts_falls_back_to_entry_file() {
        let d = dir("node-entry", &[("package.json", r#"{"main":"lib/boot.js"}"#), ("lib/boot.js", "")]);
        assert_eq!(cmds(&d), ["node lib/boot.js"]);
        let d = dir("node-server", &[("package.json", "{}"), ("server.js", "")]);
        assert_eq!(cmds(&d), ["node server.js"]);
        let d = dir("node-none", &[("package.json", r#"{"scripts":{"build":"x"}}"#)]);
        assert!(cmds(&d).is_empty());
    }

    #[test]
    fn maven_single_module_and_wrapper() {
        let d = dir("mvn1", &[("pom.xml", BOOT)]);
        assert_eq!(cmds(&d), ["mvn spring-boot:run"]);
        let d = dir("mvn-w", &[("pom.xml", BOOT), ("mvnw", "")]);
        assert_eq!(cmds(&d), ["./mvnw spring-boot:run"]);
        let d = dir("mvn-q", &[("pom.xml", "<project><plugin>quarkus-maven-plugin</plugin></project>")]);
        assert_eq!(cmds(&d), ["mvn quarkus:dev"]);
        let d = dir("mvn-unknown", &[("pom.xml", "<project/>")]);
        assert!(start_candidates(&d)[0].note.as_ref().unwrap().contains("没找到启动插件"));
    }

    #[test]
    fn maven_multi_module_targets_the_boot_submodule() {
        let root = "<project><modules><module>core</module><module>admin</module></modules></project>";
        let d = dir("mvn-mm", &[("pom.xml", root), ("core/pom.xml", "<project/>"), ("admin/pom.xml", BOOT)]);
        assert_eq!(
            cmds(&d),
            [
                "mvn -pl admin -am install -DskipTests && mvn -pl admin spring-boot:run",
                "mvn -pl admin spring-boot:run"
            ],
            "推荐「先安装依赖再启动」，首次一定能跑；根目录不能直接 spring-boot:run"
        );
        // 没有任何子模块带插件：给出提示而不是装作能启动
        let d = dir("mvn-mm0", &[("pom.xml", root), ("core/pom.xml", "<project/>"), ("admin/pom.xml", "<project/>")]);
        let c = start_candidates(&d);
        assert_eq!(c.len(), 1);
        assert!(c[0].note.as_ref().unwrap().contains("子模块"));
        // 根 pom 自带插件：直接在根目录启动
        let both = format!("<project><modules><module>a</module></modules>{BOOT}</project>");
        let d = dir("mvn-mmroot", &[("pom.xml", &both)]);
        assert_eq!(cmds(&d), ["mvn spring-boot:run"]);
    }

    #[test]
    fn gradle_tasks() {
        let d = dir("g-boot", &[("build.gradle", "plugins { id 'org.springframework.boot' version '3.2.0' }"), ("gradlew", "")]);
        assert_eq!(cmds(&d), ["./gradlew bootRun"]);
        let d = dir("g-app", &[("build.gradle.kts", "plugins {\n    application\n}\n")]);
        assert_eq!(cmds(&d), ["gradle run"]);
        // 根项目只声明插件不应用：找带插件的子项目
        let d = dir("g-multi", &[
            ("settings.gradle", "include 'api'"),
            ("build.gradle", "plugins { id 'org.springframework.boot' version '3' apply false }"),
            ("api/build.gradle", "plugins { id 'org.springframework.boot' }"),
            ("lib/build.gradle", "plugins { id 'java-library' }"),
        ]);
        assert_eq!(cmds(&d), ["gradle :api:bootRun"]);
    }

    #[test]
    fn go_entry_points() {
        let d = dir("go-root", &[("go.mod", "module x"), ("main.go", "package main\n\nfunc main() {}\n")]);
        assert_eq!(cmds(&d), ["go run ."]);
        let d = dir("go-cmd", &[
            ("go.mod", "module x"),
            ("cmd/server/main.go", "package main"),
            ("cmd/worker/main.go", "package main"),
            ("cmd/lib/x.go", "package lib"),
            ("main_test.go", "package main"),
        ]);
        assert_eq!(cmds(&d), ["go run ./cmd/server", "go run ./cmd/worker"], "测试文件和非 main 包不算入口");
        let d = dir("go-air", &[("go.mod", "module x"), (".air.toml", ""), ("main.go", "package main")]);
        assert_eq!(cmds(&d), ["air", "go run ."]);
    }

    #[test]
    fn python_frameworks_and_environments() {
        let d = dir("py-django", &[("requirements.txt", "django"), ("manage.py", "")]);
        assert_eq!(cmds(&d), ["python3 manage.py runserver"]);
        let d = dir("py-fastapi", &[("requirements.txt", "fastapi\nuvicorn"), ("app/main.py", ""), (".venv/bin/python", "")]);
        assert_eq!(cmds(&d), [".venv/bin/python -m uvicorn app.main:app --reload"], "优先用项目自己的虚拟环境");
        let d = dir("py-poetry", &[("pyproject.toml", "[tool.poetry]\nname='x'\n[tool.poetry.dependencies]\nflask='*'"), ("app.py", "")]);
        assert_eq!(cmds(&d), ["poetry run python -m flask run --debug"]);
        let d = dir("py-script", &[("requirements.txt", "requests"), ("main.py", "")]);
        assert_eq!(cmds(&d), ["python3 main.py"]);
        let d = dir("py-none", &[("requirements.txt", "requests")]);
        assert!(cmds(&d).is_empty());
    }

    #[test]
    fn misc_stacks() {
        let d = dir("misc", &[
            ("docker-compose.yml", ""),
            ("Procfile", "web: gunicorn app:app\nrelease: ./migrate.sh\nworker: python w.py\n"),
            ("Makefile", "build:\n\tgo build\nrun:\n\t./x\nVERSION := 1\n"),
            ("artisan", ""),
        ]);
        assert_eq!(
            cmds(&d),
            ["php artisan serve", "docker compose up", "gunicorn app:app", "python w.py", "make run"],
            "release 不是启动命令；Makefile 只取 run/dev 这类目标"
        );
    }

    #[test]
    fn this_repository_is_recognised_as_a_tauri_app() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let c = start_candidates(root);
        assert_eq!(c[0].command, "npm run tauri dev", "{c:?}");
        assert!(c.iter().any(|x| x.command == "npm run dev"));
    }

    #[test]
    fn candidates_are_deduplicated_and_empty_dirs_yield_nothing() {
        let d = dir("dedupe", &[("Procfile", "web: make run\n"), ("Makefile", "run:\n\t./x\n")]);
        assert_eq!(cmds(&d), ["make run"]);
        let d = dir("empty", &[]);
        assert!(start_candidates(&d).is_empty());
        assert!(detect_start_commands("/definitely/not/a/dir".into()).is_err());
    }
}
