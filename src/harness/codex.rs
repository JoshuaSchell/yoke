//! Codex: `codex exec --json …`, JSON lines of `thread.started`, `item.started`,
//! `item.completed` (agent messages, reasoning, commands, file changes), `turn.completed` and
//! `turn.failed`. Not tested against a live install here; unknown lines are ignored.

use serde_json::Value;

use super::{Event, ParseState, first_line, tool_detail};

pub fn parse(v: &Value, _: &mut ParseState) -> Vec<Event> {
    let item = &v["item"];
    let id = || item["id"].as_str().unwrap_or_default().to_owned();
    match v["type"].as_str() {
        Some("thread.started") => v["thread_id"].as_str().map(|t| vec![Event::Session(t.into())]).unwrap_or_default(),
        Some("item.started") => match item["type"].as_str() {
            Some("reasoning") => vec![Event::Thinking],
            Some("command_execution") => vec![Event::ToolStart { id: id(), name: "Shell".into(), detail: first_line(item["command"].as_str().unwrap_or(""), 160) }],
            Some("mcp_tool_call") => vec![Event::ToolStart { id: id(), name: item["tool"].as_str().unwrap_or("MCP").into(), detail: tool_detail(&item["arguments"]) }],
            Some("web_search") => vec![Event::ToolStart { id: id(), name: "Search".into(), detail: item["query"].as_str().unwrap_or("").into() }],
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
            Some("file_change") => {
                let paths: Vec<&str> = item["changes"].as_array().into_iter().flatten().filter_map(|c| c["path"].as_str()).collect();
                vec![
                    Event::ToolStart { id: id(), name: "Edit".into(), detail: paths.join(", ") },
                    Event::ToolEnd { id: id(), ok: item["status"] != "failed", output: String::new() },
                ]
            }
            _ => vec![],
        },
        Some("turn.completed") => vec![Event::Done { cost_usd: None, error: None }],
        Some("turn.failed") => vec![Event::Done { cost_usd: None, error: Some(v["error"]["message"].as_str().unwrap_or("Turn failed").into()) }],
        Some("error") => vec![Event::Error(v["message"].as_str().unwrap_or("Error").into())],
        _ => vec![],
    }
}
