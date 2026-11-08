//! Claude Code's own saved sessions for the project (`~/.claude/projects/<folder>/<id>.jsonl`),
//! so threads started in the terminal, or in an earlier run of the app, show in the sidebar and
//! can be continued with `--resume`.
//!
//! Listing reads each file once for its title (the CLI's `ai-title`, else the first prompt) and
//! skips files with no prompt in them (the model-listing handshake leaves those). A thread's
//! transcript is read when it's first opened.

use std::collections::HashMap;
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
    if t.is_empty() || t.starts_with("<command-") || t.starts_with("<local-command") || t.starts_with("Caveat:") || t.starts_with("<system-reminder>") { return None; }
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
/// (`<id>/subagents/agent-<agent>.jsonl`): an agent resumed with `SendMessage` has a card for
/// each run, where its call was made, and its transcript is split between them.
pub fn load(path: &Path) -> Loaded {
    let Ok(file) = std::fs::read_to_string(path) else { return Loaded::default() };
    let subagents = path.with_extension("").join("subagents");
    let mut calls = first_calls(&subagents);
    let (runs, context, _) = rows(&file, false, &mut calls);
    let mut rows: Vec<Kind> = runs.into_iter().flat_map(|r| r.rows).collect();
    let mut loaded = HashMap::new();
    fill(&mut rows, &subagents, &mut calls, &mut loaded, 0);
    Loaded { rows, context }
}

/// Where Claude Code keeps the subagents' transcripts of session `session` run in `project`.
pub fn subagents_dir(home: &Path, project: &Path, session: &str) -> PathBuf {
    project_dir(home, project).join(session).join("subagents")
}

/// The `Agent` call that first started agent `agent`, from the `meta.json` Claude Code keeps
/// beside its transcript.
pub fn agent_first_call(subagents: &Path, agent: &str) -> Option<String> {
    agent_meta(subagents, agent)["toolUseId"].as_str().map(str::to_owned)
}

/// Agent `agent`'s saved runs, oldest first, as rows (`subagents` is `subagents_dir`).
pub fn agent_runs(subagents: &Path, agent: &str) -> Vec<Run> {
    if !plain_id(agent) { return Vec::new(); }
    let Ok(text) = std::fs::read_to_string(subagents.join(format!("agent-{agent}.jsonl"))) else { return Vec::new() };
    let mut calls = first_calls(subagents);
    let (mut runs, ..) = rows(&text, true, &mut calls);
    let mut loaded = HashMap::new();
    for r in &mut runs { fill(&mut r.rows, subagents, &mut calls, &mut loaded, 1); }
    runs
}

fn plain_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Every subagent's first call, by call: each `agent-<id>.meta.json` names its `toolUseId`.
/// (A subagent's own transcript doesn't say which agent its `Agent` calls started.)
fn first_calls(subagents: &Path) -> HashMap<String, String> {
    let Ok(dir) = std::fs::read_dir(subagents) else { return HashMap::new() };
    dir.flatten().filter_map(|e| {
        let name = e.file_name().to_string_lossy().into_owned();
        let agent = name.strip_prefix("agent-")?.strip_suffix(".meta.json")?.to_owned();
        let meta: Value = serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
        Some((meta["toolUseId"].as_str()?.to_owned(), agent))
    }).collect()
}

/// A subagent's transcript as `fill` gives it out: its runs, the next to give, what it does
/// and its model.
struct AgentFile {
    runs: Vec<Run>,
    next: usize,
    description: String,
    model: Option<String>,
}

/// An agent's `meta.json` (null if there's none).
fn agent_meta(subagents: &Path, agent: &str) -> Value {
    if !plain_id(agent) { return Value::Null; }
    std::fs::read_to_string(subagents.join(format!("agent-{agent}.meta.json"))).ok().and_then(|m| serde_json::from_str(&m).ok()).unwrap_or(Value::Null)
}

/// How deep subagents' transcripts are read (one a subagent started is one more).
const MAX_DEPTH: usize = 4;

/// One run of a subagent, as its transcript holds it: what it was asked (its first message, or
/// the message that resumed it), and its rows.
#[derive(Default)]
pub struct Run {
    pub prompt: Option<String>,
    pub rows: Vec<Kind>,
}

/// Gives each subagent card in `rows` (and in theirs, down to `MAX_DEPTH`) its agent and its
/// run's rows. An agent's runs go to its cards in order: its `Agent` call's card takes the
/// first, each resume's card the next. `loaded` keeps each agent's runs not yet given out.
fn fill(out: &mut [Kind], subagents: &Path, calls: &mut HashMap<String, String>, loaded: &mut HashMap<String, AgentFile>, depth: usize) {
    if depth >= MAX_DEPTH { return; }
    for k in out.iter_mut() {
        let Kind::Agents(g) = k else { continue };
        for a in g.iter_mut() {
            if a.task.is_none() { a.task = calls.get(&a.call).cloned(); }
            let Some(agent) = a.task.clone() else { continue };
            let f = loaded.entry(agent.clone()).or_insert_with(|| {
                let text = if plain_id(&agent) { std::fs::read_to_string(subagents.join(format!("agent-{agent}.jsonl"))).unwrap_or_default() } else { String::new() };
                let (runs, _, model) = rows(&text, true, calls);
                let description = agent_meta(subagents, &agent)["description"].as_str().unwrap_or_default().to_owned();
                AgentFile { runs, next: 0, description, model }
            });
            // A resume names the agent, not what it does: the card says what its first did.
            if a.description.is_empty() { a.description = f.description.clone(); } else if f.description.is_empty() { f.description = a.description.clone(); }
            a.model = f.model.clone();
            // A resume whose first run isn't here (made before this transcript) takes the run
            // after it.
            if f.next == 0 && a.resumed { f.next = 1.min(f.runs.len().saturating_sub(1)); }
            let Some(run) = f.runs.get_mut(f.next) else { continue };
            f.next += 1;
            let mut inner = std::mem::take(&mut run.rows);
            a.prompt = run.prompt.take();
            fill(&mut inner, subagents, calls, loaded, depth + 1);
            a.items = inner.into_iter().map(|kind| Item { id: 0, rev: 0, kind }).collect();
        }
    }
}

/// A transcript's runs (one, for a session's own; one per run for a subagent's), how full its
/// context was at the end, and the model of its last reply. `sidechain`:
/// it's a subagent's own, whose lines are all marked so, whose first message is what it was
/// asked, and where a message from the agent that started it (or another) starts a run.
/// `calls` gets the agents its `Agent` calls started, as their results say.
fn rows(file: &str, sidechain: bool, calls: &mut HashMap<String, String>) -> (Vec<Run>, Option<(String, u64)>, Option<String>) {
    let mut runs = vec![Run::default()];
    let mut context = None;
    let mut last_model = None;
    let mut asked = false;
    for line in file.lines() {
        if !(line.contains("\"type\":\"user\"") || line.contains("\"type\":\"assistant\"")) { continue; }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if !sidechain && v["isSidechain"].as_bool() == Some(true) { continue; }
        let run = runs.len() - 1;
        let out = &mut runs[run].rows;
        match v["type"].as_str() {
            Some("user") => {
                // A turn the CLI started itself, to report on background work: the divider
                // the app showed, not a message from the person.
                if let Some(n) = notification(&v) {
                    if let Some(call) = &n.call { finish_agent(out, call, &n); }
                    out.push(Kind::Wake(n.summary));
                    continue;
                }
                // A resume: the message another agent sent this one starts its next run.
                if sidechain && let Some(m) = resume_message(&v) {
                    runs.push(Run { prompt: Some(m), rows: Vec::new() });
                    continue;
                }
                if let Some(p) = prompt_in(&v, sidechain) {
                    // A subagent's first message is what it was asked (its card shows it).
                    if sidechain && !asked {
                        asked = true;
                        runs[run].prompt = Some(p);
                        continue;
                    }
                    out.push(Kind::User(p));
                    continue;
                }
                for b in v["message"]["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_result") {
                    let call = b["tool_use_id"].as_str().unwrap_or_default();
                    let state = if b["is_error"].as_bool() == Some(true) { ToolState::Failed } else { ToolState::Ok };
                    let text = output_text(&b["content"]);
                    if let Some(agent) = v["toolUseResult"]["agentId"].as_str() { calls.insert(call.to_owned(), agent.to_owned()); }
                    // A `SendMessage` that resumed an agent: that agent's next run, its card here.
                    let resumed = v["toolUseResult"]["resumedAgentId"].as_str().map(str::to_owned)
                        .or_else(|| serde_json::from_str::<Value>(text.trim()).ok().and_then(|j| j["resumedAgentId"].as_str().map(str::to_owned)));
                    if let Some(agent) = resumed { resume_card(out, call, &agent); }
                    set_result(out, call, state, text);
                }
            }
            Some("assistant") => {
                let model = v["message"]["model"].as_str().filter(|m| !m.starts_with('<'));
                if let Some(m) = model { last_model = Some(m.to_owned()); }
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
                                let agent = Agent::new(0, call, tool_detail(&b["input"]), ToolState::Failed);
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
    (runs, context, last_model)
}

/// The message that resumed a subagent, from its transcript's line for it: one from the agent
/// that started it (`coordinator`) or another (`peer`), without the CLI's frame around it.
fn resume_message(v: &Value) -> Option<String> {
    let kind = v["origin"]["kind"].as_str()?;
    if !matches!(kind, "coordinator" | "peer") { return None; }
    if let Some(body) = v["origin"]["body"].as_str().filter(|b| !b.trim().is_empty()) { return Some(body.trim().to_owned()); }
    let text = match &v["message"]["content"] {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => return None,
    };
    Some(crate::app::resume_text(&text))
}

/// The tool row of `SendMessage` call `call`, which resumed agent `agent`, as that run's card.
fn resume_card(rows: &mut [Kind], call: &str, agent: &str) {
    let Some(k) = rows.iter_mut().rev().find(|k| matches!(k, Kind::Tool { call: c, .. } if c == call)) else { return };
    let mut a = Agent::new(0, call.to_owned(), String::new(), ToolState::Ok);
    a.task = Some(agent.to_owned());
    a.resumed = true;
    *k = Kind::Agents(vec![a]);
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

    /// A saved session where an agent was resumed with SendMessage (as 2.1.319 saves it): a
    /// card for each run, where its call was made, each with its own part of the agent's
    /// transcript and what it was asked; an agent it started (known by its `meta.json`, since a
    /// subagent's transcript doesn't say) gets its rows too.
    #[test]
    fn a_resumed_agent_comes_back_as_a_card_per_run() {
        let dir = std::env::temp_dir().join(format!("yoke-history-resume-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("s1/subagents")).unwrap();
        let main = [
            r#"{"type":"user","message":{"role":"user","content":"Start a helper"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"a1","name":"Agent","input":{"description":"Echo helper","prompt":"echo ONE","run_in_background":true}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"a1","content":"Async agent launched successfully."}]},"toolUseResult":{"agentId":"x1","isAsync":true}}"#,
            r#"{"type":"user","message":{"role":"user","content":"<task-notification>\n<task-id>x1</task-id>\n<tool-use-id>a1</tool-use-id>\n<status>completed</status>\n<summary>Agent \"Echo helper\" completed</summary>\n<result>ONE</result>\n</task-notification>"},"origin":{"kind":"task-notification"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"Send it another"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"s1","name":"SendMessage","input":{"to":"x1","message":"echo TWO"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"s1","content":"{\"success\":true,\"message\":\"Resuming agent x1\",\"resumedAgentId\":\"x1\"}"}]},"toolUseResult":{"success":true,"resumedAgentId":"x1"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"<task-notification>\n<task-id>x1</task-id>\n<tool-use-id>s1</tool-use-id>\n<status>completed</status>\n<summary>Agent \"Echo helper\" completed</summary>\n<result>TWO</result>\n</task-notification>"},"origin":{"kind":"task-notification"}}"#,
        ].join("\n");
        let sub = [
            r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"echo ONE"}}"#,
            r#"{"type":"assistant","isSidechain":true,"message":{"model":"claude-haiku-5-5","content":[{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"echo ONE"}}]}}"#,
            r#"{"type":"user","isSidechain":true,"message":{"content":[{"type":"tool_result","tool_use_id":"b1","content":"ONE"}]}}"#,
            r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"<system-reminder>\n<tools-used-before-summary>Bash</tools-used-before-summary>\n</system-reminder>"}}"#,
            r#"{"type":"user","isSidechain":true,"origin":{"kind":"coordinator"},"message":{"role":"user","content":"The coordinator sent a message while you were working:\necho TWO\n\nAddress this before completing your current task."}}"#,
            r#"{"type":"assistant","isSidechain":true,"message":{"model":"claude-haiku-5-5","content":[{"type":"tool_use","id":"n1","name":"Agent","input":{"description":"Inner helper","prompt":"echo TWO"}}]}}"#,
            r#"{"type":"user","isSidechain":true,"message":{"content":[{"type":"tool_result","tool_use_id":"n1","content":"TWO"}]}}"#,
            r#"{"type":"assistant","isSidechain":true,"message":{"model":"claude-haiku-5-5","content":[{"type":"text","text":"TWO"}]}}"#,
        ].join("\n");
        let inner = [
            r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"echo TWO"}}"#,
            r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"tool_use","id":"c1","name":"Bash","input":{"command":"echo TWO"}}]}}"#,
            r#"{"type":"user","isSidechain":true,"message":{"content":[{"type":"tool_result","tool_use_id":"c1","content":"TWO"}]}}"#,
        ].join("\n");
        std::fs::write(dir.join("s1.jsonl"), &main).unwrap();
        std::fs::write(dir.join("s1/subagents/agent-x1.jsonl"), &sub).unwrap();
        std::fs::write(dir.join("s1/subagents/agent-x1.meta.json"), r#"{"description":"Echo helper","toolUseId":"a1"}"#).unwrap();
        std::fs::write(dir.join("s1/subagents/agent-y1.jsonl"), &inner).unwrap();
        std::fs::write(dir.join("s1/subagents/agent-y1.meta.json"), r#"{"description":"Inner helper","toolUseId":"n1","parentAgentId":"x1"}"#).unwrap();
        let rows = load(&dir.join("s1.jsonl")).rows;
        let runs = agent_runs(&dir.join("s1/subagents"), "x1");
        let _ = std::fs::remove_dir_all(&dir);
        let cards: Vec<&Agent> = rows.iter().filter_map(|k| match k { Kind::Agents(g) => Some(g), _ => None }).flatten().collect();
        assert_eq!(cards.len(), 2, "a card per run, no SendMessage row");
        assert!(!rows.iter().any(|k| matches!(k, Kind::Tool { .. })));
        let (one, two) = (cards[0], cards[1]);
        assert_eq!((one.task.as_deref(), one.resumed, two.task.as_deref(), two.resumed), (Some("x1"), false, Some("x1"), true));
        assert_eq!((two.description.as_str(), two.call.as_str()), ("Echo helper", "s1"));
        assert_eq!((one.prompt.as_deref(), two.prompt.as_deref()), (Some("echo ONE"), Some("echo TWO")));
        assert_eq!(one.items.len(), 1, "its Bash row; the reminder isn't a message");
        assert_eq!((two.state, two.output.as_str()), (ToolState::Ok, "TWO"), "the resumed run's end, from its notification");
        assert_eq!(two.model.as_deref(), Some("claude-haiku-5-5"));
        let Kind::Agents(g) = &two.items[0].kind else { panic!("the inner agent's card") };
        assert_eq!((g[0].task.as_deref(), g[0].items.len()), (Some("y1"), 1), "its own Bash row, from its transcript");
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[1].prompt.as_deref(), Some("echo TWO"));
    }
}
