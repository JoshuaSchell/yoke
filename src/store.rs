//! Threads the app keeps itself: Codex and OpenCode ones, whose CLIs keep no history this app
//! reads back (Claude Code's sessions come from its own files, `history`), and threads in
//! worktrees, which Claude Code files under the worktree's folder rather than the project's. One JSON file per
//! thread in `threads/` beside `prefs.json`, written when a turn ends, so the thread is in its
//! project's group at the next launch and replying resumes its session.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

use crate::app::{Kind, Thread, ToolState};
use crate::harness::{ALL, Harness};
use crate::worktree::Worktree;

/// Where the records go: `threads/` beside the prefs file. `None` for in-memory prefs (tests).
pub fn dir(prefs: &rune::prefs::Prefs) -> Option<PathBuf> {
    Some(prefs.path()?.parent()?.join("threads"))
}

/// Which threads have a record, once they have a session to resume: the ones whose CLI
/// keeps none we read, and the ones in worktrees.
pub fn keeps(t: &Thread) -> bool {
    (t.harness != Harness::Claude || t.worktree.is_some()) && t.session.is_some()
}

fn file(dir: &Path, h: Harness, session: &str) -> PathBuf {
    let safe: String = session.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    dir.join(format!("{}-{safe}.json", h.id()))
}

/// A record found by `list`: enough for the sidebar row. The transcript loads when it opens.
pub struct Saved {
    pub harness: Harness,
    pub session: String,
    pub title: String,
    pub model: String,
    pub modified: SystemTime,
    pub path: PathBuf,
    pub worktree: Option<Worktree>,
}

/// The record of `t` as JSON text, for `write` (made on the UI thread, written off it).
pub fn record(t: &Thread) -> Option<String> {
    let session = t.session.as_deref()?;
    // Claude Code keeps a Claude thread's transcript itself.
    let items: Vec<Value> = if t.harness == Harness::Claude { Vec::new() } else { t.items.iter().filter_map(|i| kind_json(&i.kind)).collect() };
    let updated = t.updated.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let doc = json!({
        "version": 1,
        "harness": t.harness.id(),
        "session": session,
        "project": t.project.to_string_lossy(),
        "title": t.title,
        "model": t.model,
        "updated": updated,
        "worktree": t.worktree.as_ref().map(|w| json!({ "path": w.path.to_string_lossy(), "branch": w.branch })),
        "items": items,
    });
    Some(doc.to_string())
}

/// Saves `t`'s record into `dir`, off the UI thread.
pub fn save(dir: &Path, t: &Thread) {
    let Some(session) = t.session.as_deref() else { return };
    let Some(text) = record(t) else { return };
    let (dir, path) = (dir.to_owned(), file(dir, t.harness, session));
    std::thread::spawn(move || {
        let _ = std::fs::create_dir_all(&dir);
        // Write then rename, so a crash mid-write leaves the last good record.
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() { let _ = std::fs::rename(&tmp, &path); }
    });
}

pub fn remove(dir: &Path, h: Harness, session: &str) {
    let _ = std::fs::remove_file(file(dir, h, session));
}

/// The records of threads in `project`, newest first.
pub fn list(dir: &Path, project: &Path) -> Vec<Saved> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Saved> = entries.flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let path = e.path();
            let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
            if Path::new(v["project"].as_str()?) != project { return None; }
            let harness = ALL.into_iter().find(|h| Some(h.id()) == v["harness"].as_str())?;
            Some(Saved {
                harness,
                session: v["session"].as_str()?.to_owned(),
                title: v["title"].as_str().unwrap_or("Untitled").to_owned(),
                model: v["model"].as_str().unwrap_or(crate::harness::models::DEFAULT).to_owned(),
                modified: SystemTime::UNIX_EPOCH + Duration::from_secs(v["updated"].as_u64().unwrap_or(0)),
                worktree: v["worktree"]["path"].as_str().map(|p| Worktree { path: PathBuf::from(p), branch: v["worktree"]["branch"].as_str().unwrap_or_default().to_owned() }),
                path,
            })
        })
        .collect();
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

/// A record's transcript.
pub fn load(path: &Path) -> Vec<Kind> {
    let Some(v) = std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) else { return Vec::new() };
    v["items"].as_array().into_iter().flatten().filter_map(json_kind).collect()
}

fn state_name(s: ToolState) -> &'static str {
    match s {
        ToolState::Running | ToolState::Failed => "failed",
        ToolState::Ok => "ok",
    }
}

fn kind_json(k: &Kind) -> Option<Value> {
    Some(match k {
        Kind::User(t) => json!({ "type": "user", "text": t }),
        Kind::Assistant { text, .. } => json!({ "type": "assistant", "text": text }),
        Kind::Tool { call, name, detail, state, output } => json!({ "type": "tool", "call": call, "name": name, "detail": detail, "state": state_name(*state), "output": output }),
        Kind::Error(t) => json!({ "type": "error", "text": t }),
        Kind::TurnEnd { secs, cost_usd, stopped } => json!({ "type": "turn-end", "secs": secs, "cost_usd": cost_usd, "stopped": stopped }),
    })
}

fn json_kind(v: &Value) -> Option<Kind> {
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_owned();
    Some(match v["type"].as_str()? {
        "user" => Kind::User(s("text")),
        "assistant" => Kind::Assistant { text: s("text"), streaming: false },
        "tool" => Kind::Tool { call: s("call"), name: s("name"), detail: s("detail"), state: if v["state"] == "ok" { ToolState::Ok } else { ToolState::Failed }, output: s("output") },
        "error" => Kind::Error(s("text")),
        "turn-end" => Kind::TurnEnd { secs: v["secs"].as_f64().unwrap_or(0.0), cost_usd: v["cost_usd"].as_f64(), stopped: v["stopped"].as_bool().unwrap_or(false) },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_round_trips() {
        let dir = std::env::temp_dir().join(format!("yoke-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut t = Thread::new(1, "Write the changelog".into(), Harness::Codex, "gpt-5".into());
        t.session = Some("abc/123".into());
        t.project = PathBuf::from("/work/app");
        for kind in [
            Kind::User("Write it".into()),
            Kind::Tool { call: "c1".into(), name: "Shell".into(), detail: "git log".into(), state: ToolState::Ok, output: "a1b2 fix".into() },
            Kind::Assistant { text: "Done.".into(), streaming: false },
            Kind::TurnEnd { secs: 4.5, cost_usd: None, stopped: false },
        ] {
            t.items.push(crate::app::Item { id: t.items.len() as u64, rev: 0, kind });
        }
        let text = record(&t).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(file(&dir, t.harness, "abc/123"), text).unwrap();

        assert!(list(&dir, Path::new("/work/other")).is_empty(), "another project's");
        let found = list(&dir, Path::new("/work/app"));
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].harness, found[0].session.as_str(), found[0].title.as_str(), found[0].model.as_str()), (Harness::Codex, "abc/123", "Write the changelog", "gpt-5"));
        let items = load(&found[0].path);
        assert_eq!(items.len(), 4);
        assert!(matches!(&items[1], Kind::Tool { state: ToolState::Ok, output, .. } if output == "a1b2 fix"));
        assert!(matches!(&items[3], Kind::TurnEnd { secs, stopped: false, .. } if *secs == 4.5));

        remove(&dir, Harness::Codex, "abc/123");
        assert!(list(&dir, Path::new("/work/app")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
