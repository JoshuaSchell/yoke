//! What Claude Code has spent: every assistant message's token counts in Claude Code's
//! transcripts (`~/.claude/projects/**/*.jsonl`, the terminal's sessions and this app's alike),
//! priced per model, by day, model and project. T3 Code's usage page reads the same files.
//!
//! - Claude Code writes a message once per content block, all with the same usage, so
//!   messages count once by `message.id` + `requestId` (as ccusage does), across files too:
//!   a resumed or forked session repeats its history.
//! - Prices are LiteLLM's table (`model_prices_and_context_window.json`, T3 Code's source too),
//!   fetched once a day with `curl` and kept in prefs. Until then, or offline, a copy of its
//!   Claude rows built in (`usage_prices.json`) prices them. 1-hour cache writes, fast mode and
//!   US-only inference have their own rates there; the long-context tiers apply past their
//!   threshold.
//! - On a subscription this is what the same tokens would cost on the API, not a bill.
//!
//! Files are re-read only when their size or modification time changes, so a rescan after a
//! turn reads the one session that grew.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use chrono::{Local, NaiveDate, TimeZone};
use serde_json::Value;

pub const PRICES_URL: &str = "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
/// How old the fetched table may get before the next launch fetches again.
pub const PRICES_MAX_AGE: Duration = Duration::from_secs(24 * 3600);
const BUILT_IN: &str = include_str!("usage_prices.json");

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    /// Cache writes kept 5 minutes, and an hour (dearer).
    pub cache_write: u64,
    pub cache_write_1h: u64,
}

impl Tokens {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write + self.cache_write_1h
    }
    /// Everything sent to the model: what a long-context tier is decided on.
    fn prompt(&self) -> u64 {
        self.input + self.cache_read + self.cache_write + self.cache_write_1h
    }
    pub fn add(&mut self, o: &Tokens) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
        self.cache_write_1h += o.cache_write_1h;
    }
}

/// Dollars per token.
#[derive(Clone, Debug, PartialEq)]
pub struct Rate {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
    cache_write_1h: f64,
    /// Fast mode's multiple, and US-only inference's.
    fast: f64,
    us: f64,
    /// Rates past a prompt size (`_above_200k_tokens`).
    above: Option<(u64, Box<Rate>)>,
}

impl Rate {
    /// One LiteLLM entry; `None` without input and output prices.
    fn from_litellm(e: &Value) -> Option<Rate> {
        let mut r = Rate::tier(e, "")?;
        for (suffix, at) in [("_above_200k_tokens", 200_000), ("_above_100k_tokens", 100_000)] {
            if let Some(t) = Rate::tier(e, suffix) {
                r.above = Some((at, Box::new(Rate { fast: r.fast, us: r.us, ..t })));
                break;
            }
        }
        Some(r)
    }

    fn tier(e: &Value, suffix: &str) -> Option<Rate> {
        let f = |k: &str| e[format!("{k}{suffix}")].as_f64();
        let input = f("input_cost_per_token")?;
        let output = f("output_cost_per_token")?;
        let cache_write = f("cache_creation_input_token_cost").unwrap_or(input);
        let cache_write_1h = f("cache_creation_input_token_cost_above_1hr").unwrap_or(cache_write);
        let pse = &e["provider_specific_entry"];
        Some(Rate {
            input,
            output,
            cache_read: f("cache_read_input_token_cost").unwrap_or(input),
            cache_write,
            cache_write_1h,
            fast: pse["fast"].as_f64().filter(|m| *m > 0.0).unwrap_or(1.0),
            us: pse["us"].as_f64().filter(|m| *m > 0.0).unwrap_or(1.0),
            above: None,
        })
    }

    fn cost(&self, t: &Tokens, fast: bool, us: bool) -> f64 {
        let r = match &self.above {
            Some((at, above)) if t.prompt() > *at => above,
            _ => self,
        };
        let base = t.input as f64 * r.input + t.output as f64 * r.output + t.cache_read as f64 * r.cache_read
            + t.cache_write as f64 * r.cache_write + t.cache_write_1h as f64 * r.cache_write_1h;
        base * if fast { r.fast } else { 1.0 } * if us { r.us } else { 1.0 }
    }
}

/// Model name → rates.
#[derive(Clone, Debug, Default)]
pub struct Prices {
    table: HashMap<String, Rate>,
    /// When LiteLLM's table was fetched; `None` while only the built-in copy prices.
    pub fetched: Option<SystemTime>,
}

impl Prices {
    pub fn built_in() -> Prices {
        Prices::from_json(&serde_json::from_str(BUILT_IN).unwrap_or_default(), None)
    }

    /// LiteLLM's document (or our trimmed copy of it): `{model: {input_cost_per_token, …}}`.
    pub fn from_json(doc: &Value, fetched: Option<SystemTime>) -> Prices {
        let mut table = HashMap::new();
        for (name, e) in doc.as_object().into_iter().flatten() {
            if let Some(r) = Rate::from_litellm(e) { table.insert(name.clone(), r); }
        }
        Prices { table, fetched }
    }

    /// `claude-opus-5-5`, `claude-haiku-4-5-20251001`, `claude-opus-5-5[1m]`: the exact name,
    /// then without a bracketed variant or a date.
    pub fn rate(&self, model: &str) -> Option<&Rate> {
        let base = model.split('[').next().unwrap_or(model);
        let undated = match base.rsplit_once('-') {
            Some((head, tail)) if tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_digit()) => head,
            _ => base,
        };
        [model, base, undated].into_iter().find_map(|m| self.table.get(m))
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }
}

/// Fetches LiteLLM's table and keeps only Claude's rows (a few KB, where the whole file is
/// megabytes): what's saved in prefs. Blocks; call it off the UI thread.
pub fn fetch_prices() -> Result<Value, String> {
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "30", PRICES_URL])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    let doc: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let trimmed = trim_prices(&doc);
    if trimmed.as_object().is_none_or(|o| o.is_empty()) { return Err("no Claude prices in the table".into()); }
    Ok(trimmed)
}

/// The `claude-*` rows, cost fields only.
pub fn trim_prices(doc: &Value) -> Value {
    let mut keep = serde_json::Map::new();
    for (k, e) in doc.as_object().into_iter().flatten() {
        if !k.starts_with("claude-") || k.contains('/') { continue; }
        let Some(e) = e.as_object() else { continue };
        let mut t: serde_json::Map<String, Value> = e.iter()
            .filter(|(x, v)| x.contains("cost") && !x.contains("batches") && v.is_number())
            .map(|(x, v)| (x.clone(), v.clone()))
            .collect();
        if let Some(p) = e.get("provider_specific_entry").filter(|p| p.is_object()) { t.insert("provider_specific_entry".into(), p.clone()); }
        if t.contains_key("input_cost_per_token") { keep.insert(k.clone(), Value::Object(t)); }
    }
    Value::Object(keep)
}

/// One assistant message's usage, as read from a transcript.
#[derive(Clone, Debug)]
struct Record {
    /// `message.id` + `requestId`, hashed: what makes it the same message in another file.
    key: u64,
    at: i64,
    model: String,
    project: String,
    tokens: Tokens,
    fast: bool,
    us: bool,
}

/// What each transcript held when it was last read.
#[derive(Default)]
pub struct Cache {
    files: HashMap<PathBuf, (SystemTime, u64, Vec<Record>)>,
}

/// Spend for one day, model and project.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub day: NaiveDate,
    pub model: String,
    pub project: String,
    pub tokens: Tokens,
    /// 0 for a model with no price.
    pub cost: f64,
    pub priced: bool,
    pub messages: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub rows: Vec<Row>,
    pub files: usize,
    pub scanned: Option<SystemTime>,
    /// How long the scan took, for the page's footnote.
    pub took: Duration,
}

/// Where Claude Code keeps transcripts: each `$CLAUDE_CONFIG_DIR` (comma-separated), else
/// `~/.claude` and `~/.config/claude`.
pub fn roots() -> Vec<PathBuf> {
    if let Some(dirs) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return dirs.to_string_lossy().split(',').map(|d| PathBuf::from(d.trim()).join("projects")).collect();
    }
    let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from) else { return Vec::new() };
    vec![home.join(".claude/projects"), home.join(".config/claude/projects")]
}

/// Reads what changed under `roots` and totals it all. Blocks; call it off the UI thread.
pub fn scan(cache: &mut Cache, roots: &[PathBuf], prices: &Prices) -> Report {
    let start = std::time::Instant::now();
    let mut seen_files = HashSet::new();
    for root in roots {
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                let path = e.path();
                let Ok(meta) = e.metadata() else { continue };
                if meta.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|x| x == "jsonl") {
                    let (mtime, len) = (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len());
                    let fresh = cache.files.get(&path).is_some_and(|(m, l, _)| *m == mtime && *l == len);
                    if !fresh {
                        let records = std::fs::read_to_string(&path).map(|s| read_transcript(&s, &path)).unwrap_or_default();
                        cache.files.insert(path.clone(), (mtime, len, records));
                    }
                    seen_files.insert(path);
                }
            }
        }
    }
    cache.files.retain(|p, _| seen_files.contains(p));
    let mut report = total(cache.files.values().flat_map(|(_, _, r)| r), prices);
    report.files = cache.files.len();
    report.scanned = Some(SystemTime::now());
    report.took = start.elapsed();
    report
}

/// Every assistant message with usage in one transcript.
fn read_transcript(s: &str, path: &Path) -> Vec<Record> {
    // The folder a session file sits in names its project (`-Users-josh-rune`); a line's
    // `cwd` says it properly.
    let folder = path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut out = Vec::new();
    for line in s.lines() {
        // Most lines are prompts, tool output and progress; skip them before parsing.
        if !line.contains("\"usage\"") || !line.contains("\"assistant\"") { continue; }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if let Some(r) = record(&v, &folder) { out.push(r); }
    }
    out
}

fn record(v: &Value, folder: &str) -> Option<Record> {
    if v["type"] != "assistant" { return None; }
    let m = &v["message"];
    let model = m["model"].as_str().filter(|m| !m.is_empty() && !m.starts_with('<'))?;
    let u = &m["usage"];
    let n = |k: &str| u[k].as_u64().unwrap_or(0);
    let written = n("cache_creation_input_tokens");
    let (w5, w1h) = match (u["cache_creation"]["ephemeral_5m_input_tokens"].as_u64(), u["cache_creation"]["ephemeral_1h_input_tokens"].as_u64()) {
        (None, None) => (written, 0),
        (a, b) => (a.unwrap_or(0), b.unwrap_or(0)),
    };
    let tokens = Tokens { input: n("input_tokens"), output: n("output_tokens"), cache_read: n("cache_read_input_tokens"), cache_write: w5, cache_write_1h: w1h };
    if tokens.total() == 0 { return None; }
    let at = chrono::DateTime::parse_from_rfc3339(v["timestamp"].as_str()?).ok()?.timestamp();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match (m["id"].as_str(), v["requestId"].as_str()) {
        (None, None) => v["uuid"].as_str().unwrap_or_default().hash(&mut h),
        (id, req) => (id, req).hash(&mut h),
    }
    let project = v["cwd"].as_str()
        .and_then(|c| Path::new(c).file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| folder.to_owned());
    Some(Record {
        key: h.finish(),
        at,
        model: model.to_owned(),
        project,
        tokens,
        fast: u["speed"] == "fast",
        us: u["inference_geo"] == "us",
    })
}

fn total<'a>(records: impl Iterator<Item = &'a Record>, prices: &Prices) -> Report {
    let mut seen = HashSet::new();
    let mut rows: HashMap<(NaiveDate, &str, &str), Row> = HashMap::new();
    for r in records {
        if !seen.insert(r.key) { continue; }
        let Some(day) = Local.timestamp_opt(r.at, 0).single().map(|t| t.date_naive()) else { continue };
        let rate = prices.rate(&r.model);
        let row = rows.entry((day, r.model.as_str(), r.project.as_str())).or_insert_with(|| Row {
            day, model: r.model.clone(), project: r.project.clone(), tokens: Tokens::default(), cost: 0.0, priced: rate.is_some(), messages: 0,
        });
        row.tokens.add(&r.tokens);
        row.cost += rate.map_or(0.0, |p| p.cost(&r.tokens, r.fast, r.us));
        row.messages += 1;
    }
    let mut rows: Vec<Row> = rows.into_values().collect();
    rows.sort_by(|a, b| a.day.cmp(&b.day).then(a.model.cmp(&b.model)).then(a.project.cmp(&b.project)));
    Report { rows, ..Report::default() }
}

/// One period's totals, for the page.
#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub cost: f64,
    pub tokens: Tokens,
    pub messages: u32,
    /// Tokens no price covered.
    pub unpriced: u64,
    /// By day, every day in the period (zero days included), oldest first.
    pub days: Vec<(NaiveDate, f64, u64)>,
    /// Model, cost, tokens, messages: dearest first.
    pub models: Vec<(String, f64, Tokens, u32)>,
    /// Project, cost, tokens: dearest first.
    pub projects: Vec<(String, f64, u64)>,
}

impl Report {
    /// The `days` days up to and including `today` (1 is today alone).
    pub fn summary(&self, today: NaiveDate, days: u32) -> Summary {
        let from = today - chrono::Days::new(days.saturating_sub(1) as u64);
        let mut s = Summary::default();
        let mut by_day: HashMap<NaiveDate, (f64, u64)> = HashMap::new();
        let mut models: HashMap<&str, (f64, Tokens, u32)> = HashMap::new();
        let mut projects: HashMap<&str, (f64, u64)> = HashMap::new();
        for r in self.rows.iter().filter(|r| r.day >= from && r.day <= today) {
            s.cost += r.cost;
            s.tokens.add(&r.tokens);
            s.messages += r.messages;
            if !r.priced { s.unpriced += r.tokens.total(); }
            let d = by_day.entry(r.day).or_default();
            d.0 += r.cost;
            d.1 += r.tokens.total();
            let m = models.entry(&r.model).or_default();
            m.0 += r.cost;
            m.1.add(&r.tokens);
            m.2 += r.messages;
            let p = projects.entry(&r.project).or_default();
            p.0 += r.cost;
            p.1 += r.tokens.total();
        }
        s.days = from.iter_days().take_while(|d| *d <= today).map(|d| {
            let (c, t) = by_day.get(&d).copied().unwrap_or_default();
            (d, c, t)
        }).collect();
        s.models = models.into_iter().map(|(m, (c, t, n))| (m.to_owned(), c, t, n)).collect();
        s.models.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.2.total().cmp(&a.2.total())));
        s.projects = projects.into_iter().map(|(p, (c, t))| (p.to_owned(), c, t)).collect();
        s.projects.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.2.cmp(&a.2)));
        s
    }
}

pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

/// `$1,234.56`, `$12.34`, `$0.42`, `<$0.01`.
pub fn dollars(x: f64) -> String {
    if x > 0.0 && x < 0.005 { return "<$0.01".into(); }
    let cents = (x * 100.0).round() as i64;
    let (whole, frac) = (cents / 100, cents % 100);
    let mut digits = whole.to_string();
    let mut i = digits.len() as isize - 3;
    while i > 0 {
        digits.insert(i as usize, ',');
        i -= 3;
    }
    format!("${digits}.{frac:02}")
}

/// `950`, `12.3K`, `4.56M`, `1.2B`.
pub fn compact(n: u64) -> String {
    let f = n as f64;
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => trim(format!("{:.1}", f / 1e3)) + "K",
        1_000_000..1_000_000_000 => trim(format!("{:.2}", f / 1e6)) + "M",
        _ => trim(format!("{:.2}", f / 1e9)) + "B",
    }
}

fn trim(s: String) -> String {
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(id: &str, req: &str, ts: &str, model: &str, usage: &str) -> String {
        format!(r#"{{"type":"assistant","timestamp":"{ts}","requestId":"{req}","cwd":"/Users/x/app","message":{{"id":"{id}","model":"{model}","role":"assistant","content":[],"usage":{usage}}}}}"#)
    }

    #[test]
    fn built_in_prices_cover_current_models() {
        let p = Prices::built_in();
        for m in ["claude-opus-5-5", "claude-fable-5-1", "claude-sonnet-5-5", "claude-haiku-5-5", "claude-haiku-4-5-20251001", "claude-opus-5-5[1m]"] {
            assert!(p.rate(m).is_some(), "{m}");
        }
        assert!(p.rate("gpt-5").is_none());
    }

    #[test]
    fn prices_messages_once_with_1h_cache_and_fast_mode() {
        let doc = serde_json::json!({"claude-test-1": {
            "input_cost_per_token": 4e-6, "output_cost_per_token": 2e-5, "cache_read_input_token_cost": 2e-7,
            "cache_creation_input_token_cost": 5e-6, "cache_creation_input_token_cost_above_1hr": 8e-6,
            "provider_specific_entry": {"fast": 2.0, "us": 1.1}}});
        let prices = Prices::from_json(&doc, None);
        let u = r#"{"input_tokens":1000,"output_tokens":100,"cache_read_input_tokens":10000,"cache_creation_input_tokens":2000,"cache_creation":{"ephemeral_5m_input_tokens":1000,"ephemeral_1h_input_tokens":1000}}"#;
        let fast = r#"{"input_tokens":1000,"output_tokens":0,"speed":"fast"}"#;
        // The same message written twice (two content blocks), then again in a resumed session.
        let a = [line("m1", "r1", "2026-11-07T10:00:00Z", "claude-test-1-20260101", u), line("m1", "r1", "2026-11-07T10:00:01Z", "claude-test-1-20260101", u), line("m2", "r2", "2026-11-07T11:00:00Z", "claude-test-1", fast)].join("\n");
        let b = [line("m1", "r1", "2026-11-07T10:00:00Z", "claude-test-1", u), line("m3", "r3", "2026-11-07T12:00:00Z", "<synthetic>", u)].join("\n");
        let mut recs = read_transcript(&a, Path::new("/p/-Users-x-app/s1.jsonl"));
        recs.extend(read_transcript(&b, Path::new("/p/-Users-x-app/s2.jsonl")));
        assert_eq!(recs.len(), 4, "the synthetic message isn't usage");
        let report = total(recs.iter(), &prices);
        let day = chrono::DateTime::parse_from_rfc3339("2026-11-07T10:00:00Z").unwrap().with_timezone(&Local).date_naive();
        let s = report.summary(day, 1);
        assert_eq!(s.messages, 2);
        let m1 = 1000.0 * 4e-6 + 100.0 * 2e-5 + 10000.0 * 2e-7 + 1000.0 * 5e-6 + 1000.0 * 8e-6;
        let m2 = 1000.0 * 4e-6 * 2.0;
        assert!((s.cost - (m1 + m2)).abs() < 1e-9, "{} vs {}", s.cost, m1 + m2);
        assert_eq!(s.tokens.cache_write_1h, 1000);
        assert_eq!(s.projects[0].0, "app");
        assert_eq!(s.days.len(), 1);
    }

    #[test]
    fn a_long_prompt_uses_the_tier_above() {
        let doc = serde_json::json!({"m": {"input_cost_per_token": 1.0, "output_cost_per_token": 1.0, "input_cost_per_token_above_200k_tokens": 2.0, "output_cost_per_token_above_200k_tokens": 3.0}});
        let r = Prices::from_json(&doc, None);
        let r = r.rate("m").unwrap();
        assert_eq!(r.cost(&Tokens { input: 10, output: 1, ..Default::default() }, false, false), 11.0);
        assert_eq!(r.cost(&Tokens { input: 1, output: 1, cache_read: 200_000, ..Default::default() }, false, false), 2.0 + 3.0 + 200_000.0 * 2.0);
    }

    #[test]
    fn summary_fills_empty_days() {
        let prices = Prices::built_in();
        let recs = read_transcript(&line("a", "b", "2026-11-01T12:00:00Z", "claude-haiku-5-5", r#"{"input_tokens":10,"output_tokens":10}"#), Path::new("/p/x/s.jsonl"));
        let report = total(recs.iter(), &prices);
        let today = NaiveDate::from_ymd_opt(2026, 11, 7).unwrap();
        let s = report.summary(today, 7);
        assert_eq!(s.days.len(), 7);
        assert_eq!(s.days.iter().filter(|d| d.2 > 0).count(), 1);
        assert_eq!(report.summary(today, 1).messages, 0);
    }

    /// This machine's transcripts and plan, read for real (no tokens spent):
    /// `cargo test -- --ignored live_usage --nocapture`.
    #[test]
    #[ignore]
    fn live_usage_reads_this_machine() {
        let mut cache = Cache::default();
        let prices = Prices::built_in();
        let r = scan(&mut cache, &roots(), &prices);
        let again = scan(&mut cache, &roots(), &prices);
        let s = r.summary(today(), 30);
        eprintln!("{} files in {:?} (rescan {:?}); 30 days: {} over {} responses, {} tokens, {} unpriced",
            r.files, r.took, again.took, dollars(s.cost), s.messages, compact(s.tokens.total()), compact(s.unpriced));
        for (m, c, t, n) in &s.models { eprintln!("  {m}: {} · {} tokens · {n}", dollars(*c), compact(t.total())); }
        for (p, c, t) in s.projects.iter().take(5) { eprintln!("  {p}: {} · {}", dollars(*c), compact(*t)); }
        assert!(r.files > 0);
        assert_eq!(again.summary(today(), 30).messages, s.messages);
        let bin = crate::harness::detect().into_iter().find_map(|(h, p)| (h == crate::harness::Harness::Claude).then_some(p).flatten()).expect("claude installed");
        let l = crate::harness::limits::probe(&bin, &std::env::temp_dir()).unwrap();
        eprintln!("plan {:?}: {:?}", l.plan, l.windows.iter().map(|w| (w.label.clone(), w.used)).collect::<Vec<_>>());
        match fetch_prices() {
            Ok(doc) => eprintln!("fetched {} Claude prices", doc.as_object().map_or(0, |o| o.len())),
            Err(e) => eprintln!("fetch failed: {e}"),
        }
    }

    #[test]
    fn money_and_counts_read_short() {
        assert_eq!(dollars(1234.567), "$1,234.57");
        assert_eq!(dollars(0.42), "$0.42");
        assert_eq!(dollars(0.001), "<$0.01");
        assert_eq!(dollars(0.0), "$0.00");
        assert_eq!(compact(950), "950");
        assert_eq!(compact(12_340), "12.3K");
        assert_eq!(compact(4_560_000), "4.56M");
        assert_eq!(compact(2_000_000), "2M");
    }

    #[test]
    fn trims_litellm_to_claude_cost_fields() {
        let doc = serde_json::json!({
            "claude-x": {"input_cost_per_token": 1e-6, "output_cost_per_token": 2e-6, "max_tokens": 10, "input_cost_per_token_batches": 5e-7, "provider_specific_entry": {"fast": 2}},
            "gpt-5": {"input_cost_per_token": 1e-6, "output_cost_per_token": 2e-6},
            "bedrock/claude-x": {"input_cost_per_token": 1e-6, "output_cost_per_token": 2e-6}});
        let t = trim_prices(&doc);
        assert_eq!(t.as_object().unwrap().len(), 1);
        assert!(t["claude-x"].get("max_tokens").is_none() && t["claude-x"].get("input_cost_per_token_batches").is_none());
        assert_eq!(t["claude-x"]["provider_specific_entry"]["fast"], 2);
    }
}
