//! Claude Code: `claude -p --input-format stream-json --output-format stream-json --verbose
//! --include-partial-messages --permission-prompt-tool stdio`.
//!
//! Lines are `system/init` (session and model), `stream_event` (raw API stream: text and
//! thinking deltas), `assistant` (each finished message, with its tool calls), `user` (tool
//! results) and `result` (cost, errors). With the stdio prompt tool the CLI also sends
//! `control_request`s: `can_use_tool` asks whether a tool may run and waits for our
//! `control_response` on stdin; `control_cancel_request` withdraws one.
//!
//! Subagents: a line with a `parent_tool_use_id` is a subagent's (the `Agent` tool call that
//! started it), and becomes an [`Event::Sub`] for that call's card. `system/task_*` lines track
//! each task the CLI runs (a subagent, a shell command) from start to finish, and
//! `background_tasks_changed` lists the ones running in the background, which the process stays
//! alive for after its `result`: once they finish it starts a turn of its own (a second
//! `system/init`) to report on them.

use serde_json::Value;

use super::{AskRequest, BackgroundTask, Event, ParseState, TaskStart, TaskUsage, output_text, tool_body, tool_detail};

pub fn parse(v: &Value, p: &mut ParseState) -> Vec<Event> {
    let s = |k: &str| v.get(k).and_then(Value::as_str);
    let mut out = Vec::new();
    match s("type") {
        Some("control_request") => {
            let id = s("request_id").unwrap_or_default().to_owned();
            let r = &v["request"];
            match r["subtype"].as_str() {
                Some("can_use_tool") => {
                    let reason = r["decision_reason"].as_str().or(r["blocked_path"].as_str()).filter(|x| !x.is_empty());
                    out.push(Event::Ask(AskRequest {
                        request_id: id,
                        tool_use_id: r["tool_use_id"].as_str().map(str::to_owned),
                        tool: r["tool_name"].as_str().unwrap_or("tool").to_owned(),
                        input: r.get("input").cloned().unwrap_or_else(|| Value::Object(Default::default())),
                        suggestions: r["permission_suggestions"].as_array().cloned().unwrap_or_default(),
                        reason: reason.map(str::to_owned),
                    }));
                }
                // Anything else (hooks, MCP) we never registered for: refuse, so the CLI
                // doesn't wait on an answer that won't come.
                other => p.replies.push(serde_json::json!({
                    "type": "control_response",
                    "response": {"subtype": "error", "request_id": id, "error": format!("Yoke doesn't handle {}", other.unwrap_or("this request"))},
                }).to_string()),
            }
        }
        Some("control_cancel_request") => {
            if let Some(id) = s("request_id") { out.push(Event::AskCancelled { request_id: id.into() }); }
        }
        // Each turn the process runs starts with one: an answer to a message, or a turn the
        // CLI starts itself once background work it was waiting on has finished.
        Some("system") if s("subtype") == Some("init") => {
            p.turn_open = true;
            out.push(Event::TurnStart);
            if let Some(id) = s("session_id") { out.push(Event::Session(id.into())); }
            if let Some(m) = s("model") {
                p.model = Some(m.to_owned());
                out.push(Event::Model(m.into()));
            }
        }
        Some("system") => out.extend(task_event(v, p)),
        // A message sent with a uuid: queued behind the running turn, taken in, answered.
        Some("command_lifecycle") => {
            if let (Some(uuid), Some(state)) = (s("command_uuid"), s("state")) {
                out.push(Event::Lifecycle { uuid: uuid.into(), state: state.into() });
            }
        }
        Some("stream_event" | "assistant" | "user") => {
            // A subagent's lines name the tool call that started it; they go to its card, never
            // into the main conversation.
            match v["parent_tool_use_id"].as_str().filter(|id| !id.is_empty()) {
                None => out.extend(message(v, p, None)),
                Some(parent) => {
                    for e in message(v, p, Some(parent)) {
                        if matches!(e, Event::TextDelta(_) | Event::Text(_) | Event::ToolStart { .. } | Event::ToolEnd { .. }) {
                            out.push(Event::Sub { parent: parent.to_owned(), event: Box::new(e) });
                        }
                    }
                }
            }
        }
        // The plan's windows as the turn spends them (`limits::Limits::apply_event`).
        Some("rate_limit_event") => {
            if v["rate_limit_info"].is_object() { out.push(Event::RateLimit(v["rate_limit_info"].clone())); }
        }
        Some("result") => {
            p.turn_open = false;
            let failed = v["is_error"].as_bool().unwrap_or(false) || s("subtype").is_some_and(|t| t != "success");
            let error = failed.then(|| s("result").map(str::to_owned).unwrap_or_else(|| s("subtype").unwrap_or("error").into()));
            // An interrupt ends the turn with `error_during_execution`; that's the stop, not a
            // failure to show.
            let interrupted = failed && s("subtype") == Some("error_during_execution") && s("terminal_reason").is_some_and(|r| r.starts_with("aborted"));
            let error = if interrupted { None } else { error };
            if let Some(id) = s("session_id") { out.push(Event::Session(id.into())); }
            if let Some(w) = context_window(&v["modelUsage"], p.model.as_deref()) { out.push(Event::ContextWindow(w)); }
            out.push(Event::Done { cost_usd: v["total_cost_usd"].as_f64(), error });
        }
        _ => {}
    }
    out
}

/// A conversation line (`stream_event`, `assistant` or `user`), the main agent's (`parent`
/// none) or a subagent's.
fn message(v: &Value, p: &mut ParseState, parent: Option<&str>) -> Vec<Event> {
    let mut out = Vec::new();
    let streamed = match parent {
        None => &mut p.streamed_text,
        Some(id) => p.sub_streamed.entry(id.to_owned()).or_default(),
    };
    match v["type"].as_str() {
        Some("stream_event") => {
            let e = &v["event"];
            // A model call's usage: when it starts, and its output once it ends. A subagent's
            // calls have a context of their own.
            if parent.is_none() {
                let usage = match e["type"].as_str() {
                    Some("message_start") => Some(&e["message"]["usage"]),
                    Some("message_delta") => Some(&e["usage"]),
                    _ => None,
                };
                if let Some(n) = usage.and_then(context_tokens) { out.push(Event::ContextUsed(n)); }
            }
            match e["type"].as_str() {
                Some("message_start") => *streamed = false,
                Some("content_block_delta") => match e["delta"]["type"].as_str() {
                    Some("text_delta") => {
                        if let Some(t) = e["delta"]["text"].as_str() {
                            *streamed = true;
                            out.push(Event::TextDelta(t.into()));
                        }
                    }
                    Some("thinking_delta") => out.push(Event::Thinking),
                    _ => {}
                },
                Some("content_block_start") if e["content_block"]["type"] == "thinking" => out.push(Event::Thinking),
                _ => {}
            }
        }
        Some("assistant") => {
            for b in v["message"]["content"].as_array().into_iter().flatten() {
                match b["type"].as_str() {
                    // Text already arrived as deltas; only use the final copy if it didn't.
                    Some("text") if !*streamed => {
                        if let Some(t) = b["text"].as_str() { out.push(Event::Text(t.into())); }
                    }
                    Some("tool_use") => out.push(Event::ToolStart {
                        id: b["id"].as_str().unwrap_or_default().into(),
                        name: b["name"].as_str().unwrap_or("tool").into(),
                        detail: tool_detail(&b["input"]),
                        body: tool_body(b["name"].as_str().unwrap_or_default(), &b["input"]),
                    }),
                    _ => {}
                }
            }
        }
        Some("user") => {
            for b in v["message"]["content"].as_array().into_iter().flatten() {
                if b["type"] == "tool_result" {
                    out.push(Event::ToolEnd {
                        id: b["tool_use_id"].as_str().unwrap_or_default().into(),
                        ok: !b["is_error"].as_bool().unwrap_or(false),
                        output: output_text(&b["content"]),
                    });
                }
            }
        }
        _ => {}
    }
    out
}

/// The CLI's task events (`system/task_*`, `background_tasks_changed`): subagents and shell
/// commands it runs, in the foreground of a turn or the background. Other `system` lines are
/// left out.
fn task_event(v: &Value, p: &mut ParseState) -> Option<Event> {
    let s = |k: &str| v[k].as_str().map(str::to_owned);
    let usage = |u: &Value| u.is_object().then(|| TaskUsage {
        tokens: u["total_tokens"].as_u64().unwrap_or(0),
        tool_uses: u["tool_uses"].as_u64().unwrap_or(0),
        duration_ms: u["duration_ms"].as_u64().unwrap_or(0),
    });
    let id = s("task_id");
    Some(match v["subtype"].as_str()? {
        "task_started" => Event::TaskStarted(TaskStart {
            id: id?,
            call: s("tool_use_id"),
            kind: s("task_type").unwrap_or_default(),
            description: s("description").unwrap_or_default(),
            subagent_type: s("subagent_type"),
            background: v["is_backgrounded"].as_bool().unwrap_or(false),
            parent_task: s("parent_task_id"),
            prompt: s("prompt"),
            ambient: v["ambient"].as_bool().unwrap_or(false) || v["skip_transcript"].as_bool().unwrap_or(false),
        }),
        "task_progress" => Event::TaskProgress {
            id: id?,
            // The model's one-line summary when the CLI makes one, else what it's doing.
            description: s("summary").filter(|x| !x.trim().is_empty()).or(s("description")).unwrap_or_default(),
            last_tool: s("last_tool_name"),
            usage: usage(&v["usage"]),
        },
        "task_updated" => Event::TaskUpdated {
            id: id?,
            status: v["patch"]["status"].as_str().map(str::to_owned),
            background: v["patch"]["is_backgrounded"].as_bool(),
            error: v["patch"]["error"].as_str().map(str::to_owned),
        },
        "task_notification" => Event::TaskDone {
            id: id?,
            status: s("status").unwrap_or_default(),
            summary: s("summary").unwrap_or_default(),
            usage: usage(&v["usage"]),
        },
        "background_tasks_changed" => {
            let tasks: Vec<BackgroundTask> = v["tasks"].as_array().into_iter().flatten().filter_map(|t| Some(BackgroundTask {
                id: t["task_id"].as_str()?.to_owned(),
                kind: t["task_type"].as_str().unwrap_or_default().to_owned(),
                description: t["description"].as_str().unwrap_or_default().to_owned(),
                parent_task: t["parent_task_id"].as_str().map(str::to_owned),
                ambient: t["ambient"].as_bool().unwrap_or(false),
            })).collect();
            p.background = tasks.iter().filter(|t| !t.ambient).count();
            Event::Background(tasks)
        }
        _ => return None,
    })
}

/// The tokens a model call's `usage` puts in the context: input, cache reads and writes, and
/// output. `None` for a usage with none of them (a synthetic message).
pub fn context_tokens(usage: &Value) -> Option<u64> {
    let n: u64 = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens", "output_tokens"]
        .iter().filter_map(|k| usage[*k].as_u64()).sum();
    (n > 0).then_some(n)
}

/// The session model's context window from a `result`'s `modelUsage` (by model id): the entry
/// for `model` (a `[1m]` suffix aside), else the one that read the most input, since a
/// subagent's model may be in there too.
fn context_window(usage: &Value, model: Option<&str>) -> Option<u64> {
    let map = usage.as_object()?;
    let base = |m: &str| m.split('[').next().unwrap_or(m).to_owned();
    let entry = model.and_then(|m| map.get(m).or_else(|| map.iter().find(|(k, _)| base(k) == base(m)).map(|(_, v)| v)))
        .or_else(|| map.values().max_by_key(|v| ["inputTokens", "cacheReadInputTokens", "cacheCreationInputTokens"].iter().filter_map(|k| v[*k].as_u64()).sum::<u64>()))?;
    entry["contextWindow"].as_u64().filter(|w| *w > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::TaskStart;

    fn run(lines: &[&str]) -> Vec<Event> {
        let mut p = ParseState::default();
        lines.iter().flat_map(|l| parse(&serde_json::from_str(l).unwrap(), &mut p)).collect()
    }

    #[test]
    fn streams_text_and_skips_the_final_copy() {
        let ev = run(&[
            r#"{"type":"system","subtype":"init","session_id":"s1","model":"claude-haiku-5-5"}"#,
            r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hi"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"session_id":"s1","total_cost_usd":0.002}"#,
        ]);
        assert_eq!(ev, vec![
            Event::TurnStart,
            Event::Session("s1".into()),
            Event::Model("claude-haiku-5-5".into()),
            Event::TextDelta("Hi".into()),
            Event::Session("s1".into()),
            Event::Done { cost_usd: Some(0.002), error: None },
        ]);
    }

    #[test]
    fn rate_limit_events_pass_through() {
        let ev = run(&[r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","rateLimitType":"five_hour","utilization":0.4,"resetsAt":1794108000},"session_id":"s1"}"#]);
        let [Event::RateLimit(info)] = ev.as_slice() else { panic!("{ev:?}") };
        assert_eq!(info["rateLimitType"], "five_hour");
    }

    /// Trimmed from Claude Code 2.1.319's stream for one Haiku reply.
    #[test]
    fn context_used_and_window() {
        let ev = run(&[
            r#"{"type":"system","subtype":"init","session_id":"s1","model":"claude-haiku-5-5"}"#,
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"usage":{"input_tokens":2,"cache_creation_input_tokens":9751,"cache_read_input_tokens":11382,"output_tokens":4}}},"parent_tool_use_id":null}"#,
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"usage":{"input_tokens":900000,"output_tokens":1}}},"parent_tool_use_id":"toolu_1"}"#,
            r#"{"type":"stream_event","event":{"type":"message_delta","usage":{"input_tokens":2,"cache_creation_input_tokens":9751,"cache_read_input_tokens":11382,"output_tokens":53}},"parent_tool_use_id":null}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"session_id":"s1","modelUsage":{"claude-sonnet-5-5":{"inputTokens":90000,"contextWindow":200000},"claude-haiku-5-5":{"inputTokens":2,"cacheReadInputTokens":11382,"contextWindow":1000000}}}"#,
        ]);
        let context: Vec<&Event> = ev.iter().filter(|e| matches!(e, Event::ContextUsed(_) | Event::ContextWindow(_))).collect();
        assert_eq!(context, vec![&Event::ContextUsed(21139), &Event::ContextUsed(21188), &Event::ContextWindow(1_000_000)], "a subagent's call is left out, and the window is the session model's");
        assert_eq!(context_window(&serde_json::json!({"claude-opus-5-5": {"contextWindow": 1_000_000}}), Some("claude-opus-5-5[1m]")), Some(1_000_000));
        assert_eq!(context_window(&serde_json::json!({"a": {"inputTokens": 5, "contextWindow": 200_000}, "b": {"inputTokens": 50, "contextWindow": 400_000}}), None), Some(400_000));
    }

    #[test]
    fn tool_calls_start_and_end() {
        let ev = run(&[
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls -la\nmore"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"a\nb","is_error":false}]}}"#,
        ]);
        assert_eq!(ev, vec![
            Event::ToolStart { id: "t1".into(), name: "Bash".into(), detail: "ls -la".into(), body: crate::harness::ToolBody::Code("ls -la\nmore".into()) },
            Event::ToolEnd { id: "t1".into(), ok: true, output: "a\nb".into() },
        ]);
    }

    #[test]
    fn can_use_tool_asks_and_the_answer_is_a_control_response() {
        let ev = run(&[
            r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"cargo check"},"tool_use_id":"t1","permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"cargo check:*"}],"behavior":"allow","destination":"localSettings"}]}}"#,
            r#"{"type":"control_cancel_request","request_id":"r1"}"#,
        ]);
        let Event::Ask(ask) = &ev[0] else { panic!("{ev:?}") };
        assert_eq!((ask.request_id.as_str(), ask.tool.as_str(), ask.tool_use_id.as_deref()), ("r1", "Bash", Some("t1")));
        assert_eq!(ask.input["command"], "cargo check");
        assert_eq!(ev[1], Event::AskCancelled { request_id: "r1".into() });

        let reply = |d| serde_json::from_str::<Value>(&ask.reply(&d)).unwrap();
        let allow = reply(crate::harness::Decision::Allow);
        assert_eq!(allow["type"], "control_response");
        assert_eq!(allow["response"]["request_id"], "r1");
        assert_eq!(allow["response"]["response"]["behavior"], "allow");
        assert_eq!(allow["response"]["response"]["updatedInput"]["command"], "cargo check");
        assert!(allow["response"]["response"].get("updatedPermissions").is_none());
        let always = reply(crate::harness::Decision::AlwaysAllow);
        assert_eq!(always["response"]["response"]["updatedPermissions"][0]["rules"][0]["ruleContent"], "cargo check:*");
        let deny = reply(crate::harness::Decision::Deny);
        assert_eq!(deny["response"]["response"]["behavior"], "deny");
        assert!(deny["response"]["response"]["message"].is_string());
    }

    #[test]
    fn requests_we_dont_handle_are_refused() {
        let mut p = ParseState::default();
        let v: Value = serde_json::from_str(r#"{"type":"control_request","request_id":"h1","request":{"subtype":"hook_callback","callback_id":"x"}}"#).unwrap();
        assert!(parse(&v, &mut p).is_empty());
        let r: Value = serde_json::from_str(&p.replies[0]).unwrap();
        assert_eq!((r["response"]["subtype"].as_str(), r["response"]["request_id"].as_str()), (Some("error"), Some("h1")));
        // Our own initialize's answer is ignored.
        let v: Value = serde_json::from_str(r#"{"type":"control_response","response":{"subtype":"success","request_id":"yoke-init","response":{}}}"#).unwrap();
        assert!(parse(&v, &mut p).is_empty());
    }

    /// An edit carries its before and after, a write its file, and a long command the whole of
    /// itself; a short command is all on its row already.
    #[test]
    fn tool_calls_carry_their_bodies() {
        use crate::harness::{ToolBody, tool_body};
        use serde_json::json;
        assert_eq!(tool_body("Edit", &json!({"file_path": "a.rs", "old_string": "x", "new_string": "y"})), ToolBody::Edits(vec![("x".into(), "y".into())]));
        assert_eq!(tool_body("MultiEdit", &json!({"edits": [{"old_string": "a", "new_string": "b"}, {"old_string": "c", "new_string": "d"}]})), ToolBody::Edits(vec![("a".into(), "b".into()), ("c".into(), "d".into())]));
        assert_eq!(tool_body("edit", &json!({"filePath": "a.ts", "oldString": "1", "newString": "2"})), ToolBody::Edits(vec![("1".into(), "2".into())]), "OpenCode's names");
        assert_eq!(tool_body("Write", &json!({"file_path": "a.rs", "content": "fn main() {}"})), ToolBody::Code("fn main() {}".into()));
        assert_eq!(tool_body("Bash", &json!({"command": "ls -la"})), ToolBody::None);
        assert_eq!(tool_body("Bash", &json!({"command": "cd x &&\\\n  cargo test"})), ToolBody::Code("cd x &&\\\n  cargo test".into()));
        assert_eq!(tool_body("Grep", &json!({"pattern": "fn"})), ToolBody::None);
        let big = "line\n".repeat(20_000);
        let ToolBody::Code(kept) = tool_body("Write", &json!({"content": big})) else { panic!() };
        assert!(kept.len() <= 64 * 1024 && kept.ends_with("line"), "a large file is cut at a line's end");
    }

    /// "Always allow" saves only rules that allow, in the project's local settings or the
    /// session: not a mode change, an added folder, or the user's global settings.
    #[test]
    fn always_allow_keeps_only_local_allow_rules() {
        use crate::harness::{AskRequest, Decision};
        use serde_json::json;
        let local = json!({"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "cargo check:*"}], "behavior": "allow", "destination": "localSettings"});
        let ask = AskRequest {
            request_id: "r1".into(),
            tool_use_id: None,
            tool: "Bash".into(),
            input: json!({"command": "cargo check"}),
            suggestions: vec![
                local.clone(),
                json!({"type": "addRules", "rules": [{"toolName": "Bash"}], "behavior": "allow", "destination": "userSettings"}),
                json!({"type": "setMode", "mode": "bypassPermissions", "destination": "session"}),
                json!({"type": "addDirectories", "directories": ["/"], "destination": "session"}),
            ],
            reason: None,
        };
        assert_eq!(ask.safe_rules(), vec![local]);
        assert_eq!(ask.rule_labels(), ["Bash(cargo check:*)"]);
        let reply: serde_json::Value = serde_json::from_str(&ask.reply(&Decision::AlwaysAllow)).unwrap();
        assert_eq!(reply["response"]["response"]["updatedPermissions"].as_array().unwrap().len(), 1);
    }

    /// A prompt that starts with "-" reaches Codex and OpenCode as the prompt, not an option.
    #[test]
    fn a_prompt_starting_with_a_dash_isnt_an_option() {
        use crate::harness::{Access, DEFAULT_EFFORT, Harness, Turn};
        let turn = Turn { harness: Harness::Codex, prompt: "--dangerously-bypass-approvals-and-sandbox".into(), model: crate::harness::models::DEFAULT.into(), effort: DEFAULT_EFFORT.into(), access: Access::ReadOnly, resume: None, cwd: ".".into(), images: Vec::new(), peer_messages: false };
        let args = |h: Harness| h.command(std::path::Path::new("cli"), &turn).get_args().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>();
        let codex = args(Harness::Codex);
        assert_eq!(&codex[codex.len() - 2..], ["--", "--dangerously-bypass-approvals-and-sandbox"]);
        assert_eq!(args(Harness::OpenCode).last().unwrap(), " --dangerously-bypass-approvals-and-sandbox");
    }

    /// Every line of a fixture through the parser, with the state after each.
    fn replay(file: &str) -> (Vec<Event>, ParseState) {
        let mut p = ParseState::default();
        let ev = file.lines().filter(|l| !l.trim().is_empty()).flat_map(|l| parse(&serde_json::from_str(l).unwrap(), &mut p)).collect();
        (ev, p)
    }

    fn main_text(ev: &[Event]) -> String {
        ev.iter().filter_map(|e| match e { Event::TextDelta(t) | Event::Text(t) => Some(t.as_str()), _ => None }).collect()
    }

    /// Claude Code 2.1.319, trimmed: a foreground subagent ("Count files") and one in the
    /// background ("Slow echo") that runs a command of its own, then the turn the CLI starts
    /// itself once the background one is done. (stdin was closed up front, so the first turn's
    /// result comes late, after that turn.)
    #[test]
    fn subagents_get_their_own_events_and_tasks() {
        let (ev, p) = replay(include_str!("fixtures/claude-subagents.jsonl"));
        let (count, slow) = ("toolu_01V7YxLSKwisZ2Uv5Q2DpWgh", "toolu_01QdVSg8wxoHq5JUjYS94fwU");
        // The main conversation: two Agent calls and its own words, none of the subagents'.
        let starts: Vec<(&str, &str)> = ev.iter().filter_map(|e| match e { Event::ToolStart { id, name, .. } => Some((id.as_str(), name.as_str())), _ => None }).collect();
        assert_eq!(starts, [(count, "Agent"), (slow, "Agent")]);
        let text = main_text(&ev);
        assert!(text.contains("STARTED") && text.contains("FINISHED"), "{text}");
        assert!(!text.contains("SLOW_DONE"), "a subagent's reply isn't the agent's: {text}");
        // Each subagent's own tool call, result and words, for its card.
        let subs: Vec<(&str, &Event)> = ev.iter().filter_map(|e| match e { Event::Sub { parent, event } => Some((parent.as_str(), event.as_ref())), _ => None }).collect();
        assert!(subs.iter().any(|(p, e)| *p == count && matches!(e, Event::ToolStart { name, detail, .. } if name == "Bash" && detail.contains("wc -l"))));
        assert!(subs.iter().any(|(p, e)| *p == count && matches!(e, Event::ToolEnd { ok: true, .. })));
        assert!(subs.iter().any(|(p, e)| *p == slow && matches!(e, Event::Text(t) if t == "SLOW_DONE")));
        assert!(subs.iter().all(|(p, _)| *p == count || *p == slow));
        // Their tasks, from start to end.
        let started: Vec<&TaskStart> = ev.iter().filter_map(|e| match e { Event::TaskStarted(t) => Some(t), _ => None }).collect();
        assert_eq!(started.len(), 3);
        assert_eq!((started[0].description.as_str(), started[0].kind.as_str(), started[0].background, started[0].call.as_deref()), ("Count files", "local_agent", false, Some(count)));
        assert_eq!((started[1].description.as_str(), started[1].background, started[1].subagent_type.as_deref()), ("Slow echo", true, Some("general-purpose")));
        assert!(started[1].prompt.as_deref().is_some_and(|p| p.contains("SLOW_DONE")));
        assert_eq!((started[2].kind.as_str(), started[2].parent_task.as_deref()), ("local_bash", Some("a97e6751fb977a8cb")), "the slow agent's own command");
        assert!(ev.iter().any(|e| matches!(e, Event::TaskProgress { id, description, usage: Some(u), .. } if id == "acb23a59beb62cd31" && description == "Running Count entries in /tmp" && u.tool_uses == 1)));
        assert!(ev.iter().any(|e| matches!(e, Event::TaskUpdated { id, status: Some(s), .. } if id == "acb23a59beb62cd31" && s == "completed")));
        assert!(ev.iter().any(|e| matches!(e, Event::TaskDone { id, status, summary, usage: Some(u) } if id == "acb23a59beb62cd31" && status == "completed" && summary == "485" && u.duration_ms == 2038)));
        assert!(ev.iter().any(|e| matches!(e, Event::TaskDone { id, summary, .. } if id == "a97e6751fb977a8cb" && summary == "SLOW_DONE")));
        // What runs in the background: the slow agent, then nothing.
        let rosters: Vec<Vec<&str>> = ev.iter().filter_map(|e| match e { Event::Background(b) => Some(b.iter().map(|t| t.description.as_str()).collect()), _ => None }).collect();
        assert_eq!(rosters, [vec!["Slow echo"], vec![]]);
        assert_eq!(p.background, 0);
        // Two turns in the one process.
        assert_eq!(ev.iter().filter(|e| matches!(e, Event::TurnStart)).count(), 2);
        assert_eq!(ev.iter().filter(|e| matches!(e, Event::Done { error: None, .. })).count(), 2);
        assert!(!p.turn_open);
    }

    /// A background command (2.1.319, trimmed): its call returns at once, the turn ends, and
    /// once the command is done the CLI reports on it in a turn of its own.
    #[test]
    fn a_background_command_outlives_its_turn() {
        let file = include_str!("fixtures/claude-background-bash.jsonl");
        let mut p = ParseState::default();
        let mut seen = Vec::new();
        for l in file.lines() {
            for e in parse(&serde_json::from_str(l).unwrap(), &mut p) {
                seen.push((e, p.turn_open, p.background));
            }
        }
        let first_done = seen.iter().position(|(e, ..)| matches!(e, Event::Done { .. })).unwrap();
        assert_eq!((seen[first_done].1, seen[first_done].2), (false, 1), "the turn is over, the command isn't");
        let ev: Vec<Event> = seen.into_iter().map(|(e, ..)| e).collect();
        assert!(ev.iter().any(|e| matches!(e, Event::TaskStarted(t) if t.kind == "local_bash" && t.background && t.call.as_deref() == Some("toolu_01PuTXdVBrD22K1t3UqBVBn6"))));
        assert!(ev.iter().any(|e| matches!(e, Event::TaskDone { status, summary, .. } if status == "completed" && summary.starts_with("Background command"))));
        let after: Vec<&Event> = ev[first_done + 1..].iter().filter(|e| matches!(e, Event::TurnStart | Event::Done { .. } | Event::Text(_) | Event::TextDelta(_))).collect();
        assert!(matches!(after.as_slice(), [Event::TurnStart, Event::Text(t), Event::Done { .. }] if t == "FINISHED"), "{after:?}");
        assert_eq!(p.background, 0);
    }

    /// Stop (an interrupt) with `perTaskStopAffordance`: the turn ends, aborted, which isn't an
    /// error to show; the background agent lives on until `stop_task` stops it (2.1.319).
    #[test]
    fn an_interrupted_turn_isnt_a_failure_and_a_stopped_task_says_so() {
        let (ev, _) = replay(include_str!("fixtures/claude-stop-task.jsonl"));
        let dones: Vec<&Option<String>> = ev.iter().filter_map(|e| match e { Event::Done { error, .. } => Some(error), _ => None }).collect();
        assert_eq!(dones, [&None, &None], "the interrupted turn, then the one the CLI started after the stop");
        let sleeper = "a761c9704b5720442";
        assert!(ev.iter().any(|e| matches!(e, Event::TaskUpdated { id, status: Some(s), .. } if id == sleeper && s == "killed")));
        assert!(ev.iter().any(|e| matches!(e, Event::TaskDone { id, status, .. } if id == sleeper && status == "stopped")));
        // The agent was still in the background after the interrupt.
        let interrupted = ev.iter().position(|e| matches!(e, Event::Done { .. })).unwrap();
        let killed = ev.iter().position(|e| matches!(e, Event::TaskUpdated { id, .. } if id == sleeper)).unwrap();
        assert!(interrupted < killed);
    }

    #[test]
    fn lifecycle_and_unknown_system_lines() {
        let ev = run(&[
            r#"{"type":"command_lifecycle","command_uuid":"u1","state":"queued"}"#,
            r#"{"type":"system","subtype":"status","status":"requesting"}"#,
            r#"{"type":"system","subtype":"hook_started","hook_id":"h"}"#,
            r#"{"type":"system","subtype":"task_started","task_id":"t","task_type":"local_bash","description":"x","ambient":true}"#,
        ]);
        assert_eq!(ev[0], Event::Lifecycle { uuid: "u1".into(), state: "queued".into() });
        assert_eq!(ev.len(), 2, "status and hook lines are ignored: {ev:?}");
        assert!(matches!(&ev[1], Event::TaskStarted(t) if t.ambient));
    }

    /// A reset time past what the clock holds is dropped, not a crash.
    #[test]
    fn an_impossible_reset_time_doesnt_crash() {
        let mut l = crate::harness::limits::Limits::from_json(&serde_json::json!({"available": true, "windows": [{"id": "session", "label": "Session", "used": 10.0}], "checked": 1})).unwrap_or_else(|| panic!("limits parse"));
        let _ = l.apply_event(&serde_json::json!({"rateLimitType": "five_hour", "utilization": 0.5, "resetsAt": 1e20}));
        let _ = crate::harness::limits::Limits::from_json(&serde_json::json!({"available": true, "windows": [{"id": "session", "label": "Session", "used": 10.0, "resets": u64::MAX}], "checked": u64::MAX}));
    }
}
