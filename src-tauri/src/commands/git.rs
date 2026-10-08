//! 一键拉取代码：对项目工作目录执行 `git pull --ff-only` 并给出可读的结果。
//!
//! 只允许快进（fast-forward）：本地有分叉时不会擅自产生 merge 提交，
//! 而是明确告诉用户需要手动处理。

use crate::commands::process::kill_tree;
use crate::models::{Project, StartOutcome};
use crate::state::AppState;
use serde::Serialize;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tauri::{Manager, State};

/// 网络拉取的最长等待时间
const PULL_TIMEOUT: Duration = Duration::from_secs(120);
/// 本地查询类命令的最长等待时间
const QUERY_TIMEOUT: Duration = Duration::from_secs(20);

/// 拉取某个项目的最新代码
#[tauri::command(async)]
pub fn git_pull(id: String, state: State<AppState>) -> Result<StartOutcome, String> {
    let project = state
        .config
        .lock()
        .unwrap()
        .projects
        .iter()
        .find(|p| p.id == id)
        .cloned()
        .ok_or("找不到项目")?;
    let running = state.procs.lock().unwrap().contains_key(&id);
    let result = pull_in(&project.path, running);
    refresh_status_of(&state, &id);
    result
}

struct GitOut {
    ok: bool,
    stdout: String,
    stderr: String,
}

/// 通过登录 shell 运行 git（保证能找到 Homebrew 装的 git），并避免交互式提示卡住：
/// 不弹终端口令提示、ssh 不询问；LC_ALL=C 让报错是英文，便于识别原因。
fn run_git(dir: &str, args: &[&str], timeout: Duration) -> Result<GitOut, String> {
    run_git_with(dir, args, timeout, true)
}

/// login 为 false 时直接执行 git，不经过登录 shell：后台轮询状态用，省掉加载用户 profile 的开销
fn run_git_with(dir: &str, args: &[&str], timeout: Duration, login: bool) -> Result<GitOut, String> {
    let mut cmd = if login {
        let mut c = Command::new("/bin/sh");
        c.args(["-lc", "exec git \"$@\"", "sh"]);
        c
    } else {
        // GUI 应用的 PATH 很短，补上 Homebrew 的常见位置
        let mut c = Command::new("git");
        let path = std::env::var("PATH").unwrap_or_default();
        c.env("PATH", format!("/opt/homebrew/bin:/usr/local/bin:{path}"))
            // 只读查询不去刷新 / 锁定索引，免得和用户自己的 git 操作抢锁。
            // 只设在这个子进程上，不污染本进程（以及之后启动的项目）的环境
            .env("GIT_OPTIONAL_LOCKS", "0");
        c
    };
    cmd.args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    let mut child = cmd.spawn().map_err(|e| format!("无法执行 git: {e}"))?;

    // 单独线程读输出，避免管道写满导致子进程阻塞
    let drain = |mut r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = r.read_to_string(&mut s);
            s
        })
    };
    let out_t = drain(Box::new(child.stdout.take().unwrap()));
    let err_t = drain(Box::new(child.stderr.take().unwrap()));

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if started.elapsed() >= timeout => {
                kill_tree(child.id());
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("git 执行超过 {} 秒，已中止", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(format!("等待 git 失败: {e}")),
        }
    };
    Ok(GitOut {
        ok: status.success(),
        stdout: out_t.join().unwrap_or_default().trim().to_string(),
        stderr: err_t.join().unwrap_or_default().trim().to_string(),
    })
}

fn pull_in(path: &str, project_running: bool) -> Result<StartOutcome, String> {
    if !std::path::Path::new(path).is_dir() {
        return Err(format!("工作目录不存在或不是目录：{path}"));
    }
    let q = |args: &[&str]| run_git(path, args, QUERY_TIMEOUT);

    if !q(&["rev-parse", "--is-inside-work-tree"])?.ok {
        return Err("该目录不是 git 仓库，无法拉取代码".into());
    }
    let branch = q(&["rev-parse", "--abbrev-ref", "HEAD"])?.stdout;
    if branch == "HEAD" {
        return Err("当前处于 detached HEAD（游离）状态，无法拉取；请先切换到某个分支".into());
    }
    let before = q(&["rev-parse", "HEAD"])?;
    if !before.ok {
        return Err("仓库还没有任何提交，无法拉取".into());
    }

    let pull = match run_git(path, &["pull", "--ff-only"], PULL_TIMEOUT) {
        Ok(p) => p,
        Err(e) if e.contains("已中止") => {
            return Err("拉取超时（120 秒），已中止；请检查网络，或在终端手动执行 git pull".into())
        }
        Err(e) => return Err(e),
    };
    if !pull.ok {
        return Err(explain_failure("拉取", &pull.stderr, &pull.stdout));
    }

    let after = q(&["rev-parse", "HEAD"])?.stdout;
    if after == before.stdout {
        return Ok(StartOutcome::success(format!("已是最新（分支 {branch}）")));
    }
    let range = format!("{}..{}", before.stdout, after);
    let count = q(&["rev-list", "--count", &range])?.stdout;
    let stat = q(&["diff", "--shortstat", &before.stdout, &after])?.stdout;
    let mut msg = format!("分支 {branch} 已更新 {count} 个提交");
    if !stat.is_empty() {
        msg.push_str(&format!("：{}", stat.trim()));
    }
    if project_running {
        msg.push_str("\n项目正在运行，需要点「重启」才会用上新代码");
    }
    Ok(StartOutcome::success(msg))
}

/// 把 git 的英文报错翻译成原因 + 处理建议，并附上原始输出的最后几行
fn explain_failure(action: &str, stderr: &str, stdout: &str) -> String {
    let raw = if stderr.is_empty() { stdout } else { stderr };
    let lower = raw.to_lowercase();
    let hint = if lower.contains("pathspec") && lower.contains("did not match") {
        "分支不存在（可能已被删除，可先「获取远程分支」刷新列表）"
    } else if lower.contains("already exists") {
        "同名分支已存在"
    } else if lower.contains("not possible to fast-forward") || lower.contains("diverging branches") {
        "本地分支与远程已分叉，不能快进；需要在终端手动 merge 或 rebase"
    } else if lower.contains("no tracking information") || lower.contains("no upstream") {
        "当前分支没有设置上游分支（git branch --set-upstream-to）"
    } else if lower.contains("would be overwritten") || lower.contains("local changes")
        || lower.contains("unmerged") || lower.contains("needs merge")
    {
        "本地有未提交的修改会被覆盖（或存在未解决的冲突）；请先提交或 stash"
    } else if lower.contains("could not resolve host") || lower.contains("unable to access")
        || lower.contains("connection") || lower.contains("timed out")
    {
        "网络连接失败，请检查网络或代理"
    } else if lower.contains("authentication failed") || lower.contains("permission denied")
        || lower.contains("could not read username") || lower.contains("terminal prompts disabled")
    {
        "认证失败；请先在终端配置好凭据或 SSH key（这里不会弹出口令输入）"
    } else {
        "git 执行失败"
    };
    let tail: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).rev().take(5).collect();
    let tail: Vec<&str> = tail.into_iter().rev().collect();
    format!("{action}失败：{hint}\n{}", tail.join("\n"))
}


// ---------- 状态徽标：领先 / 落后 / 未提交 ----------

/// 一个仓库的轻量状态，用于在项目行上显示徽标
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct GitStatus {
    /// 本地领先上游的提交数（还没推送）
    pub ahead: u32,
    /// 本地落后上游的提交数（以最近一次 fetch 的结果为准）
    pub behind: u32,
    /// 有未提交修改的已跟踪文件数
    pub dirty: u32,
    pub has_upstream: bool,
}

/// 解析 `git status --porcelain=v2 --branch` 的输出
pub fn parse_status(text: &str) -> GitStatus {
    let mut st = GitStatus::default();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# branch.ab ") {
            // "+2 -1"
            st.has_upstream = true;
            for tok in rest.split_whitespace() {
                if let Some(n) = tok.strip_prefix('+') {
                    st.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = tok.strip_prefix('-') {
                    st.behind = n.parse().unwrap_or(0);
                }
            }
        } else if line.starts_with("1 ") || line.starts_with("2 ") || line.starts_with("u ") {
            st.dirty += 1; // 普通修改 / 重命名 / 冲突
        }
    }
    st
}

/// 计算某个目录的 git 状态；不是 git 仓库返回 None
pub fn compute_status(path: &str) -> Option<GitStatus> {
    find_git_dir(Path::new(path))?;
    let out = run_git_with(path, &["status", "--porcelain=v2", "--branch", "--untracked-files=no"], Duration::from_secs(10), false).ok()?;
    out.ok.then(|| parse_status(&out.stdout))
}

/// 前端读取：所有 git 项目最近一次计算的状态（来自后台缓存，很轻）
#[tauri::command]
pub fn project_git_status(state: State<AppState>) -> HashMap<String, GitStatus> {
    state.git_status.lock().unwrap().clone()
}

/// 立即重算某个项目的状态（拉取 / 切换分支之后调用，让徽标马上更新）
pub fn refresh_status_of(state: &AppState, id: &str) {
    let Some(path) = state.config.lock().unwrap().projects.iter().find(|p| p.id == id).map(|p| p.path.clone()) else {
        return;
    };
    let mut cache = state.git_status.lock().unwrap();
    match compute_status(&path) {
        Some(s) => {
            cache.insert(id.to_string(), s);
        }
        None => {
            cache.remove(id);
        }
    }
}

/// 重算所有项目的状态，供后台线程周期调用
pub fn refresh_all_status(state: &AppState) {
    let projects: Vec<(String, String)> =
        state.config.lock().unwrap().projects.iter().map(|p| (p.id.clone(), p.path.clone())).collect();
    let mut fresh: HashMap<String, GitStatus> = HashMap::new();
    for (id, path) in projects {
        if let Some(s) = compute_status(&path) {
            fresh.insert(id, s);
        }
    }
    *state.git_status.lock().unwrap() = fresh;
}

// ---------- 分支 ----------

/// 项目当前所在分支；detached 时 name 是短哈希
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct BranchInfo {
    pub name: String,
    pub detached: bool,
}

/// 从工作目录向上找 .git（项目可能是 monorepo 的子目录），返回 git 目录
fn find_git_dir(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        let dot = d.join(".git");
        if dot.is_dir() {
            return Some(dot);
        }
        // worktree / submodule：.git 是一个指向真实 git 目录的文件
        if dot.is_file() {
            let text = std::fs::read_to_string(&dot).ok()?;
            let target = text.trim().strip_prefix("gitdir:")?.trim();
            let p = PathBuf::from(target);
            return Some(if p.is_absolute() { p } else { d.join(p) });
        }
        dir = d.parent();
    }
    None
}

/// 直接读 HEAD 文件得到当前分支，不启动任何进程，所以能放心地频繁调用
pub fn current_branch(path: &str) -> Option<BranchInfo> {
    let git_dir = find_git_dir(Path::new(path))?;
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref: refs/heads/") {
        Some(name) => Some(BranchInfo { name: name.to_string(), detached: false }),
        None if head.len() >= 7 && head.chars().all(|c| c.is_ascii_hexdigit()) => {
            Some(BranchInfo { name: head[..7].to_string(), detached: true })
        }
        None => None,
    }
}

/// 后台线程：每 20 秒重算一次所有项目的 git 状态。单独一个线程，
/// 某个大仓库的 status 慢也不会拖住进程巡检（崩溃检测、托盘刷新）
pub fn spawn_status_worker(app: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        {
            let state = app.state::<AppState>();
            refresh_all_status(&state);
        }
        std::thread::sleep(Duration::from_secs(20));
    });
}

/// 所有项目的当前分支（id -> 分支）；非 git 项目不出现
#[tauri::command]
pub fn project_branches(state: State<AppState>) -> HashMap<String, BranchInfo> {
    let projects: Vec<Project> = state.config.lock().unwrap().projects.clone();
    projects
        .iter()
        .filter_map(|p| current_branch(&p.path).map(|b| (p.id.clone(), b)))
        .collect()
}

#[derive(Serialize, Clone, Debug)]
pub struct BranchEntry {
    /// 本地分支名，或 "origin/feature-x" 这样的远程分支名
    pub name: String,
    /// "local" | "remote"
    pub kind: &'static str,
    pub current: bool,
    pub hash: String,
    /// 最近提交的相对时间，如 "3 days ago"
    pub date: String,
    pub subject: String,
}

#[derive(Serialize, Debug)]
pub struct BranchList {
    pub current: Option<BranchInfo>,
    /// 有未提交修改的文件数（不含未跟踪文件）
    pub dirty: usize,
    pub branches: Vec<BranchEntry>,
}

fn project_of(state: &AppState, id: &str) -> Result<Project, String> {
    state
        .config
        .lock()
        .unwrap()
        .projects
        .iter()
        .find(|p| p.id == id)
        .cloned()
        .ok_or_else(|| "找不到项目".to_string())
}

fn ensure_repo(path: &str) -> Result<(), String> {
    if !Path::new(path).is_dir() {
        return Err(format!("工作目录不存在或不是目录：{path}"));
    }
    if !run_git(path, &["rev-parse", "--is-inside-work-tree"], QUERY_TIMEOUT)?.ok {
        return Err("该目录不是 git 仓库".into());
    }
    Ok(())
}

/// 列出可切换的分支：本地分支在前（当前分支置顶），然后是「只存在于远程」的分支
#[tauri::command(async)]
pub fn git_branches(id: String, state: State<AppState>) -> Result<BranchList, String> {
    let project = project_of(&state, &id)?;
    list_branches(&project.path)
}

fn list_branches(path: &str) -> Result<BranchList, String> {
    ensure_repo(path)?;
    // 字段用 0x1f 分隔，避免提交信息里的空格、制表符干扰
    let fmt = "%(refname)%1f%(HEAD)%1f%(objectname:short)%1f%(committerdate:relative)%1f%(contents:subject)";
    let out = run_git(
        path,
        &["for-each-ref", "--sort=-committerdate", &format!("--format={fmt}"), "refs/heads", "refs/remotes"],
        QUERY_TIMEOUT,
    )?;
    if !out.ok {
        return Err(explain_failure("读取分支", &out.stderr, &out.stdout));
    }
    let dirty = run_git(path, &["status", "--porcelain", "--untracked-files=no"], QUERY_TIMEOUT)?
        .stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();
    Ok(BranchList {
        current: current_branch(path),
        dirty,
        branches: parse_branches(&out.stdout),
    })
}

/// 解析 for-each-ref 的输出：本地在前，远程只保留没有同名本地分支的，并去掉 origin/HEAD
fn parse_branches(text: &str) -> Vec<BranchEntry> {
    let mut locals: Vec<BranchEntry> = Vec::new();
    let mut remotes: Vec<BranchEntry> = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\u{1f}').collect();
        if f.len() < 5 {
            continue;
        }
        let entry = |name: &str, kind: &'static str| BranchEntry {
            name: name.to_string(),
            kind,
            current: f[1].trim() == "*",
            hash: f[2].to_string(),
            date: f[3].to_string(),
            subject: f[4].to_string(),
        };
        if let Some(name) = f[0].strip_prefix("refs/heads/") {
            locals.push(entry(name, "local"));
        } else if let Some(name) = f[0].strip_prefix("refs/remotes/") {
            if name.ends_with("/HEAD") {
                continue;
            }
            remotes.push(entry(name, "remote"));
        }
    }
    // 当前分支置顶，其余保持「最近提交在前」
    locals.sort_by_key(|b| !b.current);
    remotes.retain(|r| {
        let short = r.name.split_once('/').map(|(_, rest)| rest).unwrap_or(&r.name);
        !locals.iter().any(|l| l.name == short)
    });
    locals.extend(remotes);
    locals
}

/// 切换到指定分支。kind 为 "remote" 时（如 origin/feature-x）：
/// 若本地已有同名分支就直接切过去，否则新建一个跟踪它的本地分支。
#[tauri::command(async)]
pub fn git_checkout(id: String, name: String, kind: String, state: State<AppState>) -> Result<StartOutcome, String> {
    let project = project_of(&state, &id)?;
    let running = state.procs.lock().unwrap().contains_key(&id);
    let result = checkout_in(&project.path, &name, &kind, running);
    refresh_status_of(&state, &id);
    result
}

fn checkout_in(path: &str, name: &str, kind: &str, project_running: bool) -> Result<StartOutcome, String> {
    ensure_repo(path)?;
    if name.is_empty() || name.starts_with('-') || name.contains("..") || name.chars().any(|c| c.is_control() || c == ' ') {
        return Err(format!("非法的分支名：{name}"));
    }
    let exists = |full_ref: &str| {
        run_git(path, &["rev-parse", "--verify", "--quiet", full_ref], QUERY_TIMEOUT).map(|o| o.ok)
    };

    let (args, done): (Vec<String>, String) = if kind == "remote" {
        let (_, local) = name.split_once('/').ok_or_else(|| format!("非法的远程分支名：{name}"))?;
        if !exists(&format!("refs/remotes/{name}"))? {
            return Err(format!("远程分支 {name} 不存在，请先「获取远程分支」刷新列表"));
        }
        if exists(&format!("refs/heads/{local}"))? {
            (vec!["checkout".into(), local.into(), "--".into()], format!("已切换到本地分支 {local}"))
        } else {
            (
                vec!["checkout".into(), "-b".into(), local.into(), "--track".into(), name.into()],
                format!("已创建并切换到 {local}（跟踪 {name}）"),
            )
        }
    } else {
        if !exists(&format!("refs/heads/{name}"))? {
            return Err(format!("本地分支 {name} 不存在"));
        }
        (vec!["checkout".into(), name.into(), "--".into()], format!("已切换到分支 {name}"))
    };

    let argv: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let out = run_git(path, &argv, PULL_TIMEOUT)?;
    if !out.ok {
        return Err(explain_failure("切换分支", &out.stderr, &out.stdout));
    }
    let mut msg = done;
    if project_running {
        msg.push_str("\n项目正在运行，代码已变化，可能需要点「重启」才会生效");
    }
    Ok(StartOutcome::success(msg))
}

/// 获取远程最新的分支信息（git fetch --prune），让列表里能看到新分支
#[tauri::command(async)]
pub fn git_fetch(id: String, state: State<AppState>) -> Result<StartOutcome, String> {
    let project = project_of(&state, &id)?;
    ensure_repo(&project.path)?;
    let out = match run_git(&project.path, &["fetch", "--prune"], PULL_TIMEOUT) {
        Ok(o) => o,
        Err(e) if e.contains("已中止") => return Err("获取超时（120 秒），已中止；请检查网络".into()),
        Err(e) => return Err(e),
    };
    if !out.ok {
        return Err(explain_failure("获取远程分支", &out.stderr, &out.stdout));
    }
    refresh_status_of(&state, &id); // fetch 之后「落后」数会变
    Ok(StartOutcome::success("已获取远程最新分支".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn git(dir: &Path, args: &[&str]) {
        let o = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    }

    /// 建一个裸远程 + 两个克隆（a 是被测项目，b 用来推新提交）
    fn setup(name: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let base = std::env::temp_dir().join(format!("devbox-git-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let origin = base.join("origin.git");
        git(&base, &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
        let a = base.join("a");
        let b = base.join("b");
        git(&base, &["clone", "-q", origin.to_str().unwrap(), a.to_str().unwrap()]);
        std::fs::write(a.join("f.txt"), "1\n").unwrap();
        git(&a, &["add", "."]);
        git(&a, &["commit", "-q", "-m", "init"]);
        git(&a, &["push", "-q", "-u", "origin", "HEAD:main"]);
        git(&base, &["clone", "-q", origin.to_str().unwrap(), b.to_str().unwrap()]);
        (a, b, origin)
    }

    fn push_commit(repo: &Path, file: &str, text: &str) {
        std::fs::write(repo.join(file), text).unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-q", "-m", file]);
        git(repo, &["push", "-q", "origin", "HEAD:main"]);
    }

    #[test]
    fn already_up_to_date() {
        let (a, _b, _o) = setup("uptodate");
        let out = pull_in(a.to_str().unwrap(), false).unwrap();
        assert!(out.message.contains("已是最新"), "{}", out.message);
    }

    #[test]
    fn pulls_new_commits_and_summarizes() {
        let (a, b, _o) = setup("pull");
        push_commit(&b, "x.txt", "x\n");
        push_commit(&b, "y.txt", "y\n");
        let out = pull_in(a.to_str().unwrap(), false).unwrap();
        assert!(out.message.contains("已更新 2 个提交"), "{}", out.message);
        assert!(out.message.contains("2 files changed"), "{}", out.message);
        assert!(a.join("y.txt").exists());
        assert!(!out.message.contains("重启"));
    }

    #[test]
    fn running_project_is_told_to_restart() {
        let (a, b, _o) = setup("running");
        push_commit(&b, "x.txt", "x\n");
        let out = pull_in(a.to_str().unwrap(), true).unwrap();
        assert!(out.message.contains("重启"), "{}", out.message);
    }

    #[test]
    fn diverged_branch_is_explained_not_merged() {
        let (a, b, _o) = setup("diverged");
        push_commit(&b, "remote.txt", "r\n");
        std::fs::write(a.join("local.txt"), "l\n").unwrap();
        git(&a, &["add", "."]);
        git(&a, &["commit", "-q", "-m", "local"]);
        let err = pull_in(a.to_str().unwrap(), false).unwrap_err();
        assert!(err.contains("已分叉"), "{err}");
        assert!(!a.join("remote.txt").exists(), "不应擅自合并");
    }

    #[test]
    fn dirty_tree_conflict_is_explained() {
        let (a, b, _o) = setup("dirty");
        push_commit(&b, "f.txt", "remote change\n");
        std::fs::write(a.join("f.txt"), "local uncommitted\n").unwrap();
        let err = pull_in(a.to_str().unwrap(), false).unwrap_err();
        assert!(err.contains("未提交的修改"), "{err}");
    }

    #[test]
    fn non_repo_and_missing_dir() {
        let d = std::env::temp_dir().join(format!("devbox-git-plain-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        assert!(pull_in(d.to_str().unwrap(), false).unwrap_err().contains("不是 git 仓库"));
        assert!(pull_in("/no/such/dir", false).unwrap_err().contains("不存在"));
    }

    #[test]
    fn explains_common_failures() {
        assert!(explain_failure("拉取", "fatal: unable to access 'https://x': Could not resolve host: x", "").contains("网络"));
        assert!(explain_failure("拉取", "Permission denied (publickey).", "").contains("认证"));
        assert!(explain_failure("拉取", "There is no tracking information for the current branch.", "").contains("上游分支"));
        assert!(explain_failure("拉取", "weird", "").contains("git 执行失败"));
    }

    // ---------- 分支 ----------

    fn branch_of(dir: &Path) -> String {
        current_branch(dir.to_str().unwrap()).map(|b| b.name).unwrap_or_default()
    }

    #[test]
    fn current_branch_reads_head_without_git() {
        let (a, _b, _o) = setup("curbranch");
        assert_eq!(branch_of(&a), "main");
        git(&a, &["checkout", "-q", "-b", "feature/x"]);
        assert_eq!(branch_of(&a), "feature/x", "带斜杠的分支名也要完整");
        // 子目录（monorepo）也能向上找到 .git
        std::fs::create_dir_all(a.join("web/app")).unwrap();
        assert_eq!(branch_of(&a.join("web/app")), "feature/x");
        // detached：返回短哈希并标记
        let sha = run_git(a.to_str().unwrap(), &["rev-parse", "HEAD"], QUERY_TIMEOUT).unwrap().stdout;
        git(&a, &["checkout", "-q", "--detach", &sha]);
        let b = current_branch(a.to_str().unwrap()).unwrap();
        assert!(b.detached && b.name == sha[..7], "{b:?}");
        // 非 git 目录
        let plain = std::env::temp_dir().join(format!("devbox-nogit-{}", std::process::id()));
        std::fs::create_dir_all(&plain).unwrap();
        assert!(current_branch(plain.to_str().unwrap()).is_none(), "非 git 目录不应有分支");
    }

    #[test]
    fn worktree_style_git_file_is_followed() {
        let (a, _b, _o) = setup("wt");
        let wt = a.parent().unwrap().join("wt-dir");
        git(&a, &["worktree", "add", "-q", "-b", "wt-branch", wt.to_str().unwrap()]);
        assert!(wt.join(".git").is_file());
        assert_eq!(branch_of(&wt), "wt-branch");
    }

    #[test]
    fn lists_local_then_remote_only_branches() {
        let (a, b, _o) = setup("list");
        git(&a, &["branch", "dev"]);
        // 远程新增 release 分支；a 还没 fetch，所以先看不到
        git(&b, &["checkout", "-q", "-b", "release"]);
        git(&b, &["push", "-q", "-u", "origin", "release"]);
        let before = list_branches(a.to_str().unwrap()).unwrap();
        assert!(!before.branches.iter().any(|x| x.name == "origin/release"));

        git(&a, &["fetch", "-q"]);
        let l = list_branches(a.to_str().unwrap()).unwrap();
        let names: Vec<_> = l.branches.iter().map(|x| (x.name.as_str(), x.kind, x.current)).collect();
        assert_eq!(names[0], ("main", "local", true), "当前分支置顶: {names:?}");
        assert!(names.contains(&("dev", "local", false)));
        assert!(names.contains(&("origin/release", "remote", false)));
        assert!(!names.iter().any(|(n, _, _)| *n == "origin/main"), "已有同名本地分支的远程不重复列出");
        assert!(!names.iter().any(|(n, _, _)| n.ends_with("/HEAD")), "不列 origin/HEAD");
        assert_eq!(l.dirty, 0);
        assert!(!l.branches[0].subject.is_empty() && !l.branches[0].hash.is_empty());
    }

    #[test]
    fn checkout_local_and_remote_tracking() {
        let (a, b, _o) = setup("checkout");
        git(&a, &["branch", "dev"]);
        let out = checkout_in(a.to_str().unwrap(), "dev", "local", false).unwrap();
        assert!(out.message.contains("已切换到分支 dev"), "{}", out.message);
        assert_eq!(branch_of(&a), "dev");

        // 远程独有分支：新建跟踪分支
        git(&b, &["checkout", "-q", "-b", "feature/y"]);
        std::fs::write(b.join("y.txt"), "y").unwrap();
        git(&b, &["add", "."]);
        git(&b, &["commit", "-q", "-m", "y"]);
        git(&b, &["push", "-q", "-u", "origin", "feature/y"]);
        git(&a, &["fetch", "-q"]);
        let out = checkout_in(a.to_str().unwrap(), "origin/feature/y", "remote", true).unwrap();
        assert!(out.message.contains("已创建并切换到 feature/y（跟踪 origin/feature/y）"), "{}", out.message);
        assert!(out.message.contains("重启"), "运行中要提示重启");
        assert_eq!(branch_of(&a), "feature/y");
        assert!(a.join("y.txt").exists(), "工作区内容随分支变化");

        // 再选同一个远程分支：本地已有，直接切过去而不是报错
        git(&a, &["checkout", "-q", "main"]);
        let out = checkout_in(a.to_str().unwrap(), "origin/feature/y", "remote", false).unwrap();
        assert!(out.message.contains("已切换到本地分支 feature/y"), "{}", out.message);
    }

    #[test]
    fn checkout_blocked_by_local_changes_is_explained_and_safe() {
        let (a, _b, _o) = setup("blocked");
        git(&a, &["checkout", "-q", "-b", "other"]);
        std::fs::write(a.join("f.txt"), "other version\n").unwrap();
        git(&a, &["commit", "-qam", "other"]);
        git(&a, &["checkout", "-q", "main"]);
        std::fs::write(a.join("f.txt"), "my uncommitted work\n").unwrap();

        let l = list_branches(a.to_str().unwrap()).unwrap();
        assert_eq!(l.dirty, 1);
        let err = checkout_in(a.to_str().unwrap(), "other", "local", false).unwrap_err();
        assert!(err.contains("未提交的修改"), "{err}");
        assert_eq!(branch_of(&a), "main", "失败时留在原分支");
        assert_eq!(std::fs::read_to_string(a.join("f.txt")).unwrap(), "my uncommitted work\n", "不丢失未提交修改");
    }

    #[test]
    fn checkout_rejects_bad_or_missing_branches() {
        let (a, _b, _o) = setup("badbranch");
        let p = a.to_str().unwrap();
        assert!(checkout_in(p, "--orphan", "local", false).unwrap_err().contains("非法"));
        assert!(checkout_in(p, "a b", "local", false).unwrap_err().contains("非法"));
        assert!(checkout_in(p, "nope", "local", false).unwrap_err().contains("不存在"));
        assert!(checkout_in(p, "origin/nope", "remote", false).unwrap_err().contains("不存在"));
        assert!(checkout_in(std::env::temp_dir().to_str().unwrap(), "main", "local", false).is_err());
    }

    #[test]
    fn parse_branches_handles_odd_subjects() {
        let text = "refs/heads/main\u{1f}*\u{1f}abc1234\u{1f}2 days ago\u{1f}fix: a\tb | c\n\
                    refs/remotes/origin/HEAD\u{1f} \u{1f}abc1234\u{1f}2 days ago\u{1f}x\n\
                    refs/remotes/origin/dev\u{1f} \u{1f}def5678\u{1f}1 day ago\u{1f}wip\n\
                    garbage line\n";
        let b = parse_branches(text);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].subject, "fix: a\tb | c");
        assert_eq!(b[1].name, "origin/dev");
    }

    // ---------- 状态徽标 ----------

    #[test]
    fn parse_status_reads_ahead_behind_and_dirty() {
        let text = "# branch.oid abc\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -3\n\
                    1 .M N... 100644 100644 100644 a b f1.txt\n\
                    2 R. N... 100644 100644 100644 a b R100 new.txt\told.txt\n\
                    u UU N... 1 2 3 4 a b c conflict.txt\n";
        assert_eq!(parse_status(text), GitStatus { ahead: 2, behind: 3, dirty: 3, has_upstream: true });
        // 没有上游：没有 branch.ab 行
        let st = parse_status("# branch.oid abc\n# branch.head main\n1 .M N... x x x x x f.txt\n");
        assert_eq!(st, GitStatus { ahead: 0, behind: 0, dirty: 1, has_upstream: false });
        assert_eq!(parse_status(""), GitStatus::default());
    }

    #[test]
    fn status_of_a_real_repo_tracks_ahead_behind_dirty() {
        let (a, b, _o) = setup("status");
        let p = a.to_str().unwrap();
        let s = compute_status(p).unwrap();
        assert_eq!(s, GitStatus { ahead: 0, behind: 0, dirty: 0, has_upstream: true });

        // 本地提交 → 领先 1
        std::fs::write(a.join("l.txt"), "l").unwrap();
        git(&a, &["add", "."]);
        git(&a, &["commit", "-q", "-m", "local"]);
        assert_eq!(compute_status(p).unwrap().ahead, 1);

        // 远程新提交，fetch 之后 → 落后 1（没 fetch 之前不知道）
        push_commit_rebased(&b, "r.txt");
        assert_eq!(compute_status(p).unwrap().behind, 0, "没 fetch，看不到远程的新提交");
        git(&a, &["fetch", "-q"]);
        let s = compute_status(p).unwrap();
        assert_eq!((s.ahead, s.behind), (1, 1));

        // 修改已跟踪文件 → dirty 1；新建未跟踪文件不算
        std::fs::write(a.join("f.txt"), "changed\n").unwrap();
        std::fs::write(a.join("untracked.txt"), "u").unwrap();
        assert_eq!(compute_status(p).unwrap().dirty, 1);
    }

    fn push_commit_rebased(repo: &Path, file: &str) {
        std::fs::write(repo.join(file), "x").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-q", "-m", file]);
        git(repo, &["push", "-q", "origin", "HEAD:main"]);
    }

    #[test]
    fn no_upstream_and_non_repo() {
        let (a, _b, _o) = setup("noup");
        git(&a, &["checkout", "-q", "-b", "local-only"]);
        let s = compute_status(a.to_str().unwrap()).unwrap();
        assert!(!s.has_upstream && s.ahead == 0 && s.behind == 0);
        let plain = std::env::temp_dir().join(format!("devbox-nogit-status-{}", std::process::id()));
        std::fs::create_dir_all(&plain).unwrap();
        assert!(compute_status(plain.to_str().unwrap()).is_none());
    }

    #[test]
    fn status_polling_does_not_leak_env_into_this_process() {
        let (a, _b, _o) = setup("envleak");
        compute_status(a.to_str().unwrap()).unwrap();
        assert!(std::env::var_os("GIT_OPTIONAL_LOCKS").is_none(), "不能污染本进程环境");
    }

    #[test]
    fn status_query_does_not_take_the_index_lock() {
        // GIT_OPTIONAL_LOCKS=0：即使用户正在做 git 操作（持有 index.lock），状态查询也不会失败或卡住
        let (a, _b, _o) = setup("lock");
        std::fs::write(a.join(".git/index.lock"), "").unwrap();
        assert!(compute_status(a.to_str().unwrap()).is_some());
        std::fs::remove_file(a.join(".git/index.lock")).unwrap();
    }

    #[test]
    fn cache_is_updated_after_a_pull() {
        use crate::models::Config;
        let (a, b, _o) = setup("cache");
        let st = AppState::for_test(Config {
            projects: vec![serde_json::from_value(serde_json::json!({
                "id": "p", "name": "p", "path": a.to_string_lossy(), "start_command": "x"
            }))
            .unwrap()],
            ..Default::default()
        });
        refresh_all_status(&st);
        assert_eq!(st.git_status.lock().unwrap().get("p").unwrap().behind, 0);
        push_commit_rebased(&b, "n.txt");
        git(&a, &["fetch", "-q"]);
        refresh_status_of(&st, "p");
        assert_eq!(st.git_status.lock().unwrap().get("p").unwrap().behind, 1);
        pull_in(a.to_str().unwrap(), false).unwrap();
        refresh_status_of(&st, "p");
        assert_eq!(st.git_status.lock().unwrap().get("p").unwrap().behind, 0, "拉取后徽标清零");
        // 非 git 项目不进缓存
        st.config.lock().unwrap().projects[0].path = "/tmp".into();
        refresh_all_status(&st);
        assert!(st.git_status.lock().unwrap().is_empty());
    }
}
