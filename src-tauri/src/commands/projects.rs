use crate::models::{DetectedProject, Project};
use crate::state::AppState;
use std::fs;
use std::path::Path;
use tauri::State;
use uuid::Uuid;

/// 列出所有项目
#[tauri::command]
pub fn list_projects(state: State<AppState>) -> Vec<Project> {
    state.config.lock().unwrap().projects.clone()
}

/// 新增或更新项目（按 id 判断；id 为空则新建）
#[tauri::command]
pub fn save_project(mut project: Project, state: State<AppState>) -> Result<Project, String> {
    if project.id.trim().is_empty() {
        project.id = Uuid::new_v4().to_string();
    }
    {
        let mut cfg = state.config.lock().unwrap();
        if let Some(existing) = cfg.projects.iter_mut().find(|p| p.id == project.id) {
            *existing = project.clone();
        } else {
            cfg.projects.push(project.clone());
        }
    }
    state.persist()?;
    Ok(project)
}

/// 按前端拖拽后的 id 顺序重排项目列表
/// 传入的 id 中不存在的会被忽略；未出现在 ids 里的项目保持原有相对顺序，追加在后面
#[tauri::command]
pub fn reorder_projects(ids: Vec<String>, state: State<AppState>) -> Result<Vec<Project>, String> {
    let projects = {
        let mut cfg = state.config.lock().unwrap();
        let mut rest = std::mem::take(&mut cfg.projects);
        let mut ordered: Vec<Project> = Vec::with_capacity(rest.len());
        for id in &ids {
            if let Some(i) = rest.iter().position(|p| &p.id == id) {
                ordered.push(rest.remove(i));
            }
        }
        ordered.extend(rest);
        cfg.projects = ordered;
        cfg.projects.clone()
    };
    state.persist()?;
    Ok(projects)
}

/// 删除项目
#[tauri::command]
pub fn delete_project(id: String, state: State<AppState>) -> Result<(), String> {
    // 先确保进程已停止
    let _ = crate::commands::process::stop_project_inner(&id, state.inner());
    {
        let mut cfg = state.config.lock().unwrap();
        cfg.projects.retain(|p| p.id != id);
    }
    state.persist()?;
    Ok(())
}

/// 弹出 macOS 原生「选择文件夹」对话框，返回所选目录的绝对路径
/// 用户取消时返回 Ok(None)
#[tauri::command]
pub fn pick_directory() -> Result<Option<String>, String> {
    let script = r#"POSIX path of (choose folder with prompt "选择项目工作目录")"#;
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|e| format!("无法打开选择器：{}", e))?;

    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if path.is_empty() {
            Ok(None)
        } else {
            Ok(Some(path))
        }
    } else {
        // 用户点击取消时 osascript 会以 -128 退出，视为无选择而非错误
        let err = String::from_utf8_lossy(&output.stderr);
        if err.contains("-128") || err.to_lowercase().contains("cancel") {
            Ok(None)
        } else {
            Err(err.trim().to_string())
        }
    }
}

/// 根据工作目录里的配置文件 / 文档，自动探测项目信息用于表单填充
#[tauri::command]
pub fn detect_project(path: String) -> Result<DetectedProject, String> {
    let dir = Path::new(&path);
    if !dir.is_dir() {
        return Err("目录不存在或不是文件夹".into());
    }

    let mut d = DetectedProject::default();
    // 默认名称取目录名
    if let Some(n) = dir.file_name().and_then(|s| s.to_str()) {
        d.name = Some(n.to_string());
    }
    let read = |f: &str| fs::read_to_string(dir.join(f)).ok();
    let mut hint = String::new();

    if let Some(pkg) = read("package.json") {
        hint = "package.json".into();
        d.kind = Some("frontend".into());
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&pkg) {
            if let Some(name) = json.get("name").and_then(|v| v.as_str()) {
                if !name.trim().is_empty() {
                    d.name = Some(name.to_string());
                }
            }
            // 选一个启动脚本
            let mut script: Option<String> = None;
            if let Some(scripts) = json.get("scripts").and_then(|v| v.as_object()) {
                for k in ["dev", "serve", "start", "dev:web", "dev:h5"] {
                    if scripts.contains_key(k) {
                        script = Some(k.to_string());
                        break;
                    }
                }
            }
            // 判定包管理器
            let pm = if dir.join("pnpm-lock.yaml").exists() {
                "pnpm"
            } else if dir.join("yarn.lock").exists() {
                "yarn"
            } else {
                "npm"
            };
            if let Some(sc) = script {
                d.start_command = Some(if pm == "npm" {
                    format!("npm run {}", sc)
                } else {
                    format!("{} {}", pm, sc)
                });
            }
            // 后端型 node（express/nest/koa）粗判
            let deps_has = |name: &str| -> bool {
                json.get("dependencies")
                    .and_then(|v| v.as_object())
                    .map(|o| o.contains_key(name))
                    .unwrap_or(false)
            };
            if deps_has("@nestjs/core") || deps_has("express") || deps_has("koa") {
                d.kind = Some("backend".into());
            }
        }
        // 从 vite / vue 配置里找端口
        for cfg in ["vite.config.js", "vite.config.ts", "vue.config.js"] {
            if let Some(c) = read(cfg) {
                if let Some(p) = extract_port(&c) {
                    d.port = Some(p);
                    break;
                }
            }
        }
        if let Some(p) = d.port {
            d.url = Some(format!("http://localhost:{}", p));
        }
    } else if let Some(pom) = read("pom.xml") {
        hint = "pom.xml".into();
        d.kind = Some("backend".into());
        if let Some(a) = extract_pom_artifact(&pom) {
            d.name = Some(a);
        }
        d.start_command = Some("mvn spring-boot:run".into());
        for cfg in [
            "src/main/resources/application.yml",
            "src/main/resources/application.yaml",
            "src/main/resources/application.properties",
        ] {
            if let Some(c) = read(cfg) {
                if let Some(p) = extract_port(&c) {
                    d.port = Some(p);
                    break;
                }
            }
        }
        if let Some(p) = d.port {
            d.url = Some(format!("http://localhost:{}", p));
        }
    } else if let Some(cargo) = read("Cargo.toml") {
        hint = "Cargo.toml".into();
        d.kind = Some("backend".into());
        if let Some(n) = extract_toml_name(&cargo) {
            d.name = Some(n);
        }
        d.start_command = Some("cargo run".into());
    } else if read("go.mod").is_some() {
        hint = "go.mod".into();
        d.kind = Some("backend".into());
        d.start_command = Some("go run .".into());
    } else if read("pyproject.toml").is_some() || read("requirements.txt").is_some() {
        hint = "Python 项目".into();
        d.kind = Some("backend".into());
        if dir.join("manage.py").exists() {
            d.start_command = Some("python manage.py runserver".into());
            d.port = Some(8000);
            d.url = Some("http://localhost:8000".into());
        }
    }

    d.summary = if hint.is_empty() {
        "未识别到已知项目类型，请手动填写".into()
    } else {
        format!("已根据 {} 自动填充，可再手动调整", hint)
    };
    Ok(d)
}

/// 从文本里粗略提取端口号：匹配包含 "port" 的行后面的第一个数字
fn extract_port(s: &str) -> Option<u16> {
    for line in s.lines() {
        let l = line.trim();
        if l.starts_with("//") || l.starts_with('#') || l.starts_with('*') {
            continue;
        }
        let lower = l.to_lowercase();
        if let Some(idx) = lower.find("port") {
            let rest = &l[idx + 4..];
            let digits: String = rest
                .chars()
                .skip_while(|c| !c.is_ascii_digit())
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(p) = digits.parse::<u16>() {
                if p > 0 {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// 提取 XML 标签内容
fn extract_tag(s: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let i = s.find(&open)?;
    let start = i + open.len();
    let j = s[start..].find(&close)?;
    Some(s[start..start + j].trim().to_string())
}

/// 提取 pom.xml 项目自身的 artifactId（跳过 <parent> 块）
fn extract_pom_artifact(s: &str) -> Option<String> {
    let from = s.find("</parent>").map(|i| i + "</parent>".len()).unwrap_or(0);
    extract_tag(&s[from..], "artifactId")
}

/// 提取 Cargo.toml 中 [package] 的 name
fn extract_toml_name(s: &str) -> Option<String> {
    let mut in_pkg = false;
    for line in s.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_pkg = l == "[package]";
            continue;
        }
        if in_pkg && l.starts_with("name") {
            if let Some(eq) = l.find('=') {
                let v = l[eq + 1..].trim().trim_matches('"').trim().to_string();
                if !v.is_empty() {
                    return Some(v);
                }
            }
        }
    }
    None
}
