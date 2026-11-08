//! Claude Code's own saved sessions for the project (`~/.claude/projects/<folder>/<id>.jsonl`),
//! so threads started in the terminal, or in an earlier run of the app, show in the sidebar and
//! can be continued with `--resume`.
//!
//! Listing reads each file once for its title (the CLI's `ai-title`, else the first prompt) and
//! skips files with no prompt in them (the model-listing handshake leaves those). A thread's
//! transcript is read when it's first opened.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::app::{Kind, ToolState};
use crate::harness::{first_line, output_text, tool_detail};

pub struct Session {
    pub id: String,
    pub title: String,
    pub modified: SystemTime,
    pub path: PathBuf,
}

/// Claude Code's own folder: `$CLAUDE_CONFIG_DIR` when set, as the CLI reads it, else `~/.claude`.
pub fn claude_home() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) { return Some(PathBuf::from(d)); }
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".claude"))
}

/// Where Claude Code keeps `project`'s sessions: the path with every character that isn't a
/// letter or digit turned into `-` (`/Users/josh/rune` → `-Users-josh-rune`).
pub fn dir(project: &Path) -> Option<PathBuf> {
    Some(project_dir(&claude_home()?, project))
}

fn project_dir(home: &Path, project: &Path) -> PathBuf {
    let name: String = project.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    home.join("projects").join(name)
}

/// Everything Claude Code keeps for session `id` of `project` under `home`: the transcript, the
/// folder beside it (subagents, tool results), its file-edit snapshots and its shell
/// environment. Empty for an id that isn't a plain session id, so a bad one can't reach
/// anything else.
pub fn session_files(home: &Path, project: &Path, id: &str) -> Vec<PathBuf> {
    let plain = !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !plain { return Vec::new(); }
    let dir = project_dir(home, project);
    vec![dir.join(format!("{id}.jsonl")), dir.join(id), home.join("file-history").join(id), home.join("session-env").join(id)]
}

/// Deletes session `id`'s files (`session_files`). A symlink goes, not what it points at.
/// Returns whether nothing is left of them.
pub fn remove(home: &Path, project: &Path, id: &str) -> bool {
    let mut gone = true;
    for path in session_files(home, project, id) {
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        let removed = if meta.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        if removed.is_err() && std::fs::symlink_metadata(&path).is_ok() { gone = false; }
    }
    gone
}

/// The project's sessions, newest first.
pub fn list(project: &Path) -> Vec<Session> {
    let Some(dir) = dir(project) else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out: Vec<Session> = entries.flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| {
            let path = e.path();
            let modified = e.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            let id = path.file_stem()?.to_string_lossy().into_owned();
            let title = title(&std::fs::read_to_string(&path).ok()?)?;
            Some(Session { id, title, modified, path })
        })
        .collect();
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

/// The session's title: its last `ai-title`, else its first prompt. `None` with no prompt.
fn title(file: &str) -> Option<String> {
    let (mut ai, mut first) = (None, None);
    for line in file.lines() {
        // Cheap checks first: most lines are tool output and API records.
        if line.contains("\"type\":\"ai-title\"") {
            if let Some(t) = serde_json::from_str::<Value>(line).ok().and_then(|v| v["aiTitle"].as_str().map(str::to_owned)) { ai = Some(t); }
        } else if first.is_none() && line.contains("\"type\":\"user\"")
            && let Ok(v) = serde_json::from_str::<Value>(line)
            && let Some(p) = prompt(&v) {
                first = Some(first_line(&p, 80));
            }
    }
    let first = first?;
    Some(ai.unwrap_or(first))
}

/// The person's words in a `user` line: not a tool result, a meta line or a slash-command echo.
fn prompt(v: &Value) -> Option<String> {
    if v["type"] != "user" || v["isMeta"].as_bool() == Some(true) || v["isSidechain"].as_bool() == Some(true) { return None; }
    let text = match &v["message"]["content"] {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => return None,
    };
    let t = text.trim();
    if t.is_empty() || t.starts_with("<command-") || t.starts_with("<local-command") || t.starts_with("Caveat:") { return None; }
    Some(t.to_owned())
}

/// The transcript of session file `path`, as rows.
pub fn load(path: &Path) -> Vec<Kind> {
    let Ok(file) = std::fs::read_to_string(path) else { return Vec::new() };
    let mut out: Vec<Kind> = Vec::new();
    for line in file.lines() {
        if !(line.contains("\"type\":\"user\"") || line.contains("\"type\":\"assistant\"")) { continue; }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v["isSidechain"].as_bool() == Some(true) { continue; }
        match v["type"].as_str() {
            Some("user") => {
                if let Some(p) = prompt(&v) {
                    out.push(Kind::User(p));
                    continue;
                }
                for b in v["message"]["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_result") {
                    let call = b["tool_use_id"].as_str().unwrap_or_default();
                    let row = out.iter_mut().rev().find(|k| matches!(k, Kind::Tool { call: c, .. } if c == call));
                    if let Some(Kind::Tool { state, output, .. }) = row {
                        *state = if b["is_error"].as_bool() == Some(true) { ToolState::Failed } else { ToolState::Ok };
                        *output = output_text(&b["content"]);
                    }
                }
            }
            Some("assistant") => {
                for b in v["message"]["content"].as_array().into_iter().flatten() {
                    match b["type"].as_str() {
                        Some("text") => {
                            let t = b["text"].as_str().unwrap_or_default();
                            if !t.trim().is_empty() { out.push(Kind::Assistant { text: t.to_owned(), streaming: false }); }
                        }
                        Some("tool_use") => out.push(Kind::Tool {
                            call: b["id"].as_str().unwrap_or_default().to_owned(),
                            name: b["name"].as_str().unwrap_or("tool").to_owned(),
                            detail: tool_detail(&b["input"]),
                            // A call with no result in the file was cut off.
                            state: ToolState::Failed,
                            output: String::new(),
                        }),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_a_session_takes_its_files_and_nothing_else() {
        let home = std::env::temp_dir().join(format!("rune-code-remove-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let project = Path::new("/Users/josh/my.app");
        let id = "88998ad8-25b9-43da-99f0-81bfef07d76f";
        let dir = project_dir(&home, project);
        for d in [dir.join(id).join("subagents"), home.join("file-history").join(id), home.join("session-env").join(id), dir.join("memory")] {
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("f"), "x").unwrap();
        }
        std::fs::write(dir.join(format!("{id}.jsonl")), "{}").unwrap();
        std::fs::write(dir.join("other.jsonl"), "{}").unwrap();

        assert!(session_files(&home, project, "../../etc").is_empty() && session_files(&home, project, "").is_empty());
        assert!(remove(&home, project, id));
        assert!(session_files(&home, project, id).iter().all(|p| !p.exists()));
        assert!(dir.join("other.jsonl").exists() && dir.join("memory/f").exists(), "the project's other files stay");
        assert!(remove(&home, project, id), "nothing left is still done");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn project_folder_name() {
        let d = project_dir(Path::new("/h/.claude"), Path::new("/Users/josh/my.app"));
        assert_eq!(d, Path::new("/h/.claude/projects/-Users-josh-my-app"));
    }

    #[test]
    fn titles_and_transcripts() {
        let file = [
            r#"{"type":"permission-mode","permissionMode":"auto"}"#,
            r#"{"type":"user","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"Fix the build"},"isSidechain":false}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Looking."},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo build"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok","is_error":false}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Fixed."}]}}"#,
        ].join("\n");
        assert_eq!(title(&file).as_deref(), Some("Fix the build"));
        let titled = format!("{file}\n{}", r#"{"type":"ai-title","aiTitle":"Build fix"}"#);
        assert_eq!(title(&titled).as_deref(), Some("Build fix"));
        assert_eq!(title(r#"{"type":"mode"}"#), None, "no prompt, no thread");

        let path = std::env::temp_dir().join(format!("rune-code-history-{}.jsonl", std::process::id()));
        std::fs::write(&path, &file).unwrap();
        let rows = load(&path);
        let _ = std::fs::remove_file(&path);
        assert!(matches!(&rows[0], Kind::User(p) if p == "Fix the build"));
        assert!(matches!(&rows[2], Kind::Tool { state: ToolState::Ok, output, .. } if output == "ok"));
        assert_eq!(rows.len(), 4);
    }
}
