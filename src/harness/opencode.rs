//! OpenCode: `opencode run --format json …`, JSON lines of parts (`text`, `tool_use`,
//! `step_finish`, `error`) each carrying `sessionID`. Not tested against a live install here;
//! unknown lines are ignored.

use serde_json::Value;

use super::{Event, ParseState, output_text, tool_detail};

pub fn parse(v: &Value, _: &mut ParseState) -> Vec<Event> {
    let mut out = Vec::new();
    if let Some(id) = v["sessionID"].as_str() { out.push(Event::Session(id.into())); }
    let part = &v["part"];
    match v["type"].as_str() {
        Some("text") => out.push(Event::Text(part["text"].as_str().unwrap_or("").into())),
        Some("reasoning") => out.push(Event::Thinking),
        Some("tool_use") => {
            let id = part["callID"].as_str().or(part["id"].as_str()).unwrap_or_default().to_owned();
            let state = &part["state"];
            out.push(Event::ToolStart { id: id.clone(), name: part["tool"].as_str().unwrap_or("tool").into(), detail: tool_detail(&state["input"]) });
            match state["status"].as_str() {
                Some("completed") => out.push(Event::ToolEnd { id, ok: true, output: output_text(&state["output"]) }),
                Some("error") => out.push(Event::ToolEnd { id, ok: false, output: output_text(&state["error"]) }),
                _ => {}
            }
        }
        Some("error") => out.push(Event::Error(v["error"]["data"]["message"].as_str().or(v["error"]["message"].as_str()).unwrap_or("Error").into())),
        _ => {}
    }
    out
}
