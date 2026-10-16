//! 项目环境（dev / test / prod …）：自动读取项目里声明的环境，启动时按所选环境生效。
//!
//! 只读文件、不改项目里的任何配置文件。选定环境后，在「启动那一刻」注入环境变量或调整命令，
//! 切回「默认」就是项目原本的行为。各技术栈的约定：
//! - Spring Boot：`application-<名>.yml|yaml|properties` → `SPRING_PROFILES_ACTIVE=<名>`
//! - Node：package.json 里 `dev:test`、`start:prod` 这类脚本 → 改用该脚本；
//!   Vite / Vue CLI 的 `.env.<模式>` → 追加 `--mode <模式>`；
//!   Node 后端（Express / Nest / Koa…）的 `.env.<名>`、`config/<名>.*` → `NODE_ENV=<名>`
//! - Rails：`config/environments/<名>.rb` → `RAILS_ENV=<名>`
//! - Laravel：`.env.<名>` → `APP_ENV=<名>`

use crate::commands::startcmd::{package_manager, script_command};
use crate::models::Project;
use crate::state::AppState;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::State;

/// 项目里识别出的一个环境
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct EnvProfile {
    pub name: String,
    /// 从哪读到的，如 "application-dev.yml"、"package.json 脚本 dev:test"
    pub source: String,
    /// 选中后启动时注入的环境变量
    pub vars: BTreeMap<String, String>,
    /// 选中后实际执行的启动命令（多数情况与原启动命令相同）
    pub command: String,
    /// 一句话说明选中后会发生什么
    pub summary: String,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct EnvScan {
    pub profiles: Vec<EnvProfile>,
    /// 项目文件里本来就默认的环境（如 spring.profiles.active）；不选环境时跑的就是它
    pub default: Option<String>,
    /// 需要提醒用户注意的事项
    pub notes: Vec<String>,
}

/// 某个项目的环境信息 + 当前选择
#[derive(Serialize, Clone, Debug)]
pub struct ProjectEnvs {
    pub id: String,
    /// 当前选中的环境名；None 表示默认
    pub active: Option<String>,
    #[serde(flatten)]
    pub scan: EnvScan,
}

/// 选中环境后启动时要做的事
#[derive(Debug, PartialEq)]
pub(crate) struct Applied {
    pub name: String,
    pub command: String,
    pub vars: BTreeMap<String, String>,
    pub summary: String,
}

// ---------------------------------------------------------------- 入口

pub fn scan(dir: &Path, start_command: &str) -> EnvScan {
    let mut s = EnvScan::default();
    spring(dir, start_command, &mut s);
    node(dir, start_command, &mut s);
    rails(dir, &mut s);
    laravel(dir, &mut s);
    for p in &mut s.profiles {
        if p.command.is_empty() {
            p.command = start_command.to_string(); // 只设环境变量、不改命令的环境
        }
    }
    s.profiles = merge_by_name(std::mem::take(&mut s.profiles), start_command);
    s.profiles.sort_by(|a, b| rank(&a.name).cmp(&rank(&b.name)).then_with(|| a.name.cmp(&b.name)));
    s
}

/// 项目当前选中的环境对应的启动动作；没选返回 None。选了却在项目里找不到时报错，
/// 不悄悄退回默认——否则用户以为在跑 test，实际跑的是别的环境。
pub(crate) fn resolve(project: &Project) -> Result<Option<Applied>, String> {
    let Some(name) = project.active_env.as_deref().map(str::trim).filter(|n| !n.is_empty()) else {
        return Ok(None);
    };
    let scan = scan(Path::new(&project.path), &project.start_command);
    match scan.profiles.into_iter().find(|p| p.name == name) {
        Some(p) => Ok(Some(Applied { name: p.name, command: p.command, vars: p.vars, summary: p.summary })),
        None => Err(format!(
            "「{}」选择了环境「{name}」，但在项目里找不到它（环境文件可能被删除，或切换了分支）。请在项目里重新选择环境，或选「默认」再启动",
            project.name
        )),
    }
}

#[tauri::command(async)]
pub fn detect_environments(path: String, start_command: Option<String>) -> Result<EnvScan, String> {
    let dir = Path::new(&path);
    if !dir.is_dir() {
        return Err("目录不存在或不是文件夹".into());
    }
    Ok(scan(dir, start_command.as_deref().unwrap_or("")))
}

/// 所有项目的环境信息（列表上的「环境」标签用）
#[tauri::command(async)]
pub fn project_envs(state: State<AppState>) -> Vec<ProjectEnvs> {
    let projects = state.config.lock().unwrap().projects.clone();
    projects
        .into_iter()
        .map(|p| ProjectEnvs { scan: scan(Path::new(&p.path), &p.start_command), active: p.active_env.clone(), id: p.id })
        .collect()
}

/// 切换项目的环境；env 为 None 表示回到默认。正在运行的项目需要重启才会生效（由前端提示）。
#[tauri::command(async)]
pub fn set_project_env(id: String, env: Option<String>, state: State<AppState>) -> Result<String, String> {
    let env = env.map(|e| e.trim().to_string()).filter(|e| !e.is_empty());
    let project = state
        .config
        .lock()
        .unwrap()
        .projects
        .iter()
        .find(|p| p.id == id)
        .cloned()
        .ok_or("找不到项目")?;
    let message = match &env {
        None => "已切换为默认环境".to_string(),
        Some(name) => {
            let scan = scan(Path::new(&project.path), &project.start_command);
            let p = scan
                .profiles
                .iter()
                .find(|p| &p.name == name)
                .ok_or_else(|| format!("项目里没有识别到环境「{name}」"))?;
            format!("已切换到环境「{name}」：{}", p.summary)
        }
    };
    {
        let mut cfg = state.config.lock().unwrap();
        if let Some(p) = cfg.projects.iter_mut().find(|p| p.id == id) {
            p.active_env = env;
        }
    }
    state.persist()?;
    Ok(message)
}

// ---------------------------------------------------------------- 公共小工具

fn rd(dir: &Path, f: &str) -> Option<String> {
    fs::read_to_string(dir.join(f)).ok()
}

/// 目录下的文件名（排序）
fn file_names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    v.sort();
    v
}

/// 像环境名：非空，只有字母数字和 - _ .，不以点开头
fn valid_name(n: &str) -> bool {
    !n.is_empty() && !n.starts_with('.') && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// 常见环境的展示顺序：开发 → 测试 → 预发 → 生产 → 其它
fn rank(name: &str) -> u8 {
    match name.to_lowercase().as_str() {
        "dev" | "development" | "local" => 0,
        "test" | "testing" | "qa" | "sit" => 1,
        "staging" | "stage" | "uat" | "pre" | "preprod" | "pre-prod" => 2,
        "prod" | "production" | "online" | "release" => 3,
        _ => 4,
    }
}

const ENV_TOKENS: &[&str] = &[
    "dev", "development", "local", "test", "testing", "qa", "sit", "staging", "stage", "uat", "pre", "preprod", "prod",
    "production", "online", "release",
];

fn profile(name: &str, source: String, vars: &[(&str, &str)], command: String, summary: String) -> EnvProfile {
    EnvProfile {
        name: name.to_string(),
        source,
        vars: vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        command,
        summary,
    }
}

/// 同名环境（比如一个目录里既有 Spring 又有前端）合并成一个
fn merge_by_name(profiles: Vec<EnvProfile>, start_command: &str) -> Vec<EnvProfile> {
    let mut out: Vec<EnvProfile> = Vec::new();
    for p in profiles {
        if let Some(e) = out.iter_mut().find(|e| e.name == p.name) {
            e.vars.extend(p.vars);
            if e.command == start_command && p.command != start_command {
                e.command = p.command;
            }
            e.source = format!("{} + {}", e.source, p.source);
            e.summary = format!("{}；{}", e.summary, p.summary);
        } else {
            out.push(p);
        }
    }
    out
}

/// 命令的第一个词（跳过开头的 KEY=value 环境变量写法）
fn first_word(cmd: &str) -> &str {
    cmd.split_whitespace().find(|w| !w.contains('=')).unwrap_or("")
}

/// 启动命令是 Node 系的（npm / pnpm / yarn / node / vite …）
fn is_node_command(cmd: &str) -> bool {
    matches!(
        first_word(cmd),
        "npm" | "pnpm" | "yarn" | "bun" | "bunx" | "npx" | "pnpx" | "node" | "nodemon" | "ts-node" | "tsx" | "vite" | "next"
            | "nuxt" | "nest" | "vue-cli-service" | "cross-env" | "dotenv"
    )
}

/// 给命令追加参数；npm 需要先写 `--` 才会把参数传给脚本
pub(crate) fn append_args(cmd: &str, args: &str) -> String {
    let c = cmd.trim_end();
    if first_word(c) == "npm" && !c.contains(" -- ") {
        format!("{c} -- {args}")
    } else {
        format!("{c} {args}")
    }
}

/// `.env.<名>` 文件的名字（不含 .local、示例、模板）
fn dotenv_names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = file_names(dir)
        .into_iter()
        .filter_map(|f| f.strip_prefix(".env.").map(str::to_string))
        .filter(|n| valid_name(n) && !n.ends_with(".local"))
        .filter(|n| !matches!(n.as_str(), "local" | "example" | "sample" | "template" | "defaults" | "default" | "dist"))
        .collect();
    v.dedup();
    v
}

// ---------------------------------------------------------------- Spring Boot

fn spring(dir: &Path, start_command: &str, s: &mut EnvScan) {
    if !["pom.xml", "build.gradle", "build.gradle.kts"].iter().any(|f| dir.join(f).is_file()) {
        return;
    }
    let resources = resource_dirs(dir);
    let mut found: BTreeMap<String, String> = BTreeMap::new();
    for r in &resources {
        for f in file_names(r) {
            if let Some(name) = spring_profile_of(&f) {
                found.entry(name).or_insert(f);
            }
        }
    }
    if found.is_empty() {
        return;
    }

    for r in &resources {
        match spring_default_active(r) {
            Some(Active::Value(v)) => {
                s.default = Some(v);
                break;
            }
            Some(Active::Injected) => {
                s.notes.push("项目的默认环境由构建时注入（application.yml 里是 @…@ 占位符），无法静态读取；选定环境后以 SPRING_PROFILES_ACTIVE 为准".into());
                break;
            }
            None => {}
        }
    }
    if start_command.contains("spring.profiles.active") || start_command.contains("spring-boot.run.profiles") {
        s.notes.push("启动命令里已经写了 spring profile，它的优先级高于环境变量，会盖过这里的选择；请先把它从启动命令里去掉".into());
    }
    for (name, file) in found {
        let summary = format!("设置 SPRING_PROFILES_ACTIVE={name}");
        s.profiles.push(profile(&name, file, &[("SPRING_PROFILES_ACTIVE", name.as_str())], start_command.to_string(), summary));
    }
}

/// 目录自身及往下 3 层里所有的 `src/main/resources`（多模块项目每个模块各有一个）
fn resource_dirs(dir: &Path) -> Vec<PathBuf> {
    fn walk(d: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        let r = d.join("src").join("main").join("resources");
        if r.is_dir() {
            out.push(r);
        }
        if depth == 0 {
            return;
        }
        let mut subs: Vec<PathBuf> = fs::read_dir(d)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .filter(|p| {
                let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                !n.starts_with('.') && !matches!(n, "node_modules" | "target" | "build" | "dist" | "out" | "src")
            })
            .collect();
        subs.sort();
        for s in subs {
            walk(&s, depth - 1, out);
        }
    }
    let mut out = Vec::new();
    walk(dir, 3, &mut out);
    out
}

/// `application-dev.yml` / `bootstrap-test.properties` → "dev" / "test"
fn spring_profile_of(file: &str) -> Option<String> {
    let rest = file.strip_prefix("application-").or_else(|| file.strip_prefix("bootstrap-"))?;
    let name = rest
        .strip_suffix(".yml")
        .or_else(|| rest.strip_suffix(".yaml"))
        .or_else(|| rest.strip_suffix(".properties"))?;
    valid_name(name).then(|| name.to_string())
}

enum Active {
    Value(String),
    /// 值是 `@profile@` 这类构建时才替换的占位符
    Injected,
}

fn spring_default_active(res: &Path) -> Option<Active> {
    let raw = ["application.yml", "application.yaml"]
        .iter()
        .find_map(|f| rd(res, f).and_then(|t| yaml_active(&t)))
        .or_else(|| rd(res, "application.properties").and_then(|t| properties_active(&t)))?;
    let raw = raw.trim();
    if raw.starts_with('@') && raw.ends_with('@') && raw.len() > 1 {
        return Some(Active::Injected);
    }
    if let Some(inner) = raw.strip_prefix("${").and_then(|r| r.strip_suffix('}')) {
        // ${SPRING_PROFILES_ACTIVE:dev} → 取冒号后的默认值
        return inner.rsplit_once(':').map(|(_, d)| d.trim()).filter(|d| !d.is_empty()).map(|d| Active::Value(d.to_string()));
    }
    (!raw.is_empty()).then(|| Active::Value(raw.to_string()))
}

fn properties_active(text: &str) -> Option<String> {
    text.lines().map(str::trim).filter(|l| !l.starts_with('#') && !l.starts_with('!')).find_map(|l| {
        let (k, v) = l.split_once('=').or_else(|| l.split_once(':'))?;
        (k.trim() == "spring.profiles.active").then(|| v.trim().to_string()).filter(|v| !v.is_empty())
    })
}

/// 从 yml 里读 `spring.profiles.active`，支持嵌套写法和 `spring.profiles.active:` 平铺写法
fn yaml_active(text: &str) -> Option<String> {
    let mut stack: Vec<(usize, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim() == "---" {
            stack.clear(); // 多文档：每个文档各自从头算
            continue;
        }
        let body = line.trim_start();
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        let indent = line.len() - body.len();
        let Some((k, v)) = body.split_once(':') else { continue };
        let key = k.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
        let value = v.split(" #").next().unwrap_or("").trim().trim_matches(|c| c == '"' || c == '\'').to_string();
        while stack.last().is_some_and(|(i, _)| *i >= indent) {
            stack.pop();
        }
        let path: Vec<&str> = stack.iter().map(|(_, k)| k.as_str()).chain(std::iter::once(key.as_str())).collect();
        if path.join(".") == "spring.profiles.active" && !value.is_empty() {
            return Some(value);
        }
        stack.push((indent, key));
    }
    None
}

// ---------------------------------------------------------------- Node

fn node(dir: &Path, start_command: &str, s: &mut EnvScan) {
    // 启动命令不是 Node 系的（比如 package.json 只是顺带的构建工具），换脚本 / 加参数都会弄坏它
    if !start_command.trim().is_empty() && !is_node_command(start_command) {
        return;
    }
    let Some(pkg) = rd(dir, "package.json").and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else {
        return;
    };
    let pm = package_manager(dir);
    let mut found: Vec<EnvProfile> = Vec::new();

    // 1. 脚本变体：dev:test / start:prod → 选中后改用那个脚本
    if let Some(scripts) = pkg.get("scripts").and_then(|v| v.as_object()) {
        let mut keys: Vec<&String> = scripts.keys().collect();
        keys.sort();
        for k in keys {
            if let Some(env) = script_env(k) {
                if found.iter().any(|p| p.name == env) {
                    continue;
                }
                let cmd = script_command(pm, k);
                found.push(profile(&env, format!("package.json 脚本 {k}"), &[], cmd.clone(), format!("改用脚本：{cmd}")));
            }
        }
    }

    let has_dep = |n: &str| {
        ["dependencies", "devDependencies"]
            .iter()
            .any(|sec| pkg.get(sec).and_then(|v| v.as_object()).is_some_and(|o| o.contains_key(n)))
    };
    let modes = dotenv_names(dir);
    if has_dep("vite") || has_dep("@vue/cli-service") {
        // 2. Vite / Vue CLI：`--mode <名>` 会读取 .env.<名>
        for m in modes {
            if found.iter().any(|p| p.name == m) {
                continue;
            }
            let cmd = append_args(base_command(start_command), &format!("--mode {m}"));
            found.push(profile(&m, format!(".env.{m}"), &[], cmd, format!("追加 --mode {m}（读取 .env.{m}）")));
        }
    } else if ["@nestjs/core", "express", "koa", "fastify", "@hapi/hapi", "egg", "@midwayjs/core"].iter().any(|d| has_dep(d)) {
        // 3. Node 后端：用 NODE_ENV 区分 .env.<名> / config/<名>.*
        let mut names: Vec<(String, String)> = modes.into_iter().map(|m| (m.clone(), format!(".env.{m}"))).collect();
        for f in file_names(&dir.join("config")) {
            let Some((stem, ext)) = f.rsplit_once('.') else { continue };
            if ENV_TOKENS.contains(&stem) && matches!(ext, "json" | "json5" | "js" | "cjs" | "mjs" | "ts" | "yml" | "yaml" | "toml") {
                names.push((stem.to_string(), format!("config/{f}")));
            }
        }
        for (name, source) in names {
            if found.iter().any(|p| p.name == name) {
                continue;
            }
            found.push(profile(&name, source, &[("NODE_ENV", name.as_str())], start_command.to_string(), format!("设置 NODE_ENV={name}")));
        }
    }
    s.profiles.extend(found);
}

/// 启动命令为空时（新建项目还没填），用 npm run dev 当基准
fn base_command(start_command: &str) -> &str {
    if start_command.trim().is_empty() {
        "npm run dev"
    } else {
        start_command
    }
}

/// 脚本名里的环境：`dev:test` → test，`start-prod` → prod，`prod:start` → prod。
/// 只认「启动类 + 环境词」两段的写法，`build:prod`、`dev:admin`（另一个应用）都不算。
fn script_env(key: &str) -> Option<String> {
    const BASES: &[&str] = &["dev", "develop", "serve", "start"];
    let t: Vec<String> = key
        .split([':', '-', '_', '.'])
        .filter(|x| !x.is_empty())
        .map(str::to_lowercase)
        .collect();
    if t.len() != 2 || t[0] == t[1] {
        return None;
    }
    let (a, b) = (t[0].as_str(), t[1].as_str());
    if BASES.contains(&a) && ENV_TOKENS.contains(&b) {
        Some(b.to_string())
    } else if ENV_TOKENS.contains(&a) && BASES.contains(&b) {
        Some(a.to_string())
    } else {
        None
    }
}

// ---------------------------------------------------------------- Rails / Laravel

fn rails(dir: &Path, s: &mut EnvScan) {
    if !dir.join("Gemfile").is_file() {
        return;
    }
    let envs = dir.join("config").join("environments");
    for f in file_names(&envs) {
        if let Some(name) = f.strip_suffix(".rb").filter(|n| valid_name(n)) {
            s.profiles.push(profile(name, format!("config/environments/{f}"), &[("RAILS_ENV", name)], String::new(), format!("设置 RAILS_ENV={name}")));
        }
    }
    if s.profiles.iter().any(|p| p.source.starts_with("config/environments/")) {
        s.default.get_or_insert_with(|| "development".into()); // Rails 没设 RAILS_ENV 时就是 development
    }
}

fn laravel(dir: &Path, s: &mut EnvScan) {
    if !dir.join("artisan").is_file() {
        return;
    }
    for name in dotenv_names(dir) {
        s.profiles.push(profile(&name, format!(".env.{name}"), &[("APP_ENV", name.as_str())], String::new(), format!("设置 APP_ENV={name}")));
    }
    if let Some(app_env) = rd(dir, ".env").and_then(|t| {
        t.lines().map(str::trim).find_map(|l| l.strip_prefix("APP_ENV=").map(|v| v.trim().trim_matches(|c| c == '"' || c == '\'').to_string()))
    }) {
        if !app_env.is_empty() {
            s.default.get_or_insert(app_env);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let d = std::env::temp_dir().join(format!("devbox-envs-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        for (f, c) in files {
            let p = d.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, c).unwrap();
        }
        d
    }

    fn names(s: &EnvScan) -> Vec<&str> {
        s.profiles.iter().map(|p| p.name.as_str()).collect()
    }
    fn get<'a>(s: &'a EnvScan, n: &str) -> &'a EnvProfile {
        s.profiles.iter().find(|p| p.name == n).unwrap_or_else(|| panic!("没有环境 {n}: {:?}", names(s)))
    }

    // ---- Spring Boot ----

    #[test]
    fn spring_profiles_come_from_application_files_in_conventional_order() {
        let d = dir("spring", &[
            ("pom.xml", "<project/>"),
            ("src/main/resources/application.yml", "server:\n  port: 8080\n"),
            ("src/main/resources/application-prod.yml", ""),
            ("src/main/resources/application-dev.properties", ""),
            ("src/main/resources/application-test.yaml", ""),
            ("src/main/resources/bootstrap-uat.yml", ""),
            ("src/main/resources/application-local.yml.bak", ""), // 不是配置文件
        ]);
        let s = scan(&d, "mvn spring-boot:run");
        assert_eq!(names(&s), ["dev", "test", "uat", "prod"], "开发 → 测试 → 预发 → 生产");
        let p = get(&s, "test");
        assert_eq!(p.vars.get("SPRING_PROFILES_ACTIVE").map(String::as_str), Some("test"));
        assert_eq!(p.command, "mvn spring-boot:run", "不改启动命令");
        assert_eq!(p.source, "application-test.yaml");
    }

    #[test]
    fn spring_multi_module_scans_every_module_and_gradle_works_too() {
        let d = dir("spring-multi", &[
            ("build.gradle.kts", ""),
            ("admin/src/main/resources/application-dev.yml", ""),
            ("api/src/main/resources/application-dev.yml", ""),
            ("api/src/main/resources/application-prod.yml", ""),
            ("node_modules/x/src/main/resources/application-evil.yml", ""),
        ]);
        let s = scan(&d, "./gradlew bootRun");
        assert_eq!(names(&s), ["dev", "prod"], "同名合并，依赖目录不扫");
    }

    #[test]
    fn spring_default_is_read_from_yaml_nested_flat_properties_and_placeholders() {
        let nested = "spring:\n  application:\n    name: x\n  profiles:\n    active: dev # 本地\nserver:\n  port: 1\n";
        assert_eq!(yaml_active(nested).as_deref(), Some("dev"));
        assert_eq!(yaml_active("spring.profiles.active: \"test\"\n").as_deref(), Some("test"));
        assert_eq!(yaml_active("other:\n  profiles:\n    active: no\n"), None, "别的前缀下的 profiles 不算");
        let multi = "server:\n  port: 1\n---\nspring:\n  profiles:\n    active: prod\n";
        assert_eq!(yaml_active(multi).as_deref(), Some("prod"), "多文档");
        assert_eq!(properties_active("# c\nspring.profiles.active=test\n").as_deref(), Some("test"));

        let d = dir("spring-default", &[
            ("pom.xml", ""),
            ("src/main/resources/application.yml", "spring:\n  profiles:\n    active: ${SPRING_PROFILES_ACTIVE:dev}\n"),
            ("src/main/resources/application-dev.yml", ""),
            ("src/main/resources/application-prod.yml", ""),
        ]);
        assert_eq!(scan(&d, "").default.as_deref(), Some("dev"), "取占位符里的默认值");

        let d = dir("spring-injected", &[
            ("pom.xml", ""),
            ("src/main/resources/application.yml", "spring:\n  profiles:\n    active: @profileActive@\n"),
            ("src/main/resources/application-dev.yml", ""),
        ]);
        let s = scan(&d, "");
        assert_eq!(s.default, None);
        assert!(s.notes.iter().any(|n| n.contains("构建时注入")), "{:?}", s.notes);
    }

    #[test]
    fn spring_warns_when_the_start_command_pins_a_profile() {
        let d = dir("spring-pinned", &[("pom.xml", ""), ("src/main/resources/application-dev.yml", "")]);
        let s = scan(&d, "mvn spring-boot:run -Dspring-boot.run.profiles=dev");
        assert!(s.notes.iter().any(|n| n.contains("优先级高于环境变量")), "{:?}", s.notes);
        assert!(scan(&d, "mvn spring-boot:run").notes.is_empty());
    }

    #[test]
    fn spring_without_profile_files_has_no_environments() {
        let d = dir("spring-none", &[("pom.xml", ""), ("src/main/resources/application.yml", "a: 1")]);
        assert_eq!(scan(&d, ""), EnvScan::default());
    }

    // ---- Node ----

    #[test]
    fn node_script_variants_switch_to_that_script() {
        let pkg = r#"{"scripts":{"dev":"vite","dev:test":"vite --mode test","start:prod":"node a.js","prod:start":"x",
            "build:prod":"vite build","dev:admin":"vite --mode admin","test":"jest","serve-staging":"x"}}"#;
        let d = dir("node-scripts", &[("package.json", pkg)]);
        let s = scan(&d, "npm run dev");
        assert_eq!(names(&s), ["test", "staging", "prod"], "build:prod / dev:admin / test 不算环境");
        assert_eq!(get(&s, "test").command, "npm run dev:test");
        assert_eq!(get(&s, "prod").command, "npm run prod:start", "同名取字母序靠前的脚本");
        let d2 = dir("node-scripts-yarn", &[("package.json", pkg), ("yarn.lock", "")]);
        assert_eq!(get(&scan(&d2, "yarn dev"), "test").command, "yarn dev:test", "按项目用的包管理器拼命令");
    }

    #[test]
    fn vite_dotenv_modes_append_mode_flag_and_skip_local_and_examples() {
        let d = dir("vite", &[
            ("package.json", r#"{"devDependencies":{"vite":"^5"},"scripts":{"dev":"vite"}}"#),
            (".env", ""),
            (".env.local", ""),
            (".env.staging", ""),
            (".env.production.local", ""),
            (".env.production", ""),
            (".env.example", ""),
        ]);
        let s = scan(&d, "npm run dev");
        assert_eq!(names(&s), ["staging", "production"]);
        assert_eq!(get(&s, "staging").command, "npm run dev -- --mode staging", "npm 要先写 --");
        assert_eq!(get(&scan(&d, "pnpm dev"), "staging").command, "pnpm dev --mode staging");
        assert_eq!(get(&scan(&d, "npm run dev -- --host"), "staging").command, "npm run dev -- --host --mode staging", "已有 -- 不重复");
    }

    #[test]
    fn a_script_variant_wins_over_a_dotenv_mode_of_the_same_name() {
        let d = dir("vite-both", &[
            ("package.json", r#"{"devDependencies":{"vite":"^5"},"scripts":{"dev":"vite","dev:test":"vite --mode test --host"}}"#),
            (".env.test", ""),
            (".env.prod", ""),
        ]);
        let s = scan(&d, "npm run dev");
        assert_eq!(get(&s, "test").command, "npm run dev:test");
        assert_eq!(get(&s, "prod").command, "npm run dev -- --mode prod");
    }

    #[test]
    fn node_backend_uses_node_env_from_dotenv_files_and_config_dir() {
        let d = dir("nest", &[
            ("package.json", r#"{"dependencies":{"@nestjs/core":"^10"}}"#),
            (".env.development", ""),
            (".env.production", ""),
            ("config/test.json", "{}"),
            ("config/default.json", "{}"),
            ("config/database.ts", ""), // 不是环境名
        ]);
        let s = scan(&d, "node dist/main.js");
        assert_eq!(names(&s), ["development", "test", "production"]);
        assert_eq!(get(&s, "test").vars.get("NODE_ENV").map(String::as_str), Some("test"));
        assert_eq!(get(&s, "test").source, "config/test.json");
        assert_eq!(get(&s, "production").command, "node dist/main.js", "不改启动命令");
    }

    #[test]
    fn node_scan_is_skipped_when_the_start_command_is_not_node() {
        // Laravel 项目里 package.json 只是前端构建，启动的是 php artisan serve：不能给它加 --mode
        let d = dir("laravel-node", &[
            ("package.json", r#"{"devDependencies":{"vite":"^5"},"scripts":{"dev":"vite","dev:test":"x"}}"#),
            ("artisan", ""),
            (".env.testing", ""),
        ]);
        let s = scan(&d, "php artisan serve");
        assert_eq!(names(&s), ["testing"]);
        assert_eq!(get(&s, "testing").command, "php artisan serve");
        assert_eq!(get(&s, "testing").vars.get("APP_ENV").map(String::as_str), Some("testing"));
    }

    #[test]
    fn script_env_only_accepts_start_word_plus_env_word() {
        for (k, want) in [("dev:test", Some("test")), ("start:prod", Some("prod")), ("prod:start", Some("prod")),
                          ("serve-staging", Some("staging")), ("start_uat", Some("uat")), ("dev:admin", None),
                          ("build:prod", None), ("test", None), ("dev", None), ("dev:test:mock", None), ("dev:dev", None)] {
            assert_eq!(script_env(k).as_deref(), want, "{k}");
        }
    }

    #[test]
    fn append_args_handles_npm_double_dash() {
        assert_eq!(append_args("npm run dev", "--mode x"), "npm run dev -- --mode x");
        assert_eq!(append_args("npm run dev -- --host", "--mode x"), "npm run dev -- --host --mode x");
        assert_eq!(append_args("NODE_OPTIONS=--max-old-space-size=4096 npm start", "--mode x"), "NODE_OPTIONS=--max-old-space-size=4096 npm start -- --mode x");
        assert_eq!(append_args("yarn dev ", "--mode x"), "yarn dev --mode x");
        assert_eq!(append_args("npx vite", "--mode x"), "npx vite --mode x");
    }

    // ---- Rails / Laravel ----

    #[test]
    fn rails_environments_come_from_config_environments() {
        let d = dir("rails", &[
            ("Gemfile", ""),
            ("config/environments/development.rb", ""),
            ("config/environments/production.rb", ""),
            ("config/environments/staging.rb", ""),
        ]);
        let s = scan(&d, "bin/rails server");
        assert_eq!(names(&s), ["development", "staging", "production"]);
        assert_eq!(get(&s, "staging").vars.get("RAILS_ENV").map(String::as_str), Some("staging"));
        assert_eq!(get(&s, "staging").command, "bin/rails server", "Rails 只设环境变量，命令保持原样");
        assert_eq!(s.default.as_deref(), Some("development"));
    }

    #[test]
    fn laravel_environments_come_from_dotenv_files() {
        let d = dir("laravel", &[("artisan", ""), (".env", "APP_NAME=x\nAPP_ENV=\"local\"\n"), (".env.testing", ""), (".env.example", "")]);
        let s = scan(&d, "php artisan serve");
        assert_eq!(names(&s), ["testing"]);
        assert_eq!(s.default.as_deref(), Some("local"));
    }

    #[test]
    fn unknown_projects_have_no_environments() {
        let d = dir("empty", &[("README.md", "")]);
        assert_eq!(scan(&d, "make run"), EnvScan::default());
        assert_eq!(scan(&d.join("不存在"), ""), EnvScan::default());
    }

    #[test]
    fn same_named_profiles_from_different_stacks_are_merged() {
        let d = dir("merge", &[
            ("pom.xml", ""),
            ("src/main/resources/application-dev.yml", ""),
            ("package.json", r#"{"scripts":{"dev:dev":"x","start:dev":"x"}}"#),
        ]);
        let s = scan(&d, "");
        assert_eq!(names(&s), ["dev"]);
        let p = get(&s, "dev");
        assert!(p.vars.contains_key("SPRING_PROFILES_ACTIVE"));
        assert!(p.source.contains("application-dev.yml") && p.source.contains("start:dev"), "{}", p.source);
        assert_eq!(p.command, "npm run start:dev", "有命令变化的一方为准");
    }

    // ---- 生效 ----

    fn project(path: &Path, start: &str, active: Option<&str>) -> Project {
        serde_json::from_value(serde_json::json!({
            "id": "p", "name": "官网", "path": path, "start_command": start, "active_env": active,
        }))
        .unwrap()
    }

    #[test]
    fn resolve_returns_none_for_default_and_the_profile_for_a_selection() {
        let d = dir("resolve", &[("pom.xml", ""), ("src/main/resources/application-test.yml", "")]);
        assert_eq!(resolve(&project(&d, "mvn spring-boot:run", None)).unwrap(), None);
        assert_eq!(resolve(&project(&d, "mvn spring-boot:run", Some(""))).unwrap(), None);
        let a = resolve(&project(&d, "mvn spring-boot:run", Some("test"))).unwrap().unwrap();
        assert_eq!(a.name, "test");
        assert_eq!(a.command, "mvn spring-boot:run");
        assert_eq!(a.vars.get("SPRING_PROFILES_ACTIVE").map(String::as_str), Some("test"));
    }

    #[test]
    fn resolve_fails_loudly_when_the_selected_environment_vanished() {
        let d = dir("resolve-gone", &[("pom.xml", ""), ("src/main/resources/application-dev.yml", "")]);
        let err = resolve(&project(&d, "mvn spring-boot:run", Some("prod"))).unwrap_err();
        assert!(err.contains("找不到") && err.contains("prod") && err.contains("官网"), "{err}");
    }
}
