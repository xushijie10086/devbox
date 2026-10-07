//! 一键拉取代码：对项目工作目录执行 `git pull --ff-only` 并给出可读的结果。
//!
//! 只允许快进（fast-forward）：本地有分叉时不会擅自产生 merge 提交，
//! 而是明确告诉用户需要手动处理。

use crate::commands::process::kill_tree;
use crate::models::StartOutcome;
use crate::state::AppState;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tauri::State;

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
    pull_in(&project.path, running)
}

struct GitOut {
    ok: bool,
    stdout: String,
    stderr: String,
}

/// 通过登录 shell 运行 git（保证能找到 Homebrew 装的 git），并避免交互式提示卡住：
/// 不弹终端口令提示、ssh 不询问；LC_ALL=C 让报错是英文，便于识别原因。
fn run_git(dir: &str, args: &[&str], timeout: Duration) -> Result<GitOut, String> {
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-lc", "exec git \"$@\"", "sh"])
        .args(args)
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
        return Err(explain_failure(&pull.stderr, &pull.stdout));
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
fn explain_failure(stderr: &str, stdout: &str) -> String {
    let raw = if stderr.is_empty() { stdout } else { stderr };
    let lower = raw.to_lowercase();
    let hint = if lower.contains("not possible to fast-forward") || lower.contains("diverging branches") {
        "本地分支与远程已分叉，不能快进；需要在终端手动 merge 或 rebase"
    } else if lower.contains("no tracking information") || lower.contains("no upstream") {
        "当前分支没有设置上游分支（git branch --set-upstream-to）"
    } else if lower.contains("would be overwritten") || lower.contains("local changes") {
        "本地有未提交的修改会被覆盖；请先提交或 stash"
    } else if lower.contains("could not resolve host") || lower.contains("unable to access")
        || lower.contains("connection") || lower.contains("timed out")
    {
        "网络连接失败，请检查网络或代理"
    } else if lower.contains("authentication failed") || lower.contains("permission denied")
        || lower.contains("could not read username") || lower.contains("terminal prompts disabled")
    {
        "认证失败；请先在终端配置好凭据或 SSH key（这里不会弹出口令输入）"
    } else {
        "git pull 失败"
    };
    let tail: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).rev().take(5).collect();
    let tail: Vec<&str> = tail.into_iter().rev().collect();
    format!("拉取失败：{hint}\n{}", tail.join("\n"))
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
        assert!(explain_failure("fatal: unable to access 'https://x': Could not resolve host: x", "").contains("网络"));
        assert!(explain_failure("Permission denied (publickey).", "").contains("认证"));
        assert!(explain_failure("There is no tracking information for the current branch.", "").contains("上游分支"));
        assert!(explain_failure("weird", "").contains("git pull 失败"));
    }
}
