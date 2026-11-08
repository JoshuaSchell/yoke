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

use crate::app::{Agent, Item, Kind, ToolState};
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

/// The folders Claude Code has sessions for, most recently used first, read from each
/// session's `cwd` (the folder names it keeps them under can't be turned back into paths).
/// Folders that are gone, home itself, temporary folders and this app's worktrees are left out.
pub fn projects() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return Vec::new() };
    let Ok(dirs) = std::fs::read_dir(home.join(".claude/projects")) else { return Vec::new() };
    let mut found: Vec<(SystemTime, PathBuf)> = Vec::new();
    for d in dirs.flatten() {
        let Ok(files) = std::fs::read_dir(d.path()) else { continue };
        let newest = files.flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .max_by_key(|(t, _)| *t);
        let Some((when, file)) = newest else { continue };
        let Some(cwd) = session_cwd(&file) else { continue };
        let ours = crate::worktree::root().is_some_and(|r| cwd.starts_with(r));
        if !cwd.is_dir() || cwd == home || cwd.parent().is_none() || temporary(&cwd) || ours { continue; }
        if found.iter().any(|(_, p)| *p == cwd) { continue; }
        found.push((when, cwd));
    }
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    found.into_iter().map(|(_, p)| p).collect()
}

/// The folder the session ran in: the first line that says.
fn session_cwd(file: &Path) -> Option<PathBuf> {
    use std::io::BufRead;
    let f = std::io::BufReader::new(std::fs::File::open(file).ok()?);
    f.lines().map_while(Result::ok).take(200)
        .filter(|l| l.contains("\"cwd\""))
        .find_map(|l| serde_json::from_str::<Value>(&l).ok()?["cwd"].as_str().map(PathBuf::from))
}

fn temporary(p: &Path) -> bool {
    let tmp = std::env::temp_dir();
    let tmp = tmp.canonicalize().unwrap_or(tmp);
    p.starts_with(&tmp) || ["/tmp", "/private/tmp", "/var/folders", "/private/var/folders"].iter().any(|t| p.starts_with(t))
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
    out.sort_by_key(|x| std::cmp::Reverse(x.modified));
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
    prompt_in(v, false)
}

/// `prompt`, in a subagent's own transcript too (`sidechain`), whose lines are all marked so.
fn prompt_in(v: &Value, sidechain: bool) -> Option<String> {
    if v["type"] != "user" || v["isMeta"].as_bool() == Some(true) || (!sidechain && v["isSidechain"].as_bool() == Some(true)) { return None; }
    if notification(v).is_some() { return None; }
    let text = match &v["message"]["content"] {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => return None,
    };
    let t = text.trim();
    if t.is_empty() || t.starts_with("<command-") || t.starts_with("<local-command") || t.starts_with("Caveat:") { return None; }
    Some(t.to_owned())
}

/// A saved session, as `load` reads it.
#[derive(Default)]
pub struct Loaded {
    pub rows: Vec<Kind>,
    /// The model of its last reply (`claude-opus-5-5`), and the tokens in its context then.
    pub context: Option<(String, u64)>,
}

/// The transcript of session file `path`, as rows, and how full its context was at the end.
/// Subagents' cards get their own rows from their transcripts beside it
/// (`<id>/subagents/agent-<agent>.jsonl`).
pub fn load(path: &Path) -> Loaded {
    let Ok(file) = std::fs::read_to_string(path) else { return Loaded::default() };
    let subagents = path.with_extension("").join("subagents");
    let (rows, context) = rows(&file, false, &subagents, 0);
    Loaded { rows, context }
}

/// How deep subagents' transcripts are read (one a subagent started is one more).
const MAX_DEPTH: usize = 4;

/// A transcript's rows. `sidechain`: it's a subagent's own, whose lines are all marked so and
/// whose first message is what it was asked.
fn rows(file: &str, sidechain: bool, subagents: &Path, depth: usize) -> (Vec<Kind>, Option<(String, u64)>) {
    let mut out: Vec<Kind> = Vec::new();
    let mut context = None;
    // Agent calls and the subagent each one ran, from its result.
    let mut ran: Vec<(String, String)> = Vec::new();
    let mut asked = false;
    for line in file.lines() {
        if !(line.contains("\"type\":\"user\"") || line.contains("\"type\":\"assistant\"")) { continue; }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if !sidechain && v["isSidechain"].as_bool() == Some(true) { continue; }
        match v["type"].as_str() {
            Some("user") => {
                // A turn the CLI started itself, to report on background work: the divider
                // the app showed, not a message from the person.
                if let Some(n) = notification(&v) {
                    if let Some(call) = &n.call { finish_agent(&mut out, call, &n); }
                    out.push(Kind::Wake(n.summary));
                    continue;
                }
                if let Some(p) = prompt_in(&v, sidechain) {
                    // A subagent's first message is what it was asked (the card's title says it).
                    if sidechain && !asked { asked = true; continue; }
                    out.push(Kind::User(p));
                    continue;
                }
                for b in v["message"]["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_result") {
                    let call = b["tool_use_id"].as_str().unwrap_or_default();
                    let state = if b["is_error"].as_bool() == Some(true) { ToolState::Failed } else { ToolState::Ok };
                    if let Some(agent) = v["toolUseResult"]["agentId"].as_str() { ran.push((call.to_owned(), agent.to_owned())); }
                    set_result(&mut out, call, state, output_text(&b["content"]));
                }
            }
            Some("assistant") => {
                let model = v["message"]["model"].as_str().filter(|m| !m.starts_with('<'));
                if let (Some(m), Some(n)) = (model, crate::harness::claude::context_tokens(&v["message"]["usage"])) {
                    context = Some((m.to_owned(), n));
                }
                for b in v["message"]["content"].as_array().into_iter().flatten() {
                    match b["type"].as_str() {
                        Some("text") => {
                            let t = b["text"].as_str().unwrap_or_default();
                            if !t.trim().is_empty() { out.push(Kind::Assistant { text: t.to_owned(), streaming: false }); }
                        }
                        Some("tool_use") => {
                            let call = b["id"].as_str().unwrap_or_default().to_owned();
                            let name = b["name"].as_str().unwrap_or("tool").to_owned();
                            if crate::app::agent_tool(&name) {
                                // A call with no result in the file was cut off.
                                let agent = Agent { id: 0, call, description: tool_detail(&b["input"]), state: ToolState::Failed, output: String::new(), items: Vec::new() };
                                match out.last_mut() {
                                    Some(Kind::Agents(g)) => g.push(agent),
                                    _ => out.push(Kind::Agents(vec![agent])),
                                }
                            } else {
                                out.push(Kind::Tool {
                                    detail: tool_detail(&b["input"]),
                                    state: ToolState::Failed,
                                    output: String::new(),
                                    body: crate::harness::tool_body(&name, &b["input"]),
                                    call,
                                    name,
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if depth < MAX_DEPTH {
        for (call, agent) in ran {
            let Some(a) = agents_mut(&mut out).find(|a| a.call == call) else { continue };
            let Ok(text) = std::fs::read_to_string(subagents.join(format!("agent-{agent}.jsonl"))) else { continue };
            let (inner, _) = rows(&text, true, subagents, depth + 1);
            a.items = inner.into_iter().map(|kind| Item { id: 0, rev: 0, kind }).collect();
        }
    }
    (out, context)
}

/// Every subagent card in `rows` (not those inside cards).
fn agents_mut(rows: &mut [Kind]) -> impl Iterator<Item = &mut Agent> {
    rows.iter_mut().filter_map(|k| match k { Kind::Agents(g) => Some(g), _ => None }).flatten()
}

/// A tool call's result: on its row or its subagent's card.
fn set_result(rows: &mut [Kind], call: &str, st: ToolState, out: String) {
    if let Some(Kind::Tool { state, output, .. }) = rows.iter_mut().rev().find(|k| matches!(k, Kind::Tool { call: c, .. } if c == call)) {
        *state = st;
        *output = out;
        return;
    }
    if let Some(a) = agents_mut(rows).find(|a| a.call == call) {
        a.state = st;
        a.output = out;
    }
}

/// A background agent's end, from the notification the CLI woke on: its call returned at
/// once ("launched"), so this is where it finished, or not.
fn finish_agent(rows: &mut [Kind], call: &str, n: &Notification) {
    let Some(a) = agents_mut(rows).find(|a| a.call == call) else { return };
    a.state = if n.status == "completed" { ToolState::Ok } else { ToolState::Failed };
    if let Some(r) = &n.result { a.output = r.clone(); }
}

/// A `<task-notification>` the CLI sends itself when background work ends.
struct Notification {
    call: Option<String>,
    status: String,
    summary: String,
    result: Option<String>,
}

fn notification(v: &Value) -> Option<Notification> {
    let text = match &v["message"]["content"] {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => return None,
    };
    let from_task = v["origin"]["kind"] == "task-notification" || text.trim_start().starts_with("<task-notification>");
    if !from_task { return None; }
    let tag = |name: &str| -> Option<String> {
        let open = format!("<{name}>");
        let start = text.find(&open)? + open.len();
        let end = text[start..].find(&format!("</{name}>"))? + start;
        Some(text[start..end].trim().to_owned())
    };
    Some(Notification {
        call: tag("tool-use-id"),
        status: tag("status").unwrap_or_default(),
        summary: tag("summary").unwrap_or_else(|| "Background work finished".to_owned()),
        result: tag("result"),
    })
}

/// Gives every row inside subagents' cards an id of its own, counting on from `next`.
pub fn number(kind: &mut Kind, next: &mut u64) {
    if let Kind::Agents(g) = kind {
        for a in g {
            *next += 1;
            a.id = *next;
            for i in &mut a.items {
                *next += 1;
                i.id = *next;
                number(&mut i.kind, next);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_a_session_takes_its_files_and_nothing_else() {
        let home = std::env::temp_dir().join(format!("yoke-remove-{}", std::process::id()));
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
            r#"{"type":"assistant","message":{"model":"claude-opus-5-5","usage":{"input_tokens":10,"cache_read_input_tokens":90,"output_tokens":5},"content":[{"type":"text","text":"Fixed."}]}}"#,
            r#"{"type":"assistant","message":{"model":"<synthetic>","usage":{"input_tokens":0},"content":[]}}"#,
        ].join("\n");
        assert_eq!(title(&file).as_deref(), Some("Fix the build"));
        let titled = format!("{file}\n{}", r#"{"type":"ai-title","aiTitle":"Build fix"}"#);
        assert_eq!(title(&titled).as_deref(), Some("Build fix"));
        assert_eq!(title(r#"{"type":"mode"}"#), None, "no prompt, no thread");

        let path = std::env::temp_dir().join(format!("yoke-history-{}.jsonl", std::process::id()));
        std::fs::write(&path, &file).unwrap();
        let loaded = load(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(loaded.context, Some(("claude-opus-5-5".to_owned(), 105)), "the last real reply's context");
        let rows = loaded.rows;
        assert!(matches!(&rows[0], Kind::User(p) if p == "Fix the build"));
        assert!(matches!(&rows[2], Kind::Tool { state: ToolState::Ok, output, .. } if output == "ok"));
        assert_eq!(rows.len(), 4);
    }

    /// A saved session with a subagent: its card gets its own transcript's rows, and the
    /// notification the CLI woke on after background work is a divider, not a message.
    #[test]
    fn subagents_and_wakes_come_back() {
        let dir = std::env::temp_dir().join(format!("yoke-history-agents-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("s1/subagents")).unwrap();
        let main = [
            r#"{"type":"user","message":{"role":"user","content":"Count the files"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"a1","name":"Agent","input":{"description":"Count files","prompt":"ls | wc -l"}}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"a2","name":"Agent","input":{"description":"Slow echo","prompt":"sleep","run_in_background":true}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"a1","content":"The report follows:\n  485\nagentId: x1 (use SendMessage)\n<usage>tool_uses: 1</usage>"}]},"toolUseResult":{"agentId":"x1","status":"completed"}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"a2","content":"Async agent launched successfully."}]},"toolUseResult":{"agentId":"x2","isAsync":true}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"STARTED"}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":"<task-notification>\n<task-id>x2</task-id>\n<tool-use-id>a2</tool-use-id>\n<status>completed</status>\n<summary>Agent \"Slow echo\" completed</summary>\n<result>SLOW_DONE</result>\n</task-notification>"},"origin":{"kind":"task-notification"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"FINISHED"}]}}"#,
        ].join("\n");
        let sub = [
            r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"ls | wc -l"}}"#,
            r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"ls | wc -l"}}]}}"#,
            r#"{"type":"user","isSidechain":true,"message":{"content":[{"type":"tool_result","tool_use_id":"b1","content":"485"}]}}"#,
            r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"485"}]}}"#,
        ].join("\n");
        std::fs::write(dir.join("s1.jsonl"), &main).unwrap();
        std::fs::write(dir.join("s1/subagents/agent-x1.jsonl"), &sub).unwrap();
        let rows = load(&dir.join("s1.jsonl")).rows;
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(title(&main).as_deref(), Some("Count the files"));
        let Kind::Agents(g) = &rows[1] else { panic!("one group of cards") };
        assert_eq!(g.len(), 2);
        assert_eq!(crate::app::agent_report(&g[0].output), "485");
        assert_eq!(g[0].items.len(), 2, "its Bash row and its reply, not what it was asked");
        assert!(matches!(&g[0].items[0].kind, Kind::Tool { state: ToolState::Ok, output, .. } if output == "485"));
        assert_eq!((g[1].state, g[1].output.as_str()), (ToolState::Ok, "SLOW_DONE"), "the background agent's end, from its notification");
        assert!(matches!(&rows[3], Kind::Wake(w) if w == "Agent \"Slow echo\" completed"));
        assert_eq!(rows.iter().filter(|k| matches!(k, Kind::User(_))).count(), 1);
    }
}
