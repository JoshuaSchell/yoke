//! The models each harness offers, asked of the CLI itself where it can say.
//!
//! - Claude Code answers the Agent SDK's `initialize` control request (what `supportedModels()`
//!   sends) with the models this account can use, so new ones (Fable) show up without a release.
//! - OpenCode prints its providers' models with `opencode models`.
//! - Codex has no listing command; it keeps a short built-in list.
//!
//! The built-in lists are also what shows before the first answer arrives, or if asking fails.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

use super::Harness;

/// One model picker entry: `value` goes to `--model` ("Default" sends no flag), `label` shows.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelOpt {
    pub value: String,
    pub label: String,
}

impl ModelOpt {
    pub fn new(value: &str, label: &str) -> ModelOpt {
        ModelOpt { value: value.into(), label: label.into() }
    }
}

/// The value that sends no `--model` flag (the CLI's own default).
pub const DEFAULT: &str = "Default";

/// Shown until (or unless) the CLI says what it has.
pub fn fallback(h: Harness) -> Vec<ModelOpt> {
    let d = ModelOpt::new(DEFAULT, DEFAULT);
    match h {
        Harness::Claude => vec![
            d,
            ModelOpt::new("claude-fable-5-1", "Fable"),
            ModelOpt::new("opus", "Opus"),
            ModelOpt::new("sonnet", "Sonnet"),
            ModelOpt::new("haiku", "Haiku"),
        ],
        Harness::Codex | Harness::OpenCode => vec![d],
    }
}

/// Asks the CLI for its models. `None` if this harness can't say or asking failed. Blocks for
/// up to `TIMEOUT`, so call it off the UI thread.
pub fn discover(h: Harness, bin: &Path, cwd: &Path) -> Option<Vec<ModelOpt>> {
    match h {
        Harness::Claude => claude(bin, cwd),
        Harness::OpenCode => opencode(bin, cwd),
        Harness::Codex => None,
    }
}

const TIMEOUT: Duration = Duration::from_secs(30);
const REQUEST_ID: &str = "rune-code-models";

/// `claude -p --input-format stream-json …`, then one `initialize` control request on stdin.
/// The reply comes before any prompt is needed, so no tokens are spent.
fn claude(bin: &Path, cwd: &Path) -> Option<Vec<ModelOpt>> {
    let mut child = Command::new(bin)
        .args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;
    let child = Arc::new(Mutex::new(child));
    {
        // A CLI that never answers is killed, which ends the read below.
        let child = child.clone();
        std::thread::spawn(move || {
            std::thread::sleep(TIMEOUT);
            let _ = child.lock().unwrap().kill();
        });
    }
    let request = format!(r#"{{"type":"control_request","request_id":"{REQUEST_ID}","request":{{"subtype":"initialize"}}}}"#);
    let mut found = None;
    if writeln!(stdin, "{request}").and_then(|_| stdin.flush()).is_ok() {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v["type"] == "control_response" && v["response"]["request_id"] == REQUEST_ID {
                found = parse_claude(&v);
                break;
            }
        }
    }
    drop(stdin);
    let mut c = child.lock().unwrap();
    let _ = c.kill();
    let _ = c.wait();
    found
}

/// The `models` of an `initialize` reply: `[{value, displayName, description}]`, where
/// `value` "default" is the account's default (no flag).
pub fn parse_claude(v: &Value) -> Option<Vec<ModelOpt>> {
    let models = v.pointer("/response/response/models")?.as_array()?;
    let mut out: Vec<ModelOpt> = Vec::new();
    for m in models {
        let Some(value) = m["value"].as_str().filter(|s| !s.is_empty()) else { continue };
        let (value, label) = if value == "default" {
            (DEFAULT, DEFAULT)
        } else {
            (value, m["displayName"].as_str().filter(|s| !s.trim().is_empty()).unwrap_or(value))
        };
        // The picker maps labels back to values, so they must differ.
        let label = if out.iter().any(|o| o.label == label) { format!("{label} ({value})") } else { label.to_owned() };
        if !out.iter().any(|o| o.value == value) {
            out.push(ModelOpt { value: value.into(), label });
        }
    }
    if out.is_empty() { return None; }
    if out[0].value != DEFAULT {
        out.retain(|o| o.value != DEFAULT);
        out.insert(0, ModelOpt::new(DEFAULT, DEFAULT));
    }
    Some(out)
}

/// `opencode models`: one `provider/model` per line.
fn opencode(bin: &Path, cwd: &Path) -> Option<Vec<ModelOpt>> {
    let out = Command::new(bin).arg("models").current_dir(cwd).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    if !out.status.success() { return None; }
    parse_opencode(&String::from_utf8_lossy(&out.stdout))
}

pub fn parse_opencode(s: &str) -> Option<Vec<ModelOpt>> {
    let mut out = vec![ModelOpt::new(DEFAULT, DEFAULT)];
    for line in s.lines().map(str::trim) {
        if line.contains('/') && !line.contains(char::is_whitespace) && !out.iter().any(|o| o.value == line) {
            out.push(ModelOpt::new(line, line));
        }
    }
    (out.len() > 1).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_claude_initialize_models() {
        let v: Value = serde_json::from_str(r#"{"type":"control_response","response":{"subtype":"success","request_id":"rune-code-models","response":{
            "commands":[],
            "models":[
                {"value":"default","displayName":"Default (recommended)","description":"Opus 5.5"},
                {"value":"fable","displayName":"Fable","description":"Fable 5.1"},
                {"value":"opus","displayName":"Opus","description":"Opus 5.5"},
                {"value":"opus[1m]","displayName":"Opus","description":"Opus 5.5 with 1M context"},
                {"value":"haiku","displayName":"Haiku","description":"Haiku 5.5"}
            ]}}}"#).unwrap();
        let m = parse_claude(&v).unwrap();
        let pairs: Vec<(&str, &str)> = m.iter().map(|o| (o.value.as_str(), o.label.as_str())).collect();
        assert_eq!(pairs, vec![("Default", "Default"), ("fable", "Fable"), ("opus", "Opus"), ("opus[1m]", "Opus (opus[1m])"), ("haiku", "Haiku")]);
    }

    #[test]
    fn claude_reply_without_models_is_none() {
        let v: Value = serde_json::from_str(r#"{"type":"control_response","response":{"subtype":"success","request_id":"x","response":{"commands":[]}}}"#).unwrap();
        assert!(parse_claude(&v).is_none());
    }

    #[test]
    fn reads_opencode_models() {
        let m = parse_opencode("anthropic/claude-fable-5-1\nopenai/gpt-5\n\nsome banner text\n").unwrap();
        assert_eq!(m.iter().map(|o| o.value.as_str()).collect::<Vec<_>>(), vec!["Default", "anthropic/claude-fable-5-1", "openai/gpt-5"]);
    }
}
