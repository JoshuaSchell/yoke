//! Worktree threads, as T3 Code has them: a thread can run in a git worktree of its own, on a
//! new branch from the project's HEAD, so several agents can change the same repository at
//! once without touching each other's files or the project's checkout. The worktrees live in
//! `~/.yoke/worktrees/<project>/<branch>`; merging the branch back is up to you.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_names() {
        assert_eq!(branch_name("Fix the flaky file-watcher test, please now", 0x3f2a), "yoke/fix-the-flaky-file-watcher-3f2a");
        assert_eq!(branch_name("¿?", 1), "yoke/thread-0001");
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
