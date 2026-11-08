//! Coding-agent harnesses: each one is a CLI that takes a prompt and streams JSON lines.
//!
//! A turn spawns the CLI in the project folder, reads its stdout on a thread, turns each line
//! into [`Event`]s with the harness's parser and sends them to the UI thread, waking it. Nothing
//! here touches the UI; `poll` drains the channel.

pub(crate) mod claude;
mod codex;
pub mod limits;
pub mod models;
mod opencode;

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use rune::Waker;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Harness {
    Claude,
    Codex,
    OpenCode,
}

/// The effort picker's no-flag choice (named apart from the model picker's "Default").
pub const DEFAULT_EFFORT: &str = "Default effort";

pub const ALL: [Harness; 3] = [Harness::Claude, Harness::Codex, Harness::OpenCode];

/// How much the agent may do without asking. Claude Code asks the app about anything beyond
/// this (an approval card in the transcript). The other CLIs can't ask in their one-shot
/// modes, so for them this decides up front, and "Ask first" means read only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Access {
    ReadOnly,
    /// Reads go through; edits and commands ask.
    Ask,
    /// Edits go through; commands and everything else ask.
    Edits,
    Full,
}

pub const ACCESS: [Access; 4] = [Access::ReadOnly, Access::Ask, Access::Edits, Access::Full];

impl Access {
    pub fn label(self) -> &'static str {
        match self {
            Access::ReadOnly => "Read only",
            Access::Ask => "Ask first",
            Access::Edits => "Edit files",
            Access::Full => "Full access",
        }
    }
    pub fn from_label(l: &str) -> Option<Access> {
        ACCESS.into_iter().find(|a| a.label() == l)
    }
    /// Claude Code's `--permission-mode` for this level.
    pub fn claude_mode(self) -> &'static str {
        match self {
            Access::ReadOnly => "plan",
            Access::Ask => "default",
            Access::Edits => "acceptEdits",
            Access::Full => "bypassPermissions",
        }
    }
    /// Whether this level lets `tool` run without asking.
    pub fn allows(self, tool: &str) -> bool {
        match self {
            Access::Full => true,
            Access::Edits => matches!(tool, "Edit" | "Write" | "MultiEdit" | "NotebookEdit"),
            Access::ReadOnly | Access::Ask => false,
        }
    }
    /// What `h` actually gets: harnesses that can't ask treat "Ask first" as read only.
    pub fn for_harness(self, h: Harness) -> Access {
        if self == Access::Ask && !h.asks() { Access::ReadOnly } else { self }
    }
}

impl Harness {
    pub fn name(self) -> &'static str {
        match self {
            Harness::Claude => "Claude Code",
            Harness::Codex => "Codex",
            Harness::OpenCode => "OpenCode",
        }
    }
    pub fn binary(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::OpenCode => "opencode",
        }
    }
    /// The provider icon (`rune_theme_opencode::provider_icons::named`). Claude draws its own
    /// spark instead (`ui::sidebar::CLAUDE`).
    pub fn provider(self) -> &'static str {
        match self {
            Harness::Claude => "anthropic",
            Harness::Codex => "openai",
            Harness::OpenCode => "opencode",
        }
    }
    /// A short stable name for settings keys.
    pub fn id(self) -> &'static str {
        self.binary()
    }

    /// Whether the CLI asks the app before a tool runs (so approvals show as cards), rather
    /// than refusing whatever the access level doesn't allow.
    pub fn asks(self) -> bool {
        matches!(self, Harness::Claude)
    }

    /// Reasoning effort levels offered in the picker; the first is the CLI's own default (no
    /// flag). Empty for harnesses with no effort flag, which hides the picker.
    pub fn efforts(self) -> &'static [&'static str] {
        match self {
            Harness::Claude => &[DEFAULT_EFFORT, "low", "medium", "high", "xhigh", "max"],
            Harness::Codex => &[DEFAULT_EFFORT, "minimal", "low", "medium", "high"],
            Harness::OpenCode => &[],
        }
    }

    pub(crate) fn command(self, bin: &Path, t: &Turn) -> Command {
        let mut c = Command::new(bin);
        let model = (t.model != models::DEFAULT).then_some(t.model.as_str());
        let effort = (t.effort != DEFAULT_EFFORT && self.efforts().contains(&t.effort.as_str())).then_some(t.effort.as_str());
        let access = t.access.for_harness(self);
        match self {
            Harness::Claude => {
                // Two-way stream-json, as the Agent SDK drives it: the prompt goes in on stdin
                // (`stdin_lines`), and `--permission-prompt-tool stdio` makes the CLI ask us
                // (`can_use_tool` control requests) instead of refusing.
                c.args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--include-partial-messages"]);
                // Read only stays read only: plan mode refuses rather than asking (it would
                // otherwise ask to leave plan mode and start editing).
                if access != Access::ReadOnly { c.args(["--permission-prompt-tool", "stdio"]); }
                c.args(["--permission-mode", access.claude_mode()]);
                // Lets the access picker switch a running turn to Full access
                // (`Running::set_access`); without it the CLI refuses bypassPermissions mid-session.
                c.arg("--allow-dangerously-skip-permissions");
                if let Some(m) = model { c.args(["--model", m]); }
                if let Some(e) = effort { c.args(["--effort", e]); }
                if let Some(id) = &t.resume { c.args(["--resume", id]); }
                let inbound = if t.peer_messages { "accept" } else { "refuse" };
                c.args(["--settings", &format!(r#"{{"crossSessionInbound":"{inbound}"}}"#)]);
            }
            Harness::Codex => {
                c.args(["exec", "--json", "--skip-git-repo-check"]);
                match access {
                    Access::ReadOnly | Access::Ask => { c.args(["--sandbox", "read-only"]); }
                    Access::Edits => { c.arg("--full-auto"); }
                    Access::Full => { c.arg("--dangerously-bypass-approvals-and-sandbox"); }
                }
                if let Some(m) = model { c.args(["--model", m]); }
                if let Some(e) = effort { c.args(["-c", &format!("model_reasoning_effort=\"{e}\"")]); }
                for img in &t.images { c.arg("--image").arg(img); }
                if let Some(id) = &t.resume { c.args(["resume", id]); }
                c.arg(&t.prompt);
            }
            Harness::OpenCode => {
                c.args(["run", "--format", "json"]);
                if let Some(m) = model { c.args(["--model", m]); }
                if let Some(id) = &t.resume { c.args(["--session", id]); }
                // No image flag we've confirmed: name the files so the agent can read them.
                if t.images.is_empty() {
                    c.arg(&t.prompt);
                } else {
                    let paths: Vec<String> = t.images.iter().map(|p| p.display().to_string()).collect();
                    c.arg(format!("{}\n\nAttached images: {}", t.prompt, paths.join(", ")));
                }
            }
        }
        c
    }

    /// What goes to the CLI's stdin when the turn starts; empty for CLIs that take the prompt
    /// as an argument (their stdin is closed).
    pub(crate) fn stdin_lines(self, t: &Turn) -> Vec<String> {
        match self {
            Harness::Claude => vec![
                serde_json::json!({"type": "control_request", "request_id": "yoke-init", "request": {"subtype": "initialize"}}).to_string(),
                serde_json::json!({
                    "type": "user",
                    "message": {"role": "user", "content": content(t)},
                    // As the Agent SDK sends it; resuming is `--resume`'s job.
                    "parent_tool_use_id": null,
                    "session_id": "default"
                }).to_string(),
            ],
            Harness::Codex | Harness::OpenCode => Vec::new(),
        }
    }

    fn parse(self, line: &str, p: &mut ParseState) -> Vec<Event> {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            // Not JSON: show it as text so a harness we parse wrongly still says something.
            let line = line.trim();
            return if line.is_empty() { vec![] } else { vec![Event::TextDelta(format!("{line}\n"))] };
        };
        match self {
            Harness::Claude => claude::parse(&v, p),
            Harness::Codex => codex::parse(&v, p),
            Harness::OpenCode => opencode::parse(&v, p),
        }
    }
}

/// What a harness reports, in the order it happened.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The harness's own conversation id, for resuming the next turn.
    Session(String),
    Model(String),
    /// Text streaming into the current assistant message.
    TextDelta(String),
    /// A whole assistant message, from harnesses that don't stream deltas.
    Text(String),
    /// The model is reasoning (no text to show yet).
    Thinking,
    ToolStart { id: String, name: String, detail: String },
    ToolEnd { id: String, ok: bool, output: String },
    /// The turn finished (successfully or not).
    Done { cost_usd: Option<f64>, error: Option<String> },
    Error(String),
    /// The CLI asks whether a tool may run; answer with [`Running::answer`].
    Ask(AskRequest),
    /// The CLI withdrew a question (it was interrupted, or decided on its own).
    AskCancelled { request_id: String },
    /// Claude Code's `rate_limit_event`: a plan window moved (its `rate_limit_info`).
    RateLimit(serde_json::Value),
    /// Tokens in the conversation's context as of the latest model call: its input, cache
    /// reads and writes, and output.
    ContextUsed(u64),
    /// The size of the model's context window, in tokens (Claude Code's `result`).
    ContextWindow(u64),
    /// The process exited; always the last event of a turn.
    Exited { code: Option<i32>, stderr: String },
}

/// A tool waiting for the person's approval.
#[derive(Clone, Debug, PartialEq)]
pub struct AskRequest {
    pub request_id: String,
    /// The tool call this is about (its row in the transcript), when the CLI says.
    pub tool_use_id: Option<String>,
    pub tool: String,
    pub input: serde_json::Value,
    /// Rules that would allow this from now on ("Always allow"), as the CLI suggests them.
    pub suggestions: Vec<serde_json::Value>,
    /// Why it's asking, when the CLI says (a path outside the project, a rule that asks).
    pub reason: Option<String>,
}

/// The person's answer to an [`AskRequest`].
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    Allow,
    /// Allow, and add the CLI's suggested rules so it doesn't ask again.
    AlwaysAllow,
    Deny,
}

impl AskRequest {
    /// The control response line that answers this request.
    pub fn reply(&self, d: &Decision) -> String {
        let response = match d {
            Decision::Allow => serde_json::json!({"behavior": "allow", "updatedInput": self.input}),
            Decision::AlwaysAllow => serde_json::json!({"behavior": "allow", "updatedInput": self.input, "updatedPermissions": self.suggestions}),
            Decision::Deny => serde_json::json!({"behavior": "deny", "message": "The user denied this in Yoke."}),
        };
        serde_json::json!({"type": "control_response", "response": {"subtype": "success", "request_id": self.request_id, "response": response}}).to_string()
    }
}

#[derive(Default)]
pub struct ParseState {
    /// Claude: whether the current message streamed text deltas (so its final copy is skipped).
    streamed_text: bool,
    /// Claude: the session's model (`system/init`), to pick its window from `result`.
    model: Option<String>,
    /// Lines the parser wants written back to the CLI's stdin (refusing requests we don't
    /// handle, so the CLI doesn't wait on them).
    pub(crate) replies: Vec<String>,
}

/// One prompt to one harness.
pub struct Turn {
    pub harness: Harness,
    pub prompt: String,
    pub model: String,
    pub effort: String,
    pub access: Access,
    pub resume: Option<String>,
    pub cwd: PathBuf,
    /// Images sent with the prompt.
    pub images: Vec<PathBuf>,
    /// Claude Code: deliver messages from your other Claude Code sessions (`SendMessage`), such
    /// as one saying which files it's editing, or turn them away. Unset, the CLI holds a
    /// message from a session in another permission mode for approval, which a thread here
    /// can't give, so it would expire unread.
    pub peer_messages: bool,
}

/// `path` as an API image block (base64), or `None` if it can't be read or isn't an image type
/// the API takes.
fn image_block(path: &Path) -> Option<serde_json::Value> {
    use base64::Engine;
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let media = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    };
    let bytes = std::fs::read(path).ok()?;
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    Some(serde_json::json!({"type": "image", "source": {"type": "base64", "media_type": media, "data": data}}))
}

/// The prompt as message content: plain text, or text then image blocks.
fn content(t: &Turn) -> serde_json::Value {
    let blocks: Vec<serde_json::Value> = t.images.iter().filter_map(|p| image_block(p)).collect();
    if blocks.is_empty() { return serde_json::Value::String(t.prompt.clone()); }
    let mut all = vec![serde_json::json!({"type": "text", "text": t.prompt})];
    all.extend(blocks);
    serde_json::Value::Array(all)
}

/// A running turn; dropping it doesn't stop it, `stop` does.
pub struct Running {
    child: Arc<Mutex<Option<Child>>>,
    /// Open while the turn runs, for answering the CLI's questions; closed after its result
    /// so it exits.
    stdin: Arc<Mutex<Option<ChildStdin>>>,
}

impl Running {
    /// Kills the CLI and everything it started (tool commands, MCP servers): they share its
    /// process group, and one left running would hold its output open so the turn never ends.
    pub fn stop(&self) {
        if let Some(c) = self.child.lock().unwrap().as_mut() {
            #[cfg(unix)]
            // SAFETY: kill(2) with a negative pid signals that process group; no memory involved.
            unsafe { libc::kill(-(c.id() as i32), libc::SIGKILL); }
            let _ = c.kill();
        }
    }

    /// Changes a running Claude Code turn's permission mode (the Agent SDK's
    /// `setPermissionMode`). False if the CLI is no longer listening.
    pub fn set_access(&self, a: Access) -> bool {
        let line = serde_json::json!({
            "type": "control_request",
            "request_id": format!("yoke-mode-{}", a.claude_mode()),
            "request": {"subtype": "set_permission_mode", "mode": a.claude_mode()},
        }).to_string();
        write_line(&self.stdin, &line)
    }

    /// Answers the CLI's question `ask`. False if the CLI is no longer listening.
    pub fn answer(&self, ask: &AskRequest, d: &Decision) -> bool {
        write_line(&self.stdin, &ask.reply(d))
    }
}

fn write_line(stdin: &Mutex<Option<ChildStdin>>, line: &str) -> bool {
    let mut g = stdin.lock().unwrap();
    let Some(w) = g.as_mut() else { return false };
    let ok = writeln!(w, "{line}").and_then(|_| w.flush()).is_ok();
    if !ok { *g = None; }
    ok
}

/// Where each harness's binary is, if installed. Looks on PATH and in the usual install spots,
/// since a GUI app launched from Finder doesn't get the shell's PATH.
pub fn detect() -> Vec<(Harness, Option<PathBuf>)> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    for extra in [".local/bin", ".claude/local", ".bun/bin", ".npm-global/bin", ".opencode/bin", ".cargo/bin"] {
        dirs.push(home.join(extra));
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    ALL.into_iter()
        .map(|h| (h, dirs.iter().map(|d| d.join(h.binary())).find(|p| p.is_file())))
        .collect()
}

/// Starts `turn`, sending `(tag, event)` to `tx` and waking the UI after each batch.
pub fn spawn<T: Clone + Send + 'static>(bin: &Path, turn: Turn, tag: T, tx: Sender<(T, Event)>, wake: Waker) -> Running {
    let harness = turn.harness;
    let mut cmd = harness.command(bin, &turn);
    let input = harness.stdin_lines(&turn);
    let piped = if input.is_empty() { Stdio::null() } else { Stdio::piped() };
    cmd.current_dir(&turn.cwd).stdin(piped).stdout(Stdio::piped()).stderr(Stdio::piped());
    // Its own process group, so `Running::stop` can end its children too.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let child = Arc::new(Mutex::new(None));
    let stdin = Arc::new(Mutex::new(None));
    let send = move |e: Event| {
        let _ = tx.send((tag.clone(), e));
        wake();
    };
    match cmd.spawn() {
        Err(e) => {
            send(Event::Error(format!("Couldn't start `{}`: {e}", bin.display())));
            send(Event::Exited { code: None, stderr: String::new() });
        }
        Ok(mut c) => {
            let stdout = c.stdout.take().unwrap();
            let mut stderr = c.stderr.take().unwrap();
            *stdin.lock().unwrap() = c.stdin.take();
            *child.lock().unwrap() = Some(c);
            if !input.is_empty() {
                // Its own thread, so a long prompt can't block on a full pipe.
                let stdin = stdin.clone();
                std::thread::spawn(move || {
                    for line in &input {
                        if !write_line(&stdin, line) { break; }
                    }
                });
            }
            let child = child.clone();
            let stdin = stdin.clone();
            std::thread::spawn(move || {
                let err = std::thread::spawn(move || {
                    let mut s = String::new();
                    let _ = stderr.read_to_string(&mut s);
                    s
                });
                let mut p = ParseState::default();
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    for e in harness.parse(&line, &mut p) {
                        // The turn's result: close stdin so a stream-json CLI exits.
                        if matches!(e, Event::Done { .. }) { *stdin.lock().unwrap() = None; }
                        send(e);
                    }
                    for r in p.replies.drain(..) {
                        write_line(&stdin, &r);
                    }
                }
                *stdin.lock().unwrap() = None;
                let stderr = err.join().unwrap_or_default();
                let code = child.lock().unwrap().take().and_then(|mut c| c.wait().ok()).and_then(|s| s.code());
                send(Event::Exited { code, stderr });
            });
        }
    }
    Running { child, stdin }
}

/// A short, one-line description of a tool call's input, for its row.
pub(crate) fn tool_detail(input: &serde_json::Value) -> String {
    for k in ["command", "file_path", "filePath", "path", "pattern", "url", "query", "description", "prompt"] {
        if let Some(s) = input.get(k).and_then(|v| v.as_str()) {
            return first_line(s, 160);
        }
    }
    String::new()
}

pub(crate) fn first_line(s: &str, max: usize) -> String {
    let line = s.trim().lines().next().unwrap_or("");
    if line.chars().count() > max { format!("{}…", line.chars().take(max).collect::<String>()) } else { line.to_owned() }
}

/// Tool output as text, from a string or a list of `{type: text, text}` blocks.
pub(crate) fn output_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(a) => a.iter().filter_map(|b| b.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n"),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}
