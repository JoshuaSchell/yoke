//! Codex: `codex exec --json …`, JSON lines of `thread.started`, `item.started`,
//! `item.completed` (agent messages, reasoning, commands, file changes), `turn.completed` and
//! `turn.failed`. Not tested against a live install here; unknown lines are ignored.

use serde_json::Value;

use super::{Event, ParseState, ToolBody, first_line, tool_body, tool_detail};

pub fn parse(v: &Value, _: &mut ParseState) -> Vec<Event> {
    let item = &v["item"];
    let id = || item["id"].as_str().unwrap_or_default().to_owned();
    match v["type"].as_str() {
        Some("thread.started") => v["thread_id"].as_str().map(|t| vec![Event::Session(t.into())]).unwrap_or_default(),
        Some("item.started") => match item["type"].as_str() {
            Some("reasoning") => vec![Event::Thinking],
            Some("command_execution") => vec![Event::ToolStart { id: id(), name: "Shell".into(), detail: first_line(item["command"].as_str().unwrap_or(""), 160), body: tool_body("shell", item) }],
            Some("mcp_tool_call") => vec![Event::ToolStart { id: id(), name: item["tool"].as_str().unwrap_or("MCP").into(), detail: tool_detail(&item["arguments"]), body: ToolBody::None }],
            Some("web_search") => vec![Event::ToolStart { id: id(), name: "Search".into(), detail: item["query"].as_str().unwrap_or("").into(), body: ToolBody::None }],
            _ => vec![],
        },
        Some("item.completed") => match item["type"].as_str() {
            Some("agent_message") => vec![Event::Text(item["text"].as_str().unwrap_or("").into())],
            Some("reasoning") => vec![Event::Thinking],
            Some("command_execution") | Some("mcp_tool_call") | Some("web_search") => vec![Event::ToolEnd {
                id: id(),
                ok: item["status"] != "failed" && item["exit_code"].as_i64().unwrap_or(0) == 0,
                output: item["aggregated_output"].as_str().unwrap_or("").into(),
            }],
            // A row per file, so each is one path (and opens in the editor).
            Some("file_change") => {
                let ok = item["status"] != "failed";
                item["changes"].as_array().into_iter().flatten().filter_map(|c| c["path"].as_str()).enumerate().flat_map(|(i, path)| {
                    let id = format!("{}-{i}", id());
                    [
                        Event::ToolStart { id: id.clone(), name: "Edit".into(), detail: path.into(), body: ToolBody::None },
                        Event::ToolEnd { id, ok, output: String::new() },
                    ]
                }).collect()
            }
            _ => vec![],
        },
        Some("turn.completed") => vec![Event::Done { cost_usd: None, error: None }],
        Some("turn.failed") => vec![Event::Done { cost_usd: None, error: Some(v["error"]["message"].as_str().unwrap_or("Turn failed").into()) }],
        Some("error") => vec![Event::Error(v["message"].as_str().unwrap_or("Error").into())],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A change to several files is a row per file, so each row's path is one file.
    #[test]
    fn a_file_change_is_a_row_per_file() {
        let v: serde_json::Value = serde_json::from_str(r#"{"type":"item.completed","item":{"id":"c1","type":"file_change","status":"completed","changes":[{"path":"/r/a.rs"},{"path":"/r/b.rs"}]}}"#).unwrap();
        let rows: Vec<String> = parse(&v, &mut ParseState::default()).into_iter().filter_map(|e| match e {
            Event::ToolStart { detail, .. } => Some(detail),
            _ => None,
        }).collect();
        assert_eq!(rows, ["/r/a.rs", "/r/b.rs"]);
    }
}
