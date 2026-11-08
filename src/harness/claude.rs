//! Claude Code: `claude -p --input-format stream-json --output-format stream-json --verbose
//! --include-partial-messages --permission-prompt-tool stdio`.
//!
//! Lines are `system/init` (session and model), `stream_event` (raw API stream: text and
//! thinking deltas), `assistant` (each finished message, with its tool calls), `user` (tool
//! results) and `result` (cost, errors). With the stdio prompt tool the CLI also sends
//! `control_request`s: `can_use_tool` asks whether a tool may run and waits for our
//! `control_response` on stdin; `control_cancel_request` withdraws one.

use serde_json::Value;

use super::{AskRequest, Event, ParseState, output_text, tool_detail};

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
        Some("system") if s("subtype") == Some("init") => {
            if let Some(id) = s("session_id") { out.push(Event::Session(id.into())); }
            if let Some(m) = s("model") { out.push(Event::Model(m.into())); }
        }
        Some("stream_event") => {
            let e = &v["event"];
            match e["type"].as_str() {
                Some("message_start") => p.streamed_text = false,
                Some("content_block_delta") => match e["delta"]["type"].as_str() {
                    Some("text_delta") => {
                        if let Some(t) = e["delta"]["text"].as_str() {
                            p.streamed_text = true;
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
                    Some("text") if !p.streamed_text => {
                        if let Some(t) = b["text"].as_str() { out.push(Event::Text(t.into())); }
                    }
                    Some("tool_use") => out.push(Event::ToolStart {
                        id: b["id"].as_str().unwrap_or_default().into(),
                        name: b["name"].as_str().unwrap_or("tool").into(),
                        detail: tool_detail(&b["input"]),
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
        // The plan's windows as the turn spends them (`limits::Limits::apply_event`).
        Some("rate_limit_event") => {
            if v["rate_limit_info"].is_object() { out.push(Event::RateLimit(v["rate_limit_info"].clone())); }
        }
        Some("result") => {
            let failed = v["is_error"].as_bool().unwrap_or(false) || s("subtype").is_some_and(|t| t != "success");
            let error = failed.then(|| s("result").map(str::to_owned).unwrap_or_else(|| s("subtype").unwrap_or("error").into()));
            if let Some(id) = s("session_id") { out.push(Event::Session(id.into())); }
            out.push(Event::Done { cost_usd: v["total_cost_usd"].as_f64(), error });
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn tool_calls_start_and_end() {
        let ev = run(&[
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls -la\nmore"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"a\nb","is_error":false}]}}"#,
        ]);
        assert_eq!(ev, vec![
            Event::ToolStart { id: "t1".into(), name: "Bash".into(), detail: "ls -la".into() },
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
}
