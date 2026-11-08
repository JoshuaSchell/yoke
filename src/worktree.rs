//! Worktree threads, as T3 Code has them: a thread can run in a git worktree of its own, on a
//! new branch from the project's HEAD, so several agents can change the same repository at
//! once without touching each other's files or the project's checkout. The worktrees live in
//! `~/.yoke/worktrees/<project>/<branch>`; merging the branch back is up to you. Once it's
//! merged (or its pull request is), the thread settles on its own (`App::check_merged`).

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, PartialEq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
}

/// Where worktrees go. A path without spaces, since agents run shell commands in it.
pub fn root() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".yoke/worktrees"))
}

/// Whether `p` is a folder inside a git repository: a worktree can only be made of one.
pub fn is_repo(p: &Path) -> bool {
    p.ancestors().any(|a| a.join(".git").exists())
}

/// `yoke/fix-the-flaky-watcher-3f2a`: the first words of the prompt, made safe for a branch
/// name, and `salt` in hex so two threads with the same start don't collide.
pub fn branch_name(prompt: &str, salt: u64) -> String {
    let words: Vec<String> = prompt
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(5)
        .map(|w| w.to_ascii_lowercase())
        .collect();
    let slug = if words.is_empty() { "thread".to_owned() } else { words.join("-") };
    let slug: String = slug.chars().take(40).collect();
    format!("yoke/{}-{:04x}", slug.trim_end_matches('-'), salt & 0xffff)
}

/// Makes a worktree of `project` on a new branch `branch` from its HEAD. Blocks on git, so
/// call it off the UI thread. The error is git's own message.
pub fn create(project: &Path, branch: &str) -> Result<Worktree, String> {
    let root = root().ok_or("No home folder to put the worktree in")?;
    let name = project.file_name().map_or_else(|| "project".into(), |n| n.to_string_lossy().into_owned());
    let path = root.join(name).join(branch.replace('/', "-"));
    std::fs::create_dir_all(path.parent().unwrap_or(&root)).map_err(|e| e.to_string())?;
    let out = Command::new("git")
        .arg("-C").arg(project)
        .args(["worktree", "add", "-b", branch])
        .arg(&path)
        .output()
        .map_err(|e| format!("Couldn't run git: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err.trim().lines().last().unwrap_or("git worktree add failed").to_owned());
    }
    Ok(Worktree { path: path.canonicalize().unwrap_or(path), branch: branch.to_owned() })
}

/// Whether `branch` of `project` has been merged: it has commits of its own (it moved since
/// it was made, by its reflog) and the default branch, local or on `origin`, now holds them.
/// Blocks on git, so call it off the UI thread.
pub fn branch_merged(project: &Path, branch: &str) -> bool {
    let git = |args: &[&str]| Command::new("git").arg("-C").arg(project).args(args).output().ok();
    let line = |args: &[&str]| git(args).filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
    let head = format!("refs/heads/{branch}");
    let Some(tip) = line(&["rev-parse", "--verify", "--quiet", &head]) else { return false };
    // The commit it was made at: the oldest entry of its reflog. Without one there's no
    // telling a fresh branch from a merged one.
    let Some(made) = line(&["rev-list", "--walk-reflogs", &head]).and_then(|l| l.lines().last().map(str::to_owned)) else { return false };
    if made == tip { return false; }
    ["refs/remotes/origin/HEAD", "refs/remotes/origin/main", "refs/remotes/origin/master", "refs/heads/main", "refs/heads/master"].iter()
        .filter(|r| line(&["rev-parse", "--verify", "--quiet", r]).is_some())
        .any(|r| git(&["merge-base", "--is-ancestor", &tip, r]).is_some_and(|o| o.status.success()))
}

/// Whether `branch`'s pull request on GitHub is done with: merged or closed, and none still
/// open. Asks `gh` (at `gh`), so it needs it signed in; any failure is no.
/// Blocks on the network, so call it off the UI thread.
pub fn pull_request_done(gh: &Path, project: &Path, branch: &str) -> bool {
    let out = Command::new(gh)
        .current_dir(project)
        .env("GH_PROMPT_DISABLED", "1")
        .args(["pr", "list", "--head", branch, "--state", "all", "--json", "state", "--limit", "10"])
        .output();
    let Some(out) = out.ok().filter(|o| o.status.success()) else { return false };
    let Ok(prs) = serde_json::from_slice::<Vec<serde_json::Value>>(&out.stdout) else { return false };
    let states: Vec<&str> = prs.iter().filter_map(|p| p["state"].as_str()).collect();
    !states.is_empty() && !states.contains(&"OPEN")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_names() {
        assert_eq!(branch_name("Fix the flaky file-watcher test, please now", 0x3f2a), "yoke/fix-the-flaky-file-watcher-3f2a");
        assert_eq!(branch_name("¿?", 1), "yoke/thread-0001");
    }

    /// A branch counts as merged only once it has commits of its own and main holds them.
    #[test]
    fn a_branch_is_merged_once_main_has_its_commits() {
        let repo = std::env::temp_dir().join(format!("yoke-merged-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| assert!(Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap().status.success(), "git {args:?}");
        let commit = |msg: &str| git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", msg]);
        git(&["init", "-q", "-b", "main"]);
        commit("first");
        git(&["branch", "yoke/fresh"]);
        assert!(!branch_merged(&repo, "yoke/fresh"), "a branch with no commits of its own isn't merged");
        git(&["checkout", "-q", "-b", "yoke/work"]);
        commit("work");
        assert!(!branch_merged(&repo, "yoke/work"), "not on main yet");
        git(&["checkout", "-q", "main"]);
        git(&["merge", "-q", "--ff-only", "yoke/work"]);
        assert!(branch_merged(&repo, "yoke/work"));
        assert!(!branch_merged(&repo, "yoke/gone"), "no such branch");
        let _ = std::fs::remove_dir_all(&repo);
    }

    /// A worktree of a scratch repository, on its own branch, with the repository's files.
    #[test]
    fn makes_a_worktree() {
        let repo = std::env::temp_dir().join(format!("yoke-wt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| assert!(Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap().status.success(), "git {args:?}");
        git(&["init", "-q"]);
        std::fs::write(repo.join("a.txt"), "hello").unwrap();
        git(&["add", "."]);
        git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "first"]);
        assert!(is_repo(&repo));

        let branch = format!("yoke/test-{}", std::process::id());
        let wt = create(&repo, &branch).unwrap();
        assert_eq!(std::fs::read_to_string(wt.path.join("a.txt")).unwrap(), "hello");
        let head = Command::new("git").arg("-C").arg(&wt.path).args(["branch", "--show-current"]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), branch);
        assert!(create(&repo, &branch).is_err(), "the branch exists now");

        git(&["worktree", "remove", "--force", wt.path.to_str().unwrap()]);
        let _ = std::fs::remove_dir(wt.path.parent().unwrap());
        let _ = std::fs::remove_dir_all(&repo);
    }
}
