use crate::models::{Config, DetectedProject, Project};
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

// ---------- 项目组 ----------

/// 前端用来表示「全部」「未分组」两个固定 tab 的名字，不能被用作项目组名
const RESERVED_GROUPS: [&str; 2] = ["全部", "未分组"];
const MAX_GROUP_NAME: usize = 30;

/// 校验并规整项目组名
fn clean_group_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("分组名称不能为空".into());
    }
    if name.chars().count() > MAX_GROUP_NAME {
        return Err(format!("分组名称最多 {MAX_GROUP_NAME} 个字符"));
    }
    if RESERVED_GROUPS.contains(&name) {
        return Err(format!("「{name}」是系统保留名称，请换一个"));
    }
    Ok(name.to_string())
}

fn add_group(cfg: &mut Config, raw: &str) -> Result<String, String> {
    let name = clean_group_name(raw)?;
    if cfg.project_groups.contains(&name) {
        return Err(format!("分组「{name}」已存在"));
    }
    cfg.project_groups.push(name.clone());
    Ok(name)
}

fn rename_group(cfg: &mut Config, old: &str, raw_new: &str) -> Result<String, String> {
    let new = clean_group_name(raw_new)?;
    let idx = cfg
        .project_groups
        .iter()
        .position(|g| g == old)
        .ok_or_else(|| format!("找不到分组「{old}」"))?;
    if new != old && cfg.project_groups.contains(&new) {
        return Err(format!("分组「{new}」已存在"));
    }
    cfg.project_groups[idx] = new.clone();
    for p in cfg.projects.iter_mut().filter(|p| p.group.as_deref() == Some(old)) {
        p.group = Some(new.clone());
    }
    Ok(new)
}

/// 删除分组：组内项目不会被删除，回到「未分组」
fn delete_group(cfg: &mut Config, name: &str) -> Result<usize, String> {
    let before = cfg.project_groups.len();
    cfg.project_groups.retain(|g| g != name);
    if cfg.project_groups.len() == before {
        return Err(format!("找不到分组「{name}」"));
    }
    let mut moved = 0;
    for p in cfg.projects.iter_mut().filter(|p| p.group.as_deref() == Some(name)) {
        p.group = None;
        moved += 1;
    }
    Ok(moved)
}

/// 保存项目前规整它的分组：空串视为未分组；指向尚不存在的分组时自动登记
fn normalize_project_group(cfg: &mut Config, project: &mut Project) {
    project.group = project
        .group
        .take()
        .map(|g| g.trim().to_string())
        .filter(|g| !g.is_empty() && !RESERVED_GROUPS.contains(&g.as_str()));
    if let Some(g) = &project.group {
        if !cfg.project_groups.contains(g) {
            cfg.project_groups.push(g.clone());
        }
    }
}

/// 列出项目组（tab 顺序）
#[tauri::command]
pub fn list_project_groups(state: State<AppState>) -> Vec<String> {
    state.config.lock().unwrap().project_groups.clone()
}

/// 新增项目组，返回规整后的名称
#[tauri::command]
pub fn add_project_group(name: String, state: State<AppState>) -> Result<String, String> {
    let added = add_group(&mut state.config.lock().unwrap(), &name)?;
    state.persist()?;
    Ok(added)
}

/// 重命名项目组，组内项目同步改名
#[tauri::command]
pub fn rename_project_group(
    old_name: String,
    new_name: String,
    state: State<AppState>,
) -> Result<String, String> {
    let renamed = rename_group(&mut state.config.lock().unwrap(), &old_name, &new_name)?;
    state.persist()?;
    Ok(renamed)
}

/// 删除项目组；组内项目回到「未分组」，返回受影响的项目数
#[tauri::command]
pub fn delete_project_group(name: String, state: State<AppState>) -> Result<usize, String> {
    let moved = delete_group(&mut state.config.lock().unwrap(), &name)?;
    state.persist()?;
    Ok(moved)
}

/// 新增或更新项目（按 id 判断；id 为空则新建）
#[tauri::command]
pub fn save_project(mut project: Project, state: State<AppState>) -> Result<Project, String> {
    if project.id.trim().is_empty() {
        project.id = Uuid::new_v4().to_string();
    }
    {
        let mut cfg = state.config.lock().unwrap();
        normalize_project_group(&mut cfg, &mut project);
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
    let _ = crate::commands::scripts::cancel_job(state.inner(), &id);
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
    detect_in(Path::new(&path), &crate::commands::runtime::installed())
}

/// 识别项目信息；installed 是本机已安装的 Node / JDK，用来匹配项目声明的版本要求
fn detect_in(dir: &Path, installed: &crate::commands::runtime::Runtimes) -> Result<DetectedProject, String> {
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

    // 项目声明的 Node / JDK 版本，并在本机已安装的版本里匹配
    d.node = crate::commands::runtime::suggest_node(dir, &installed.node);
    d.java = crate::commands::runtime::suggest_java(dir, &installed.java);
    d.notes = maven_notes(dir);

    d.summary = if hint.is_empty() {
        "未识别到已知项目类型，请手动填写".into()
    } else {
        format!("已根据 {} 自动填充，可再手动调整", hint)
    };
    Ok(d)
}

/// 多模块 Maven 项目的提醒：根目录一般没有 spring-boot 插件，直接 `mvn spring-boot:run`
/// 会报 "No plugin found for prefix 'spring-boot'"，应该在带插件的子模块里启动
fn maven_notes(dir: &Path) -> Vec<String> {
    let Ok(pom) = fs::read_to_string(dir.join("pom.xml")) else { return vec![] };
    if !pom.contains("<modules>") || pom.contains("spring-boot-maven-plugin") {
        return vec![];
    }
    let mut modules = Vec::new();
    let mut rest = pom.as_str();
    while let Some(i) = rest.find("<module>") {
        let after = &rest[i + "<module>".len()..];
        let Some(j) = after.find("</module>") else { break };
        modules.push(after[..j].trim().to_string());
        rest = &after[j..];
    }
    let boot: Vec<&String> = modules
        .iter()
        .filter(|m| {
            fs::read_to_string(dir.join(m).join("pom.xml")).map_or(false, |p| p.contains("spring-boot-maven-plugin"))
        })
        .collect();
    let head = "这是多模块 Maven 项目，根目录没有 spring-boot 插件，直接 mvn spring-boot:run 会报 No plugin found for prefix 'spring-boot'。";
    match boot.as_slice() {
        [m] => vec![format!(
            "{head}建议先在根目录执行一次 mvn install -DskipTests，再把工作目录改为 {}/{m} 启动",
            dir.display()
        )],
        [] => vec![format!("{head}请把工作目录改为带 spring-boot 插件的子模块，或改用 java -jar 运行打包产物")],
        many => vec![format!(
            "{head}带 spring-boot 插件的子模块有：{}；请把工作目录改为要启动的那个",
            many.iter().map(|m| m.as_str()).collect::<Vec<_>>().join("、")
        )],
    }
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

#[cfg(test)]
mod group_tests {
    use super::*;

    fn project(id: &str, group: Option<&str>) -> Project {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "path": "/tmp", "start_command": "x", "group": group
        }))
        .unwrap()
    }

    fn cfg() -> Config {
        Config {
            projects: vec![project("a", Some("后端")), project("b", Some("后端")), project("c", None)],
            project_groups: vec!["后端".into(), "前端".into()],
            ..Default::default()
        }
    }

    #[test]
    fn add_validates_name() {
        let mut c = cfg();
        assert_eq!(add_group(&mut c, "  工具 ").unwrap(), "工具");
        assert!(add_group(&mut c, "工具").is_err(), "重复");
        assert!(add_group(&mut c, "   ").is_err(), "空");
        assert!(add_group(&mut c, "全部").is_err(), "保留名");
        assert!(add_group(&mut c, "未分组").is_err(), "保留名");
        assert!(add_group(&mut c, &"长".repeat(31)).is_err(), "过长");
        assert_eq!(c.project_groups, vec!["后端", "前端", "工具"]);
    }

    #[test]
    fn rename_updates_projects_and_keeps_order() {
        let mut c = cfg();
        rename_group(&mut c, "后端", "服务端").unwrap();
        assert_eq!(c.project_groups, vec!["服务端", "前端"]);
        assert_eq!(c.projects[0].group.as_deref(), Some("服务端"));
        assert_eq!(c.projects[1].group.as_deref(), Some("服务端"));
        assert_eq!(c.projects[2].group, None);
        assert!(rename_group(&mut c, "服务端", "前端").is_err(), "撞名");
        assert!(rename_group(&mut c, "不存在", "x").is_err());
    }

    #[test]
    fn delete_moves_projects_to_ungrouped() {
        let mut c = cfg();
        assert_eq!(delete_group(&mut c, "后端").unwrap(), 2);
        assert_eq!(c.project_groups, vec!["前端"]);
        assert!(c.projects.iter().all(|p| p.group.is_none()));
        assert_eq!(c.projects.len(), 3, "项目本身不会被删除");
        assert!(delete_group(&mut c, "后端").is_err());
    }

    #[test]
    fn saving_project_registers_unknown_group_and_cleans_blank() {
        let mut c = cfg();
        let mut p = project("d", Some(" 新组 "));
        normalize_project_group(&mut c, &mut p);
        assert_eq!(p.group.as_deref(), Some("新组"));
        assert!(c.project_groups.contains(&"新组".to_string()));

        let mut blank = project("e", Some("  "));
        normalize_project_group(&mut c, &mut blank);
        assert_eq!(blank.group, None);

        let mut reserved = project("f", Some("全部"));
        normalize_project_group(&mut c, &mut reserved);
        assert_eq!(reserved.group, None);
    }

    #[test]
    fn old_config_loads_and_ignores_removed_profiles() {
        let c: Config = serde_json::from_str(
            r#"{"projects":[{"id":"x","name":"x","path":"/","start_command":"y"}],"profiles":[{"id":"p","name":"旧启动组"}]}"#,
        )
        .unwrap();
        assert!(c.project_groups.is_empty());
        assert_eq!(c.projects[0].group, None);
    }
}

#[cfg(test)]
mod detect_runtime_tests {
    use super::*;
    use crate::commands::runtime::{Runtimes, RuntimeVersion};

    fn rv(version: &str, path: &str) -> RuntimeVersion {
        RuntimeVersion { version: version.into(), source: "t".into(), path: path.into() }
    }

    fn installed() -> Runtimes {
        Runtimes {
            node: vec![rv("22.1.0", "/n22"), rv("20.11.0", "/n20"), rv("20.5.1", "/n20old"), rv("18.19.1", "/n18")],
            java: vec![rv("21.0.1", "/j21"), rv("17.0.9", "/j17"), rv("1.8.0_392", "/j8")],
        }
    }

    fn dir(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("devbox-detect-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        for (f, c) in files {
            let p = d.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, c).unwrap();
        }
        d
    }

    #[test]
    fn nvmrc_selects_latest_of_that_major() {
        let d = dir("nvmrc", &[("package.json", r#"{"name":"web","scripts":{"dev":"vite"}}"#), (".nvmrc", "v20\n")]);
        let r = detect_in(&d, &installed()).unwrap();
        let n = r.node.unwrap();
        assert_eq!((n.wanted.as_str(), n.source.as_str()), ("20", ".nvmrc"));
        assert_eq!(n.matched.unwrap().version, "20.11.0", "同主版本取最新");
        assert!(r.java.is_none());
    }

    #[test]
    fn engines_range_picks_lowest_satisfying_major() {
        let d = dir("engines", &[("package.json", r#"{"name":"w","engines":{"node":">=18"}}"#)]);
        let n = detect_in(&d, &installed()).unwrap().node.unwrap();
        assert_eq!(n.source, "package.json (engines)");
        assert_eq!(n.matched.unwrap().version, "18.19.1", "满足 >=18 的最低主版本，最保守");
    }

    #[test]
    fn unsatisfiable_node_reports_wanted_without_match() {
        let d = dir("nonode", &[(".nvmrc", "14.21.3")]);
        let n = detect_in(&d, &installed()).unwrap().node.unwrap();
        assert_eq!(n.wanted, "14.21.3");
        assert!(n.matched.is_none(), "本机没有 Node 14");
    }

    #[test]
    fn vague_node_specs_are_ignored() {
        for spec in ["lts/*", "lts/iron", "node", "latest", ""] {
            let d = dir("vague", &[(".nvmrc", spec)]);
            assert!(detect_in(&d, &installed()).unwrap().node.is_none(), "{spec:?}");
        }
    }

    #[test]
    fn pom_java_version_matches_legacy_1_8() {
        let pom = "<project><properties><java.version>1.8</java.version></properties></project>";
        let d = dir("pom8", &[("pom.xml", pom)]);
        let j = detect_in(&d, &installed()).unwrap().java.unwrap();
        assert_eq!((j.wanted.as_str(), j.source.as_str()), ("8", "pom.xml"));
        assert_eq!(j.matched.unwrap().path, "/j8", "1.8 与 8 视为同一个版本");
    }

    #[test]
    fn java_requirement_from_other_files() {
        let cases: &[(&str, &str, &str, &str)] = &[
            (".sdkmanrc", "java=17.0.9-tem\n", "17", ".sdkmanrc"),
            (".tool-versions", "nodejs 20.1.0\njava corretto-8.402.08.1\n", "8", ".tool-versions"),
            (".java-version", "11\n", "11", ".java-version"),
            ("build.gradle", "java { toolchain { languageVersion = JavaLanguageVersion.of(21) } }", "21", "build.gradle"),
            ("build.gradle", "sourceCompatibility = '1.8'", "8", "build.gradle"),
            ("build.gradle.kts", "kotlin { jvmToolchain(17) }", "17", "build.gradle.kts"),
        ];
        for (file, content, major, src) in cases {
            let d = dir("jreq", &[(file, content)]);
            let r = detect_in(&d, &installed()).unwrap().java.unwrap_or_else(|| panic!("{file}"));
            assert_eq!((r.wanted.as_str(), r.source.as_str()), (*major, *src), "{file}");
        }
        // 引用了变量的写法无法判断，不应乱猜
        let d = dir("jvar", &[("pom.xml", "<properties><java.version>${jdk}</java.version></properties>")]);
        assert!(detect_in(&d, &installed()).unwrap().java.is_none());
    }

    #[test]
    fn tool_versions_gives_node_too() {
        let d = dir("tv", &[(".tool-versions", "nodejs 18.19.1\njava 17\n")]);
        let r = detect_in(&d, &installed()).unwrap();
        assert_eq!(r.node.unwrap().matched.unwrap().version, "18.19.1");
        assert_eq!(r.java.unwrap().matched.unwrap().path, "/j17");
    }

    #[test]
    fn multi_module_maven_gets_a_launch_hint() {
        let root = "<project><modules><module>core</module><module>admin</module></modules></project>";
        let d = dir("mm", &[
            ("pom.xml", root),
            ("core/pom.xml", "<project/>"),
            ("admin/pom.xml", "<project><build><plugins><plugin><artifactId>spring-boot-maven-plugin</artifactId></plugin></plugins></build></project>"),
        ]);
        let r = detect_in(&d, &installed()).unwrap();
        assert_eq!(r.notes.len(), 1);
        assert!(r.notes[0].contains("No plugin found") && r.notes[0].contains("admin"), "{:?}", r.notes);
        // 单模块 / 根 pom 自带插件：不打扰
        let d = dir("single", &[("pom.xml", "<project><artifactId>a</artifactId></project>")]);
        assert!(detect_in(&d, &installed()).unwrap().notes.is_empty());
    }
}
