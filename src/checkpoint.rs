//! Snapshots of a folder's files in git, taken before and after each turn so the diff panel
//! can show what the turn changed (as T3 Code does: `GitVcsDriver`'s checkpoints, MIT).
//!
//! A snapshot is a commit of every file git would track (`git add -A` into a scratch index,
//! so the user's own index, stash and branches are never touched), parented on HEAD and kept
//! under `refs/yoke/<token>/{start,end}` so `git gc` doesn't collect it. Deleting a thread
//! deletes its refs. A turn whose folder isn't in a git repository has no snapshots.

use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Every ref Yoke makes is under this.
pub const REF_ROOT: &str = "refs/yoke";

/// The refs of turn `token`'s snapshots: before it ran, and after.
pub fn refs(token: &str) -> (String, String) {
    (format!("{REF_ROOT}/{token}/start"), format!("{REF_ROOT}/{token}/end"))
}

/// What a diff changed: files, lines added and lines removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stat {
    pub files: usize,
    pub added: usize,
    pub removed: usize,
}

fn command(dir: &Path) -> Command {
    let mut c = Command::new("git");
    // Paths as they are (no octal escapes), no pager, colour or external diff tools.
    c.arg("-C").arg(dir).args(["-c", "core.quotePath=false", "-c", "color.ui=false"]);
    c.env("GIT_TERMINAL_PROMPT", "0").env("GIT_OPTIONAL_LOCKS", "0");
    c.stdin(Stdio::null());
    c
}

/// Runs git in `dir` with `env`; its output, or why it failed.
fn git<I, S>(dir: &Path, args: I, env: &[(&str, &OsStr)]) -> Result<Vec<u8>, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut c = command(dir);
    c.args(args);
    for (k, v) in env {
        c.env(k, v);
    }
    let out = c.output().map_err(|e| format!("couldn't run git: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

fn line(out: Vec<u8>) -> String {
    String::from_utf8_lossy(&out).trim().to_owned()
}

/// The top folder of the repository `dir` is in, if it's in one.
pub fn repo_root(dir: &Path) -> Option<PathBuf> {
    git(dir, ["rev-parse", "--show-toplevel"], &[]).ok().map(line).filter(|s| !s.is_empty()).map(PathBuf::from)
}

/// HEAD's commit, unless the repository has none yet.
pub fn head(dir: &Path) -> Option<String> {
    git(dir, ["rev-parse", "-q", "--verify", "HEAD^{commit}"], &[]).ok().map(line).filter(|s| !s.is_empty())
}

/// The tree of every file in `dir`'s working tree that git would track, ignored files left
/// out: `git add -A` into a scratch copy of the index (so unchanged files are known by their
/// stat and not read again), then `write-tree`.
pub fn snapshot_tree(dir: &Path) -> Result<String, String> {
    let index = line(git(dir, ["rev-parse", "--git-path", "index"], &[])?);
    let index = if Path::new(&index).is_absolute() { PathBuf::from(index) } else { dir.join(index) };
    // Unique to this snapshot: two can run at once (two threads' turns starting together).
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let scratch = std::env::temp_dir().join(format!("yoke-index-{}-{n}", std::process::id()));
    let _ = std::fs::copy(&index, &scratch);
    let env = [("GIT_INDEX_FILE", scratch.as_os_str())];
    let tree = git(dir, ["add", "-A", "--", "."], &env)
        .and_then(|_| git(dir, ["write-tree"], &env))
        .map(line);
    let _ = std::fs::remove_file(&scratch);
    let _ = std::fs::remove_file(scratch.with_extension("lock"));
    tree
}

/// Snapshots `dir` as a commit on HEAD and points `refname` at it. Returns the commit.
pub fn snapshot(dir: &Path, refname: &str) -> Result<String, String> {
    let tree = snapshot_tree(dir)?;
    let who: &OsStr = OsStr::new("Yoke");
    let mail: &OsStr = OsStr::new("yoke@localhost");
    let env = [("GIT_AUTHOR_NAME", who), ("GIT_AUTHOR_EMAIL", mail), ("GIT_COMMITTER_NAME", who), ("GIT_COMMITTER_EMAIL", mail)];
    let mut args = vec!["commit-tree".to_owned(), tree, "-m".into(), "Yoke checkpoint".into()];
    if let Some(h) = head(dir) {
        args.push("-p".into());
        args.push(h);
    }
    let commit = line(git(dir, &args, &env)?);
    git(dir, ["update-ref", refname, &commit], &[])?;
    Ok(commit)
}

/// Deletes `refs` (missing ones are fine).
pub fn delete_refs(dir: &Path, refs: &[String]) {
    if refs.is_empty() {
        return;
    }
    let mut c = command(dir);
    c.args(["update-ref", "--stdin"]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    let Ok(mut child) = c.spawn() else { return };
    if let Some(mut input) = child.stdin.take() {
        for r in refs.iter().filter(|r| r.starts_with(REF_ROOT)) {
            let _ = writeln!(input, "delete {r}");
        }
    }
    let _ = child.wait();
}

/// Lines added and removed between two commits or trees, by file (`--numstat`, renames
/// found). Binary files count as changed with no lines.
pub fn stat(dir: &Path, from: &str, to: &str) -> Result<Stat, String> {
    let out = git(dir, ["diff", "--numstat", "-z", "-M", "--no-ext-diff", from, to, "--"], &[])?;
    Ok(parse_numstat(&out))
}

fn parse_numstat(out: &[u8]) -> Stat {
    let mut s = Stat::default();
    let mut fields = out.split(|b| *b == 0).peekable();
    while let Some(entry) = fields.next() {
        if entry.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(entry);
        let mut parts = text.splitn(3, '\t');
        let (Some(a), Some(r), Some(path)) = (parts.next(), parts.next(), parts.next()) else { continue };
        s.files += 1;
        s.added += a.parse::<usize>().unwrap_or(0);
        s.removed += r.parse::<usize>().unwrap_or(0);
        // A rename's paths follow as two more fields.
        if path.is_empty() {
            fields.next();
            fields.next();
        }
    }
    s
}

/// The commit this branch forked from: where HEAD meets the repository's main branch
/// (origin's default, else `main` or `master`). None on the main branch itself.
pub fn fork_point(dir: &Path) -> Option<String> {
    let current = line(git(dir, ["rev-parse", "--abbrev-ref", "HEAD"], &[]).ok()?);
    let remote_default = git(dir, ["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"], &[]).ok().map(line);
    let candidates = remote_default.into_iter().chain(["main", "master", "origin/main", "origin/master"].map(String::from));
    for base in candidates {
        if base == current || base.strip_prefix("origin/") == Some(current.as_str()) {
            return None;
        }
        if let Ok(mb) = git(dir, ["merge-base", "HEAD", &base], &[]) {
            return Some(line(mb));
        }
    }
    None
}

/// A diff, as git writes it, and the whole texts of its files on each side, for the panel.
pub struct Raw {
    pub patch: String,
    /// Each file's (old path, new path) and texts, where they were read.
    pub texts: Vec<Texts>,
    /// The patch was cut short at `MAX_PATCH`.
    pub truncated: bool,
}

pub struct Texts {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub old: Option<String>,
    pub new: Option<String>,
}

/// The most of a patch read; past it the panel says the rest was left out.
const MAX_PATCH: usize = 8 << 20;
/// A file's whole text is read only up to this, for opening folds.
const MAX_TEXT: u64 = 1 << 20;
/// And no more than this in all.
const MAX_TEXTS: u64 = 24 << 20;

/// The diff from `from` to `to` (commits or trees) with its files' texts, read with one
/// `git diff` and one `git cat-file --batch`.
pub fn raw(dir: &Path, from: &str, to: &str) -> Result<Raw, String> {
    let mut c = command(dir);
    c.args(["diff", "-M", "--no-ext-diff", "--src-prefix=a/", "--dst-prefix=b/", from, to, "--"]);
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().map_err(|e| format!("couldn't run git: {e}"))?;
    let mut bytes = Vec::new();
    child.stdout.take().expect("piped").take(MAX_PATCH as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let truncated = bytes.len() > MAX_PATCH;
    if truncated {
        let _ = child.kill();
        // Up to the last whole file.
        let cut = bytes.windows(12).rposition(|w| w == b"\ndiff --git ").map_or(0, |i| i + 1);
        bytes.truncate(cut);
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if !truncated && !status.success() {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        return Err(err.trim().to_owned());
    }
    let patch = String::from_utf8_lossy(&bytes).into_owned();
    let names = git(dir, ["diff", "--name-status", "-z", "-M", "--no-ext-diff", from, to, "--"], &[])?;
    let pairs = parse_name_status(&names);
    let texts = read_texts(dir, from, to, pairs).unwrap_or_default();
    Ok(Raw { patch, texts, truncated })
}

/// (old path, new path) per file from `--name-status -z`: `M\0path\0`, `A\0path\0`,
/// `D\0path\0`, `R100\0old\0new\0`.
fn parse_name_status(out: &[u8]) -> Vec<(Option<String>, Option<String>)> {
    let mut fields = out.split(|b| *b == 0).map(|f| String::from_utf8_lossy(f).into_owned());
    let mut files = Vec::new();
    while let Some(status) = fields.next() {
        let Some(kind) = status.chars().next() else { continue };
        let Some(a) = fields.next() else { break };
        match kind {
            'R' | 'C' => {
                let Some(b) = fields.next() else { break };
                files.push((Some(a), Some(b)));
            }
            'A' => files.push((None, Some(a))),
            'D' => files.push((Some(a), None)),
            _ => files.push((Some(a.clone()), Some(a))),
        }
    }
    files
}

/// Each file's whole texts from `from` and `to`, through one `git cat-file --batch`. A
/// binary, missing or too large blob is left out.
fn read_texts(dir: &Path, from: &str, to: &str, pairs: Vec<(Option<String>, Option<String>)>) -> Result<Vec<Texts>, String> {
    let mut c = command(dir);
    c.args(["cat-file", "--batch"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = c.spawn().map_err(|e| e.to_string())?;
    let mut input = child.stdin.take().expect("piped");
    let asks: Vec<String> = pairs
        .iter()
        .flat_map(|(a, b)| [a.as_ref().map(|p| format!("{from}:{p}")), b.as_ref().map(|p| format!("{to}:{p}"))])
        .flatten()
        .collect();
    // Written from its own thread so a long list can't fill the pipe while we read.
    let writer = std::thread::spawn(move || {
        for a in asks {
            if writeln!(input, "{a}").is_err() {
                break;
            }
        }
    });
    let mut out = BufReader::new(child.stdout.take().expect("piped"));
    let mut budget = MAX_TEXTS;
    let mut next = |out: &mut BufReader<_>| -> Option<String> {
        let mut header = String::new();
        out.read_line(&mut header).ok()?;
        let mut parts = header.split_whitespace();
        let (_, kind, size) = (parts.next()?, parts.next()?, parts.next()?.parse::<u64>().ok()?);
        let mut body = vec![0; size as usize];
        out.read_exact(&mut body).ok()?;
        let mut nl = [0u8; 1];
        out.read_exact(&mut nl).ok()?;
        if kind != "blob" || size > MAX_TEXT || size > budget || body.contains(&0) {
            return None;
        }
        budget -= size;
        String::from_utf8(body).ok()
    };
    let mut texts = Vec::with_capacity(pairs.len());
    for (a, b) in pairs {
        let old = if a.is_some() { next(&mut out) } else { None };
        let new = if b.is_some() { next(&mut out) } else { None };
        texts.push(Texts { old_path: a, new_path: b, old, new });
    }
    let _ = writer.join();
    let _ = child.wait();
    Ok(texts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("yoke-checkpoint-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, ["init", "-q", "-b", "main"], &[]).unwrap();
        let env: [(&str, &OsStr); 4] = [("GIT_AUTHOR_NAME", "t".as_ref()), ("GIT_AUTHOR_EMAIL", "t@t".as_ref()), ("GIT_COMMITTER_NAME", "t".as_ref()), ("GIT_COMMITTER_EMAIL", "t@t".as_ref())];
        std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(dir.join(".gitignore"), "ignored/\n").unwrap();
        git(&dir, ["add", "-A"], &[]).unwrap();
        git(&dir, ["commit", "-q", "-m", "first"], &env).unwrap();
        dir
    }

    #[test]
    fn snapshots_take_untracked_files_and_leave_the_index_alone() {
        let dir = repo("snap");
        let (start, end) = refs("t1");
        snapshot(&dir, &start).unwrap();
        std::fs::write(dir.join("a.txt"), "one\n2\nthree\n").unwrap();
        std::fs::write(dir.join("new.txt"), "fresh\n").unwrap();
        std::fs::create_dir_all(dir.join("ignored")).unwrap();
        std::fs::write(dir.join("ignored/x.txt"), "no\n").unwrap();
        snapshot(&dir, &end).unwrap();
        assert_eq!(stat(&dir, &start, &end).unwrap(), Stat { files: 2, added: 2, removed: 1 });
        // The user's index still has only the first commit's files staged.
        let staged = line(git(&dir, ["diff", "--cached", "--name-only"], &[]).unwrap());
        assert_eq!(staged, "");
        let status = line(git(&dir, ["status", "--porcelain"], &[]).unwrap());
        assert!(status.contains("?? new.txt"), "{status}");
        // The diff and its texts; the ignored file isn't in it.
        let raw = raw(&dir, &start, &end).unwrap();
        assert!(raw.patch.contains("+2") && raw.patch.contains("+fresh") && !raw.patch.contains("ignored"));
        let a = raw.texts.iter().find(|t| t.new_path.as_deref() == Some("a.txt")).unwrap();
        assert_eq!((a.old.as_deref(), a.new.as_deref()), (Some("one\ntwo\nthree\n"), Some("one\n2\nthree\n")));
        let n = raw.texts.iter().find(|t| t.new_path.as_deref() == Some("new.txt")).unwrap();
        assert_eq!((n.old_path.as_deref(), n.new.as_deref()), (None, Some("fresh\n")));
        // Deleting the refs leaves no trace of them.
        delete_refs(&dir, &[start.clone(), end]);
        assert!(git(&dir, ["rev-parse", "-q", "--verify", &start], &[]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_working_tree_diffs_against_head_and_a_branch_against_its_fork() {
        let dir = repo("work");
        assert_eq!(fork_point(&dir), None, "on main itself");
        git(&dir, ["checkout", "-q", "-b", "feature"], &[]).unwrap();
        let fork = head(&dir).unwrap();
        assert_eq!(fork_point(&dir), Some(fork.clone()));
        std::fs::write(dir.join("b.txt"), "b\n").unwrap();
        let tree = snapshot_tree(&dir).unwrap();
        assert_eq!(stat(&dir, &fork, &tree).unwrap(), Stat { files: 1, added: 1, removed: 0 });
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// How long a snapshot of a real folder takes: `YOKE_SNAPSHOT_DIR=<repo> cargo test
    /// --release snapshot_time -- --ignored --nocapture`. Writes only a dangling tree object.
    #[test]
    #[ignore]
    fn snapshot_time() {
        let dir = PathBuf::from(std::env::var("YOKE_SNAPSHOT_DIR").expect("YOKE_SNAPSHOT_DIR"));
        let mut times = Vec::new();
        for _ in 0..5 {
            let t = std::time::Instant::now();
            snapshot_tree(&dir).unwrap();
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        println!("snapshot_tree: median {:.1} ms, max {:.1} ms (5 runs)", times[2], times[4]);
    }

    #[test]
    fn numstat_and_name_status_read_renames() {
        let s = parse_numstat(b"1\t2\ta.txt\x00-\t-\tlogo.png\x003\t0\t\x00old.rs\x00new.rs\x00");
        assert_eq!(s, Stat { files: 3, added: 4, removed: 2 });
        let n = parse_name_status(b"M\x00a.txt\x00R087\x00old.rs\x00new.rs\x00A\x00b\x00D\x00c\x00");
        assert_eq!(n, vec![
            (Some("a.txt".into()), Some("a.txt".into())),
            (Some("old.rs".into()), Some("new.rs".into())),
            (None, Some("b".into())),
            (Some("c".into()), None),
        ]);
    }
}
