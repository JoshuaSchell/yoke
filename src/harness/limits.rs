//! Claude plan limits: how much of each usage window (the 5-hour session, the week, a model's
//! own weekly allowance) the account has used, and when each resets.
//!
//! The Agent SDK's `get_usage` control request asks the CLI (`claude -p --input-format
//! stream-json`) without a prompt, so no tokens are spent; it is what T3 Code's usage meters
//! read. Its reply has the plan (`subscription_type`), a `limits` list (`kind`, `percent`,
//! `resets_at`, a model `scope`), and the extra-usage `spend`. During a turn the CLI also sends
//! `rate_limit_event`s, which move the matching window between probes (`Limits::apply_event`).

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(30);
const REQUEST_ID: &str = "yoke-usage";

const SESSION: Duration = Duration::from_secs(5 * 3600);
const WEEK: Duration = Duration::from_secs(7 * 24 * 3600);

/// What the account's plan allows, as last read.
#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    /// "Max", "Pro", … `None` when signed in with an API key or through a cloud provider.
    pub plan: Option<String>,
    /// False when the account has no plan windows (an API key): nothing to meter.
    pub available: bool,
    pub windows: Vec<Window>,
    /// Extra usage (paid past the plan), when it's turned on.
    pub spend: Option<Spend>,
    pub checked: SystemTime,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    /// Stable id: `session`, `weekly_all`, `weekly_scoped:<model>`.
    pub id: String,
    /// "Session", "Weekly", "Weekly · Fable".
    pub label: String,
    /// 0–100.
    pub used: f64,
    pub resets: Option<SystemTime>,
    /// How long the window runs, for the pace mark.
    pub length: Option<Duration>,
    /// The CLI's own reading: "normal", "warning", "critical"… (empty if it gave none).
    pub severity: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spend {
    pub used: f64,
    pub limit: Option<f64>,
    pub currency: String,
}

impl Window {
    /// How far through the window we are, 0–1, if its length and reset are known: where even
    /// spending would have the bar now.
    pub fn elapsed(&self, now: SystemTime) -> Option<f64> {
        let left = self.resets?.duration_since(now).unwrap_or_default();
        let length = self.length?;
        Some((1.0 - left.as_secs_f64() / length.as_secs_f64()).clamp(0.0, 1.0))
    }
}

impl Limits {
    /// The window closest to its limit: what the composer's gauge shows.
    pub fn tightest(&self) -> Option<&Window> {
        self.windows.iter().max_by(|a, b| a.used.total_cmp(&b.used))
    }

    /// A `rate_limit_event`'s `rate_limit_info` from a running turn: `rateLimitType`
    /// (`five_hour`, `seven_day`, …), `utilization` 0–1 and `resetsAt` in epoch seconds.
    /// Moves the matching window; false if it named none we show.
    pub fn apply_event(&mut self, info: &Value) -> bool {
        let id = match info["rateLimitType"].as_str() {
            Some("five_hour") => "session",
            Some("seven_day") => "weekly_all",
            _ => return false,
        };
        let Some(w) = self.windows.iter_mut().find(|w| w.id == id) else { return false };
        let mut changed = false;
        if let Some(u) = info["utilization"].as_f64() {
            w.used = (u * 100.0).clamp(0.0, 100.0);
            changed = true;
        }
        if let Some(t) = info["resetsAt"].as_f64().filter(|t| *t > 0.0) {
            w.resets = Duration::try_from_secs_f64(t).ok().and_then(|d| SystemTime::UNIX_EPOCH.checked_add(d));
            changed = true;
        }
        if info["status"] == "rejected" {
            w.used = 100.0;
            w.severity = "critical".into();
            changed = true;
        }
        changed
    }

    /// For prefs, so the gauge has numbers at launch before the CLI answers.
    pub fn to_json(&self) -> Value {
        let secs = |t: Option<SystemTime>| t.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d| d.as_secs());
        json!({
            "plan": self.plan,
            "available": self.available,
            "checked": secs(Some(self.checked)),
            "windows": self.windows.iter().map(|w| json!({
                "id": w.id, "label": w.label, "used": w.used, "resets": secs(w.resets),
                "length": w.length.map(|d| d.as_secs()), "severity": w.severity,
            })).collect::<Vec<_>>(),
            "spend": self.spend.as_ref().map(|s| json!({"used": s.used, "limit": s.limit, "currency": s.currency})),
        })
    }

    pub fn from_json(v: &Value) -> Option<Limits> {
        let at = |v: &Value| v.as_u64().and_then(|s| SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(s)));
        let windows = v["windows"].as_array()?.iter().filter_map(|w| Some(Window {
            id: w["id"].as_str()?.into(),
            label: w["label"].as_str()?.into(),
            used: w["used"].as_f64()?,
            resets: at(&w["resets"]),
            length: w["length"].as_u64().map(Duration::from_secs),
            severity: w["severity"].as_str().unwrap_or_default().into(),
        })).collect();
        let spend = v["spend"].as_object().map(|s| Spend {
            used: s["used"].as_f64().unwrap_or(0.0),
            limit: s["limit"].as_f64(),
            currency: s["currency"].as_str().unwrap_or("USD").into(),
        });
        Some(Limits {
            plan: v["plan"].as_str().map(str::to_owned),
            available: v["available"].as_bool()?,
            windows,
            spend,
            checked: at(&v["checked"])?,
        })
    }
}

/// Asks the CLI. `Err` says why it couldn't (not signed in, timed out). Blocks for up to
/// `TIMEOUT`, so call it off the UI thread.
pub fn probe(bin: &Path, cwd: &Path) -> Result<Limits, String> {
    let mut child = Command::new(bin)
        .args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Couldn't start claude: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let child = Arc::new(Mutex::new(child));
    {
        let child = child.clone();
        std::thread::spawn(move || {
            std::thread::sleep(TIMEOUT);
            let _ = child.lock().unwrap().kill();
        });
    }
    let request = format!(r#"{{"type":"control_request","request_id":"{REQUEST_ID}","request":{{"subtype":"get_usage"}}}}"#);
    let mut found = Err("Claude Code didn't answer".to_owned());
    if writeln!(stdin, "{request}").and_then(|_| stdin.flush()).is_ok() {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v["type"] == "control_response" && v["response"]["request_id"] == REQUEST_ID {
                found = parse(&v["response"], SystemTime::now());
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

/// A `get_usage` control response (`{subtype, request_id, response}`).
pub fn parse(r: &Value, now: SystemTime) -> Result<Limits, String> {
    if r["subtype"] == "error" {
        return Err(r["error"].as_str().unwrap_or("Claude Code couldn't read usage").to_owned());
    }
    let r = &r["response"];
    let plan = r["subscription_type"].as_str().filter(|s| !s.is_empty()).map(plan_name);
    let available = r["rate_limits_available"].as_bool().unwrap_or(false);
    let rl = &r["rate_limits"];
    let mut windows = Vec::new();
    if available {
        for l in rl["limits"].as_array().into_iter().flatten() {
            let Some(used) = l["percent"].as_f64() else { continue };
            let model = l["scope"]["model"]["display_name"].as_str();
            let (id, label, length) = match (l["kind"].as_str().unwrap_or_default(), model) {
                ("session", _) => ("session".to_owned(), "Session".to_owned(), Some(SESSION)),
                ("weekly_all", _) => ("weekly_all".to_owned(), "Weekly".to_owned(), Some(WEEK)),
                ("weekly_scoped", Some(m)) => (format!("weekly_scoped:{m}"), format!("Weekly · {m}"), Some(WEEK)),
                (other, m) if !other.is_empty() => (format!("{other}:{}", m.unwrap_or("")), title_case(other), None),
                _ => continue,
            };
            if windows.iter().any(|w: &Window| w.id == id) { continue; }
            windows.push(Window {
                id,
                label,
                used: used.clamp(0.0, 100.0),
                resets: l["resets_at"].as_str().and_then(iso),
                length,
                severity: l["severity"].as_str().unwrap_or_default().into(),
            });
        }
        // Older CLIs have no `limits` list, only the two windows by name.
        if windows.is_empty() {
            for (key, id, label, length) in [("five_hour", "session", "Session", SESSION), ("seven_day", "weekly_all", "Weekly", WEEK)] {
                let Some(used) = rl[key]["utilization"].as_f64() else { continue };
                windows.push(Window { id: id.into(), label: label.into(), used: used.clamp(0.0, 100.0), resets: rl[key]["resets_at"].as_str().and_then(iso), length: Some(length), severity: String::new() });
            }
        }
    }
    let spend = rl["spend"].as_object().filter(|s| s.get("enabled").and_then(Value::as_bool) == Some(true)).map(|s| {
        let money = |v: &Value| {
            let minor = v["amount_minor"].as_f64()?;
            let exp = v["exponent"].as_i64().unwrap_or(2) as i32;
            Some(minor / 10f64.powi(exp))
        };
        Spend {
            used: money(&s["used"]).unwrap_or(0.0),
            limit: money(&s["limit"]),
            currency: s["used"]["currency"].as_str().unwrap_or("USD").into(),
        }
    });
    Ok(Limits { plan, available, windows, spend, checked: now })
}

/// `max` → "Max", `team_premium` → "Team Premium".
fn plan_name(s: &str) -> String {
    title_case(s.trim_start_matches("claude_"))
}

fn title_case(s: &str) -> String {
    s.split(['_', ' ']).filter(|w| !w.is_empty()).map(|w| {
        let mut c = w.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
    }).collect::<Vec<_>>().join(" ")
}

/// RFC 3339 (`2026-11-08T03:20:00.102083+00:00`).
pub fn iso(s: &str) -> Option<SystemTime> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(SystemTime::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real `get_usage` reply (Claude Code 2.1.319, a Max account).
    const REPLY: &str = r#"{"subtype":"success","request_id":"yoke-usage","response":{
        "subscription_type":"max","rate_limits_available":true,"rate_limits":{
            "five_hour":{"utilization":31,"resets_at":"2026-11-08T03:20:00.102083+00:00"},
            "seven_day":{"utilization":3,"resets_at":"2026-11-10T22:00:00.102099+00:00"},
            "limits":[
                {"kind":"session","group":"session","percent":31,"severity":"normal","resets_at":"2026-11-08T03:20:00.102083+00:00","scope":null,"is_active":true},
                {"kind":"weekly_all","group":"weekly","percent":3,"severity":"normal","resets_at":"2026-11-10T22:00:00.102099+00:00","scope":null,"is_active":false},
                {"kind":"weekly_scoped","group":"weekly","percent":0,"severity":"normal","resets_at":"2026-11-10T22:00:00+00:00","scope":{"model":{"id":null,"display_name":"Fable"},"surface":null},"is_active":false}],
            "spend":{"used":{"amount_minor":0,"currency":"USD","exponent":2},"limit":null,"percent":0,"enabled":false}}}}"#;

    #[test]
    fn reads_plan_windows_and_resets() {
        let now = iso("2026-11-08T01:20:00Z").unwrap();
        let l = parse(&serde_json::from_str(REPLY).unwrap(), now).unwrap();
        assert_eq!(l.plan.as_deref(), Some("Max"));
        assert!(l.available && l.spend.is_none());
        let rows: Vec<(&str, &str, f64)> = l.windows.iter().map(|w| (w.id.as_str(), w.label.as_str(), w.used)).collect();
        assert_eq!(rows, vec![("session", "Session", 31.0), ("weekly_all", "Weekly", 3.0), ("weekly_scoped:Fable", "Weekly · Fable", 0.0)]);
        assert_eq!(l.tightest().unwrap().id, "session");
        // Two hours left of five: 60% of the window has gone.
        let pace = l.windows[0].elapsed(now).unwrap();
        assert!((pace - 0.6).abs() < 0.01, "{pace}");
    }

    #[test]
    fn a_turns_rate_limit_event_moves_its_window() {
        let mut l = parse(&serde_json::from_str(REPLY).unwrap(), SystemTime::now()).unwrap();
        assert!(l.apply_event(&json!({"status":"allowed_warning","rateLimitType":"five_hour","utilization":0.82,"resetsAt":1794108000})));
        assert_eq!(l.windows[0].used, 82.0);
        assert!(!l.apply_event(&json!({"rateLimitType":"something_new","utilization":0.5})));
        assert!(l.apply_event(&json!({"status":"rejected","rateLimitType":"seven_day"})));
        assert_eq!(l.windows[1].used, 100.0);
    }

    #[test]
    fn an_api_key_has_no_windows_and_survives_prefs() {
        let r = json!({"subtype":"success","response":{"rate_limits_available":false,"rate_limits":null}});
        let l = parse(&r, SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)).unwrap();
        assert!(!l.available && l.windows.is_empty() && l.plan.is_none());
        let full = parse(&serde_json::from_str(REPLY).unwrap(), SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)).unwrap();
        let back = Limits::from_json(&full.to_json()).unwrap();
        assert_eq!(back.windows.len(), 3);
        assert_eq!(back.plan, full.plan);
        assert_eq!(back.checked, full.checked);
    }

    #[test]
    fn extra_usage_spend_reads_in_major_units() {
        let r = json!({"subtype":"success","response":{"subscription_type":"pro","rate_limits_available":true,"rate_limits":{"limits":[],
            "spend":{"enabled":true,"used":{"amount_minor":1234,"currency":"USD","exponent":2},"limit":{"amount_minor":5000,"currency":"USD","exponent":2}}}}});
        let l = parse(&r, SystemTime::now()).unwrap();
        assert_eq!(l.spend, Some(Spend { used: 12.34, limit: Some(50.0), currency: "USD".into() }));
    }
}
