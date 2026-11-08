//! The diff panel beside the transcript (⇧⌘D, or a turn's "N files +a −b" chip), after T3
//! Code's: what one of the thread's turns changed, everything not yet committed, or the
//! whole branch. Turns are diffed between their before and after snapshots (`checkpoint`), so
//! changes a turn made through a shell command show too, whichever agent ran it.
//!
//! Diffs are read off the UI thread (`git diff` and the files' texts) and drawn with Rune's
//! diff view, which folds unchanged lines and highlights the words that changed.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use rune_kit::diff::{DiffMode, DiffState, FileDiff, parse};
use rune_kit::shell::PaneState;
use serde_json::{Value, json};

use crate::app::{App, Kind, Thread, ThreadId};
use crate::checkpoint::{self, Raw, Stat};

/// git's empty tree, to diff against in a repository with no commits yet.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// One turn's snapshots.
#[derive(Clone, Debug, PartialEq)]
pub struct Checkpoint {
    /// Names its refs (`checkpoint::refs`).
    pub token: String,
    /// Which of the thread's prompts the turn answered, 0 for the first.
    pub prompt: usize,
    /// The folder the turn ran in, whose repository holds the refs.
    pub dir: PathBuf,
    /// The after snapshot is taken.
    pub ended: bool,
    /// What the turn changed, once the after snapshot is taken (None if it failed).
    pub stat: Option<Stat>,
    /// The turn's closing row in the transcript, which shows the chip.
    pub end_item: Option<u64>,
}

/// What the panel compares.
#[derive(Clone, Debug, PartialEq)]
pub enum Scope {
    /// A turn, by its checkpoint's token: before it ran to after (to now, while it runs).
    Turn(String),
    /// HEAD to the working tree, new files included.
    Uncommitted,
    /// Where the branch left the main one to the working tree.
    Branch,
}

pub struct DiffPanel {
    /// The panel beside the transcript, and its width.
    pub pane: PaneState,
    pub scope: Scope,
    /// The thread it shows (None: the new-thread screen's project).
    pub thread: Option<ThreadId>,
    pub state: DiffState,
    pub loading: bool,
    pub error: Option<String>,
    /// The diff was too long and its end was left out.
    pub truncated: bool,
    pub stat: Stat,
    /// The (thread, scope) `state` holds, so taking it again keeps folds and collapses.
    shown: Option<(Option<ThreadId>, Scope)>,
    rx: Option<Receiver<Result<Raw, String>>>,
}

impl Default for DiffPanel {
    fn default() -> DiffPanel {
        let mut pane = PaneState::new(560.0).range(360.0, 1100.0);
        pane.set_open(false);
        DiffPanel {
            pane,
            scope: Scope::Uncommitted,
            thread: None,
            state: DiffState::new(Vec::new()),
            loading: false,
            error: None,
            truncated: false,
            stat: Stat::default(),
            shown: None,
            rx: None,
        }
    }
}

/// The turn ends of `t`'s transcript, by prompt: the chip goes on the row that closed the
/// turn answering each prompt. Bumps rows whose chip changed so they're drawn again.
pub fn link_turn_ends(t: &mut Thread) {
    let mut prompt = 0usize;
    let mut ends: Vec<(usize, u64)> = Vec::new();
    for item in &t.items {
        match item.kind {
            Kind::User(_) => prompt += 1,
            Kind::TurnEnd { .. } if prompt > 0 => ends.push((prompt - 1, item.id)),
            _ => {}
        }
    }
    let mut bump = Vec::new();
    for c in &mut t.checkpoints {
        // The prompt's own turn: a turn the CLI started itself after it, to report on work
        // in the background, ends with a row of its own.
        let end = ends.iter().find(|(p, _)| *p == c.prompt).map(|(_, id)| *id);
        if end != c.end_item {
            c.end_item = end;
            bump.extend(end);
        }
    }
    for item in t.items.iter_mut().filter(|i| bump.contains(&i.id)) {
        item.rev += 1;
    }
    t.rev += 1;
}

/// The checkpoint index's file for a session: `checkpoints/<harness>-<session>.json` beside
/// the thread records.
fn index_file(store: &Path, t: &Thread) -> Option<PathBuf> {
    let session = t.session.as_deref()?;
    let safe: String = session.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    Some(store.parent()?.join("checkpoints").join(format!("{}-{safe}.json", t.harness.id())))
}

fn to_json(cs: &[Checkpoint]) -> Value {
    let turns: Vec<Value> = cs
        .iter()
        .filter(|c| c.ended)
        .map(|c| {
            let s = c.stat.unwrap_or_default();
            json!({"token": c.token, "prompt": c.prompt, "dir": c.dir.to_string_lossy(), "files": s.files, "added": s.added, "removed": s.removed, "stat": c.stat.is_some()})
        })
        .collect();
    json!({ "turns": turns })
}

fn from_json(v: &Value) -> Vec<Checkpoint> {
    let Some(turns) = v.get("turns").and_then(Value::as_array) else { return Vec::new() };
    turns
        .iter()
        .filter_map(|t| {
            let n = |k: &str| t.get(k).and_then(Value::as_u64).unwrap_or(0) as usize;
            let stat = t.get("stat").and_then(Value::as_bool).unwrap_or(false).then(|| Stat { files: n("files"), added: n("added"), removed: n("removed") });
            Some(Checkpoint {
                token: t.get("token")?.as_str()?.to_owned(),
                prompt: n("prompt"),
                dir: PathBuf::from(t.get("dir")?.as_str()?),
                ended: true,
                stat,
                end_item: None,
            })
        })
        .collect()
}

/// A token for a new turn's refs: unique across threads and launches.
fn token(id: ThreadId) -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("{nanos:x}-{id:x}")
}

impl App {
    /// A turn is about to start on thread `id` in `cwd`: notes its checkpoint and gives back
    /// the snapshot to take before the CLI starts, if the folder is in a git repository.
    pub(crate) fn begin_checkpoint(&mut self, id: ThreadId, cwd: &Path) -> Option<impl FnOnce() + Send + 'static> {
        let t = self.threads.iter().find(|t| t.id == id)?;
        let repo = t.worktree.is_some() || self.projects.iter().any(|p| p.repo && self.in_project(t, &p.path));
        if !repo { return None; }
        let prompts = t.items.iter().filter(|i| matches!(i.kind, Kind::User(_))).count();
        let tok = token(id);
        let (start, _) = checkpoint::refs(&tok);
        let t = self.thread_mut(id)?;
        t.checkpoints.push(Checkpoint { token: tok, prompt: prompts.saturating_sub(1), dir: cwd.to_owned(), ended: false, stat: None, end_item: None });
        let dir = cwd.to_owned();
        Some(move || { let _ = checkpoint::snapshot(&dir, &start); })
    }

    /// Thread `id`'s turn ended: takes the after snapshot off this thread and sizes the diff.
    pub(crate) fn end_checkpoint(&mut self, id: ThreadId) {
        let Some(c) = self.threads.iter().find(|t| t.id == id).and_then(|t| t.checkpoints.last()).filter(|c| !c.ended) else { return };
        let (tok, dir) = (c.token.clone(), c.dir.clone());
        let tx = self.checkpoint_tx.clone();
        let wake = self.waker.clone();
        std::thread::spawn(move || {
            let (start, end) = checkpoint::refs(&tok);
            let stat = checkpoint::snapshot(&dir, &end).and_then(|_| checkpoint::stat(&dir, &start, &end)).ok();
            let _ = tx.send((id, tok, stat));
            if let Some(w) = wake { w(); }
        });
    }

    /// An after snapshot is in: the turn's chip shows, the index is saved, and the panel takes
    /// the diff again if it shows this thread.
    pub(crate) fn land_checkpoint(&mut self, id: ThreadId, tok: &str, stat: Option<Stat>) {
        let Some(t) = self.thread_mut(id) else { return };
        let Some(c) = t.checkpoints.iter_mut().find(|c| c.token == tok) else { return };
        c.ended = true;
        c.stat = stat;
        link_turn_ends(t);
        self.save_checkpoints(id);
        if self.diff.pane.is_open() && self.diff.thread == Some(id) {
            self.load_diff();
        }
    }

    fn save_checkpoints(&self, id: ThreadId) {
        let Some(store) = &self.store else { return };
        let Some(t) = self.threads.iter().find(|t| t.id == id) else { return };
        let Some(file) = index_file(store, t) else { return };
        if t.checkpoints.iter().all(|c| !c.ended) { return; }
        let _ = std::fs::create_dir_all(file.parent().unwrap_or(store));
        let _ = std::fs::write(&file, to_json(&t.checkpoints).to_string());
    }

    /// Reads thread `id`'s saved checkpoints once its transcript is in.
    pub(crate) fn load_checkpoints(&mut self, id: ThreadId) {
        let Some(store) = self.store.clone() else { return };
        let Some(t) = self.thread_mut(id).filter(|t| !t.checkpoints_loaded) else { return };
        t.checkpoints_loaded = true;
        let Some(file) = index_file(&store, t) else { return };
        let Some(v) = std::fs::read_to_string(&file).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) else { return };
        let mut saved = from_json(&v);
        saved.retain(|c| !t.checkpoints.iter().any(|x| x.token == c.token));
        t.checkpoints.splice(0..0, saved);
        link_turn_ends(t);
    }

    /// Deleting thread `t`: its snapshots' refs and index go too.
    pub(crate) fn forget_checkpoints(&self, t: &Thread) {
        if let Some(file) = self.store.as_ref().and_then(|s| index_file(s, t)) {
            let _ = std::fs::remove_file(file);
        }
        let mut by_dir: Vec<(PathBuf, Vec<String>)> = Vec::new();
        for c in &t.checkpoints {
            let (a, b) = checkpoint::refs(&c.token);
            match by_dir.iter_mut().find(|(d, _)| *d == c.dir) {
                Some((_, refs)) => refs.extend([a, b]),
                None => by_dir.push((c.dir.clone(), vec![a, b])),
            }
        }
        if by_dir.is_empty() { return; }
        std::thread::spawn(move || {
            for (dir, refs) in by_dir {
                checkpoint::delete_refs(&dir, &refs);
            }
        });
    }

    // ---- the panel ----

    pub fn toggle_diff(&mut self) {
        if self.diff.pane.is_open() {
            self.diff.pane.set_open(false);
        } else {
            self.open_diff(None);
        }
    }

    /// Opens the panel on the selected thread: on `scope` if given, else its last turn (or
    /// what's uncommitted, if it has no turns with snapshots).
    pub fn open_diff(&mut self, scope: Option<Scope>) {
        let thread = self.selected;
        let last = thread.and_then(|id| self.threads.iter().find(|t| t.id == id)).and_then(|t| t.checkpoints.last()).map(|c| Scope::Turn(c.token.clone()));
        self.diff.scope = scope.or(last).unwrap_or(Scope::Uncommitted);
        self.diff.thread = thread;
        self.diff.pane.set_open(true);
        self.load_diff();
    }

    /// A turn's chip: the panel on that turn.
    pub fn open_turn_diff(&mut self, id: ThreadId, tok: String) {
        if self.selected != Some(id) { self.select(id); }
        self.open_diff(Some(Scope::Turn(tok)));
    }

    pub fn set_diff_scope(&mut self, scope: Scope) {
        if self.diff.scope != scope {
            self.diff.scope = scope;
            self.load_diff();
        }
    }

    /// The selected thread changed: the open panel follows it.
    pub(crate) fn diff_follows_selection(&mut self) {
        if self.diff.pane.is_open() && self.diff.thread != self.selected {
            self.open_diff(None);
        }
    }

    pub fn set_diff_mode(&mut self, split: bool) {
        self.diff.state.set_mode(if split { DiffMode::Split } else { DiffMode::Unified });
    }

    /// Reads the panel's diff again, off this thread.
    pub fn load_diff(&mut self) {
        let thread = self.diff.thread.and_then(|id| self.threads.iter().find(|t| t.id == id));
        let dir = match thread {
            Some(t) => self.run_dir(t),
            None => self.current.clone(),
        };
        let Some(dir) = dir else {
            self.diff.error = Some("No project to compare.".into());
            return;
        };
        let turn = match (&self.diff.scope, thread) {
            (Scope::Turn(tok), Some(t)) => match t.checkpoints.iter().find(|c| &c.token == tok) {
                Some(c) => Some(c.clone()),
                None => {
                    self.diff.scope = Scope::Uncommitted;
                    None
                }
            },
            _ => None,
        };
        let scope = self.diff.scope.clone();
        let (tx, rx) = channel();
        self.diff.rx = Some(rx);
        self.diff.loading = true;
        let wake = self.waker.clone();
        std::thread::spawn(move || {
            let read = || -> Result<Raw, String> {
                let repo = turn.as_ref().map_or(dir.clone(), |c| c.dir.clone());
                if checkpoint::repo_root(&repo).is_none() {
                    return Err("This folder isn't in a git repository, so there's nothing to compare.".into());
                }
                let (from, to) = match (&scope, &turn) {
                    (Scope::Turn(_), Some(c)) => {
                        let (start, end) = checkpoint::refs(&c.token);
                        let to = if c.ended { end } else { checkpoint::snapshot_tree(&repo)? };
                        (start, to)
                    }
                    (Scope::Branch, _) => {
                        let from = checkpoint::fork_point(&repo).ok_or("This is the main branch, so there are no branch changes. Uncommitted shows what's changed.")?;
                        (from, checkpoint::snapshot_tree(&repo)?)
                    }
                    _ => (checkpoint::head(&repo).unwrap_or_else(|| EMPTY_TREE.into()), checkpoint::snapshot_tree(&repo)?),
                };
                checkpoint::raw(&repo, &from, &to).map_err(|e| if e.contains("unknown revision") || e.contains("bad revision") { "This turn's snapshot is gone.".to_owned() } else { e })
            };
            let _ = tx.send(read());
            if let Some(w) = wake { w(); }
        });
    }

    /// A diff read off the UI thread is in: into the view, keeping folds if it's the same one.
    pub(crate) fn drain_diff(&mut self) -> bool {
        let Some(result) = self.diff.rx.as_ref().and_then(|r| r.try_recv().ok()) else { return false };
        self.diff.rx = None;
        self.diff.loading = false;
        match result {
            Err(e) => {
                self.diff.error = Some(e);
                self.diff.state.set_files(Vec::new());
                self.diff.shown = None;
                self.diff.stat = Stat::default();
            }
            Ok(raw) => {
                let files = files_of(raw.patch, raw.texts);
                self.diff.stat = Stat {
                    files: files.len(),
                    added: files.iter().map(FileDiff::additions).sum(),
                    removed: files.iter().map(FileDiff::deletions).sum(),
                };
                self.diff.error = None;
                self.diff.truncated = raw.truncated;
                let now = (self.diff.thread, self.diff.scope.clone());
                if self.diff.shown.as_ref() == Some(&now) {
                    self.diff.state.update_files(files);
                } else {
                    self.diff.state.set_files(files);
                }
                self.diff.shown = Some(now);
            }
        }
        true
    }

    /// The scope menu's choices for the panel's thread, newest turn first: (label, scope).
    pub fn diff_scopes(&self) -> Vec<(String, Scope)> {
        let mut out = Vec::new();
        if let Some(t) = self.diff.thread.and_then(|id| self.threads.iter().find(|t| t.id == id)) {
            let n = t.checkpoints.len();
            for (i, c) in t.checkpoints.iter().enumerate().rev() {
                let what = if i + 1 == n { "Last turn".to_owned() } else { format!("Turn {}", i + 1) };
                // Its totals are beside the menu, so the label is just which turn.
                let label = if c.ended { what } else { format!("{what} (running)") };
                out.push((label, Scope::Turn(c.token.clone())));
            }
        }
        out.push(("Uncommitted changes".into(), Scope::Uncommitted));
        out.push(("Branch changes".into(), Scope::Branch));
        out
    }
}

/// "3 files  +40 −12".
pub fn stat_words(s: Stat) -> String {
    let files = if s.files == 1 { "1 file".to_owned() } else { format!("{} files", s.files) };
    format!("{files}  +{} \u{2212}{}", s.added, s.removed)
}

/// The patch's files, each given its whole texts (so its folds open) where they were read.
fn files_of(patch: String, texts: Vec<checkpoint::Texts>) -> Vec<FileDiff> {
    parse(&patch)
        .into_iter()
        .map(|f| {
            let found = texts.iter().find(|t| t.new_path == f.new_path && t.old_path == f.old_path);
            match found {
                Some(t) => f.with_texts(t.old.as_deref(), t.new.as_deref()),
                None => f,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Item;
    use crate::harness::Harness;

    #[test]
    fn chips_go_on_the_row_that_closed_each_prompts_turn() {
        let mut t = Thread::new(1, "t".into(), Harness::Claude, "default".into());
        let kinds = [
            Kind::User("a".into()),
            Kind::TurnEnd { secs: 1.0, cost_usd: None, stopped: false },
            Kind::User("b".into()),
            Kind::Error("x".into()),
            Kind::TurnEnd { secs: 1.0, cost_usd: None, stopped: true },
        ];
        for (i, kind) in kinds.into_iter().enumerate() {
            t.items.push(Item { id: 10 + i as u64, rev: 0, kind });
        }
        let c = |tok: &str, prompt| Checkpoint { token: tok.into(), prompt, dir: PathBuf::from("/r"), ended: true, stat: None, end_item: None };
        t.checkpoints = vec![c("a", 0), c("b", 1), c("c", 2)];
        link_turn_ends(&mut t);
        let ends: Vec<Option<u64>> = t.checkpoints.iter().map(|c| c.end_item).collect();
        assert_eq!(ends, vec![Some(11), Some(14), None]);
        assert_eq!(t.items[1].rev, 1, "the row with a new chip is drawn again");
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git").arg("-C").arg(dir).args(args)
            .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
            .output().unwrap().status.success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn a_turn_is_snapshotted_before_and_after_and_its_snapshots_go_with_the_thread() {
        let repo = std::env::temp_dir().join(format!("yoke-diff-turn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "first"]);
        let mut s = App::new(Some(repo.clone()), rune::prefs::Prefs::memory());
        let mut t = Thread::new(7, "t".into(), Harness::Claude, "default".into());
        t.project = repo.canonicalize().unwrap();
        t.items.push(Item { id: 1, rev: 0, kind: Kind::User("change a".into()) });
        s.threads.push(t);
        let cwd = s.threads[0].project.clone();

        // Before the CLI starts: the snapshot. The agent changes a file; the turn ends.
        let before = s.begin_checkpoint(7, &cwd).expect("in a repository");
        before();
        std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
        s.thread_mut(7).unwrap().items.push(Item { id: 2, rev: 0, kind: Kind::TurnEnd { secs: 1.0, cost_usd: None, stopped: false } });
        s.end_checkpoint(7);
        let (id, tok, stat) = s.checkpoint_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        s.land_checkpoint(id, &tok, stat);
        let c = s.threads[0].checkpoints[0].clone();
        assert_eq!(c.stat, Some(Stat { files: 1, added: 1, removed: 0 }));
        assert_eq!(c.end_item, Some(2), "the chip is on the turn's closing row");

        // The panel on that turn reads the diff off this thread.
        s.selected = Some(7);
        s.open_diff(None);
        assert_eq!(s.diff.scope, Scope::Turn(tok.clone()));
        let mut loaded = false;
        for _ in 0..500 {
            if s.drain_diff() { loaded = true; break; }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(loaded && s.diff.error.is_none(), "{:?}", s.diff.error);
        assert_eq!(s.diff.state.files().len(), 1);
        assert_eq!(s.diff.stat, Stat { files: 1, added: 1, removed: 0 });

        // Deleting the thread deletes its refs.
        let (start, _) = checkpoint::refs(&tok);
        s.delete(7);
        let gone = (0..200).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(10));
            !std::process::Command::new("git").arg("-C").arg(&repo).args(["rev-parse", "-q", "--verify", &start]).output().unwrap().status.success()
        });
        assert!(gone);
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn stopping_while_the_snapshot_is_taken_means_the_cli_never_starts() {
        let (tx, rx) = std::sync::mpsc::channel();
        let turn = crate::harness::Turn {
            harness: Harness::Claude, prompt: "hi".into(), model: "default".into(), effort: "default".into(),
            access: crate::harness::Access::ReadOnly, resume: None, cwd: std::env::temp_dir(), images: Vec::new(), peer_messages: false, live: false, uuid: None,
        };
        let running = crate::harness::spawn_after(
            || std::thread::sleep(std::time::Duration::from_millis(200)),
            Path::new("/bin/sh"), turn, 1u8, tx, std::sync::Arc::new(|| {}),
        );
        running.stop();
        let (_, e) = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        // Started, the shell would have exited with a code of its own.
        assert!(matches!(e, crate::harness::Event::Exited { code: None, .. }), "{e:?}");
    }

    #[test]
    fn the_index_keeps_ended_turns() {
        let mut a = Checkpoint { token: "t1".into(), prompt: 0, dir: PathBuf::from("/r"), ended: true, stat: Some(Stat { files: 2, added: 5, removed: 1 }), end_item: Some(3) };
        let running = Checkpoint { token: "t2".into(), prompt: 1, ended: false, stat: None, ..a.clone() };
        let back = from_json(&to_json(&[a.clone(), running]));
        a.end_item = None;
        assert_eq!(back, vec![a]);
        assert_eq!(stat_words(Stat { files: 1, added: 3, removed: 0 }), "1 file  +3 \u{2212}0");
    }

    #[test]
    fn a_patch_and_its_texts_make_files_whose_folds_open() {
        let old: String = (1..=30).map(|i| format!("{i}\n")).collect();
        let new = old.replace("\n15\n", "\nfifteen\n");
        let patch = "diff --git a/n.txt b/n.txt\n--- a/n.txt\n+++ b/n.txt\n@@ -12,7 +12,7 @@\n 12\n 13\n 14\n-15\n+fifteen\n 16\n 17\n 18\n".to_owned();
        let texts = vec![checkpoint::Texts { old_path: Some("n.txt".into()), new_path: Some("n.txt".into()), old: Some(old), new: Some(new) }];
        let files = files_of(patch, texts);
        assert_eq!(files.len(), 1);
        assert!(files[0].expandable());
    }
}
