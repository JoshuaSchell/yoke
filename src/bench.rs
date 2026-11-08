//! Headless perf benches: CPU time, allocations and heap per frame, for the cases that can get
//! slow (a long thread, a long streaming answer, a huge tool output, many threads streaming).
//!
//! `cargo test --profile perf -- --ignored bench --nocapture --test-threads=1`
//!
//! Numbers cover update, view, layout and paint on the CPU. They leave out GPU encode,
//! presentation and the window's drawables (about 42 MB at 1180×780 on a 2× display).
//! `view` is the top-level view function only; transcript rows are built inside
//! `virtual_rows`, so their cost lands in the frame total, not in `view`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering::Relaxed};
use std::time::Instant;

use rune::{Cx, El, Env, Runtime};

use crate::app::{App, Item, Kind, Thread};
use crate::harness::{Event, Harness};

// --- Allocation counting (test builds only; relaxed atomics, a few ns per call) ---

struct Counting;

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE: AtomicI64 = AtomicI64::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        ALLOC_BYTES.fetch_add(l.size() as u64, Relaxed);
        LIVE.fetch_add(l.size() as i64, Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        ALLOC_BYTES.fetch_add(l.size() as u64, Relaxed);
        LIVE.fetch_add(l.size() as i64, Relaxed);
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size() as i64, Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        ALLOC_BYTES.fetch_add(new as u64, Relaxed);
        LIVE.fetch_add(new as i64 - l.size() as i64, Relaxed);
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Heap bytes in use right now, across all threads.
fn live() -> i64 {
    LIVE.load(Relaxed)
}

fn mb(bytes: i64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// The counters are process-wide: benches take turns even without `--test-threads=1`.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

// --- Timing ---

thread_local! {
    static VIEW_NS: Cell<u64> = const { Cell::new(0) };
}

/// `ui::view`, timed.
fn timed_view(s: &App, cx: &mut Cx<App>) -> El {
    let t = Instant::now();
    let el = crate::ui::view(s, cx);
    VIEW_NS.with(|v| v.set(v.get() + t.elapsed().as_nanos() as u64));
    el
}

#[derive(Clone, Copy)]
struct Sample {
    ms: f64,
    view_ms: f64,
    allocs: u64,
    kb: f64,
    nodes: usize,
    glyphs: usize,
    shaped: u32,
}

const DT: f64 = 1.0 / 120.0;

/// Runs `f`, then one frame, and measures both.
fn timed(rt: &mut Runtime<App>, f: impl FnOnce(&mut Runtime<App>)) -> Sample {
    VIEW_NS.with(|v| v.set(0));
    let (a0, b0) = (ALLOCS.load(Relaxed), ALLOC_BYTES.load(Relaxed));
    let t = Instant::now();
    f(rt);
    rt.frame(DT);
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    Sample {
        ms,
        view_ms: VIEW_NS.with(|v| v.get()) as f64 / 1e6,
        allocs: ALLOCS.load(Relaxed) - a0,
        kb: (ALLOC_BYTES.load(Relaxed) - b0) as f64 / 1024.0,
        nodes: rt.stats.layout_nodes,
        glyphs: rt.stats.glyphs,
        shaped: rt.stats.shaped,
    }
}

fn header() {
    eprintln!("{:<30} {:>5}  {:>7} {:>7} {:>7}  {:>7}  {:>7} {:>8}  {:>6} {:>6} {:>6}",
        "", "n", "p50 ms", "p95", "max", "view", "allocs", "KB alloc", "nodes", "glyphs", "shaped");
}

/// p50/p95/max of frame time; p50 of the rest.
fn summary(name: &str, v: &[Sample]) {
    if v.is_empty() { return; }
    let q = |f: fn(&Sample) -> f64, at: f64| {
        let mut x: Vec<f64> = v.iter().map(f).collect();
        x.sort_by(f64::total_cmp);
        x[((x.len() - 1) as f64 * at) as usize]
    };
    eprintln!("{name:<30} {:>5}  {:>7.2} {:>7.2} {:>7.2}  {:>7.2}  {:>7.0} {:>8.1}  {:>6.0} {:>6.0} {:>6.0}",
        v.len(), q(|s| s.ms, 0.5), q(|s| s.ms, 0.95), q(|s| s.ms, 1.0), q(|s| s.view_ms, 0.5),
        q(|s| s.allocs as f64, 0.5), q(|s| s.kb, 0.5),
        q(|s| s.nodes as f64, 0.5), q(|s| s.glyphs as f64, 0.5), q(|s| s.shaped as f64, 0.5));
}

fn new_app() -> App {
    App::new(Some(std::env::current_dir().unwrap()), rune::prefs::Prefs::memory())
}

fn runtime(app: App) -> Runtime<App> {
    let mut rt = Runtime::new(app, timed_view, (1180.0, 780.0), Env { system_dark: true, scale: 2.0 });
    rt.frame(0.0);
    rt
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..30 { rt.frame(DT); }
}

/// A thread of its own, selected, with one user row.
fn fresh_thread(app: &mut App, id: u64, busy: bool) {
    let mut t = Thread::new(id, format!("Bench thread {id}"), Harness::Claude, "opus".into());
    t.busy = busy;
    t.items.push(Item { id: id * 1000, rev: 0, kind: Kind::User("Explain the retry design in depth.".into()) });
    app.threads.insert(0, t);
}

/// A long, realistic answer: prose with inline markup, lists, code blocks and a table.
fn long_answer(bytes: usize) -> String {
    let mut s = String::new();
    let mut n = 0;
    while s.len() < bytes {
        n += 1;
        s.push_str(&format!("## Part {n}\n\nThe retry loop backs off **exponentially**, capped by `MAX_ATTEMPTS`, and each \
            attempt is logged with its [request id](https://example.com/r/{n}). A jittered delay keeps clients from \
            retrying in lockstep, which matters once *hundreds* of them share an upstream.\n\n"));
        s.push_str("- `fetch` wraps the request and owns the loop\n- `Backoff::next` returns the delay, or `None` to give up\n- errors other than timeouts and 5xx return at once\n\n");
        if n % 2 == 0 {
            s.push_str("```rust\nfn next(&mut self) -> Option<Duration> {\n    if self.attempt >= MAX_ATTEMPTS { return None; }\n    self.attempt += 1;\n    let base = self.base * 2u32.pow(self.attempt);\n    Some(base.min(self.cap) + jitter())\n}\n```\n\n");
        }
        if n % 5 == 0 {
            s.push_str("| attempt | delay | cumulative |\n|---|---|---|\n| 1 | 100 ms | 100 ms |\n| 2 | 200 ms | 300 ms |\n| 3 | 400 ms | 700 ms |\n\n");
        }
    }
    s
}

/// Splits `s` into deltas of about `size` bytes, at char boundaries.
fn deltas(s: &str, size: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < s.len() {
        let mut end = (start + size).min(s.len());
        while !s.is_char_boundary(end) { end += 1; }
        out.push(s[start..end].to_owned());
        start = end;
    }
    out
}

/// Opening a 3,000-row thread, streaming 1,500 deltas into it, scrolling it, idling.
#[test]
#[ignore]
fn bench_long_thread() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut app = new_app();
    let before = live();
    crate::app::scenes::long_with(&mut app, 500);
    let rows: usize = app.threads.iter().map(|t| t.items.len()).sum();
    let state = live() - before;
    app.selected = Some(101);
    let mut rt = runtime(app);
    eprintln!("\n== long thread: {rows} rows; state {:.1} MB ({:.0} B/row)", mb(state), state as f64 / rows as f64);
    header();
    let heap = live();
    let open = timed(&mut rt, |rt| rt.update(|s, _| s.select(100)));
    summary("open 3,000-row thread", &[open]);
    settle(&mut rt);
    eprintln!("{:<30} {:+.1} MB", "heap after opening", mb(live() - heap));

    rt.update(|s, _| s.apply(100, Event::TextDelta(String::new())));
    let words = "The retry loop backs off **exponentially**, and `MAX_ATTEMPTS` caps it. ";
    let mut stream = Vec::new();
    for i in 0..1500 {
        let d = if i % 40 == 39 { "\n\n".to_owned() } else { words.split(' ').nth(i % 11).unwrap_or("x").to_owned() + " " };
        stream.push(timed(&mut rt, |rt| rt.update(|s, _| s.apply(100, Event::TextDelta(d)))));
    }
    summary("stream delta frame", &stream);
    rt.update(|s, _| s.apply(100, Event::Done { cost_usd: None, error: None }));
    settle(&mut rt);

    // Virtualisation: only rows near the viewport are mounted, before and after scrolling.
    let mounted = |rt: &Runtime<App>| rt.elements().iter().filter_map(|e| e.text.clone()).filter(|t| t.contains("(#")).count();
    let before_scroll = mounted(&rt);
    let mut scroll = Vec::new();
    for _ in 0..600 {
        scroll.push(timed(&mut rt, |rt| rt.wheel(700.0, 400.0, 40.0)));
    }
    summary("wheel scroll frame", &scroll);
    let after_scroll = mounted(&rt);
    eprintln!("{:<30} {before_scroll} before scrolling, {after_scroll} after", "user rows mounted");
    assert!(before_scroll < 20 && after_scroll < 20, "rows aren't virtualised");
    let idle: Vec<Sample> = (0..120).map(|_| timed(&mut rt, |_| {})).collect();
    summary("idle frame", &idle);
}

/// One answer streamed to 64 KB: does a delta cost more as the message grows?
#[test]
#[ignore]
fn bench_long_message() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut app = new_app();
    crate::app::scenes::demo(&mut app);
    fresh_thread(&mut app, 900, true);
    app.selected = Some(900);
    let mut rt = runtime(app);
    let text = long_answer(64 * 1024);
    let parts = deltas(&text, 16);
    eprintln!("\n== long message: {} KB in {} deltas of ~16 B", text.len() / 1024, parts.len());
    header();
    let heap = live();
    let bucket = 8 * 1024;
    let mut buckets: Vec<Vec<Sample>> = vec![Vec::new(); text.len().div_ceil(bucket)];
    let mut len = 0;
    for d in parts {
        len += d.len();
        let s = timed(&mut rt, |rt| rt.update(|s, _| s.apply(900, Event::TextDelta(d))));
        buckets[(len - 1) / bucket].push(s);
    }
    for (i, b) in buckets.iter().enumerate() {
        summary(&format!("delta at {:>2}–{:>2} KB", i * 8, (i + 1) * 8), b);
    }
    eprintln!("{:<30} {:+.1} MB (message is {:.2} MB)", "heap while streaming", mb(live() - heap), mb(text.len() as i64));
    let done = timed(&mut rt, |rt| rt.update(|s, _| s.apply(900, Event::Done { cost_usd: None, error: None })));
    summary("finish (reparse as done)", &[done]);
    settle(&mut rt);
    eprintln!("{:<30} {:+.1} MB", "heap after finishing", mb(live() - heap));
}

/// A tool that prints 1 MB (a big `cat` or test log): landing it, expanding it, and what
/// 50 of them keep in memory.
#[test]
#[ignore]
fn bench_big_tool_output() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut app = new_app();
    crate::app::scenes::demo(&mut app);
    fresh_thread(&mut app, 901, true);
    app.selected = Some(901);
    let mut rt = runtime(app);
    let line = "test retry::backs_off_with_jitter_under_load ... ok (12 ms)\n";
    let output: String = line.repeat(1024 * 1024 / line.len());
    eprintln!("\n== big tool output: {:.1} MB, {} lines", mb(output.len() as i64), output.lines().count());
    header();
    let heap = live();
    let start = timed(&mut rt, |rt| rt.update(|s, _| s.apply(901, Event::ToolStart { id: "big".into(), name: "Bash".into(), detail: "cargo test".into() })));
    summary("tool starts", &[start]);
    let out = output.clone();
    let end = timed(&mut rt, |rt| rt.update(|s, _| s.apply(901, Event::ToolEnd { id: "big".into(), ok: true, output: out })));
    summary("1 MB output lands", &[end]);
    settle(&mut rt);
    let item = rt.state.thread().unwrap().items.iter().find(|i| matches!(&i.kind, Kind::Tool { call, .. } if call == "big")).unwrap().id;
    let open = timed(&mut rt, |rt| rt.update(|s, _| { s.expanded.insert(item); }));
    summary("expand it", &[open]);
    settle(&mut rt);
    let scroll: Vec<Sample> = (0..120).map(|_| timed(&mut rt, |rt| rt.wheel(700.0, 400.0, 40.0))).collect();
    summary("scroll past it", &scroll);
    let close = timed(&mut rt, |rt| rt.update(|s, _| { s.expanded.remove(&item); }));
    summary("collapse it", &[close]);
    settle(&mut rt);
    eprintln!("{:<30} {:+.1} MB", "heap held by one", mb(live() - heap));
    let heap = live();
    for i in 0..50 {
        let id = format!("big{i}");
        rt.update(|s, _| s.apply(901, Event::ToolStart { id: id.clone(), name: "Bash".into(), detail: "cargo test".into() }));
        let out = output.clone();
        rt.update(|s, _| s.apply(901, Event::ToolEnd { id, ok: true, output: out }));
        rt.frame(DT);
    }
    settle(&mut rt);
    eprintln!("{:<30} {:+.1} MB", "heap held by 50 more", mb(live() - heap));
}

/// 16 threads streaming at once, one of them on screen: what the 15 off screen cost.
#[test]
#[ignore]
fn bench_busy_threads() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut app = new_app();
    crate::app::scenes::demo(&mut app);
    let ids: Vec<u64> = (0..16).map(|i| 950 + i).collect();
    for &id in &ids { fresh_thread(&mut app, id, true); }
    app.selected = Some(ids[0]);
    let mut rt = runtime(app);
    eprintln!("\n== busy threads: {} streaming, one selected", ids.len());
    header();
    for &id in &ids { rt.update(|s, _| s.apply(id, Event::TextDelta(String::new()))); }
    settle(&mut rt);
    let words = ["The ", "retry ", "loop ", "backs ", "off ", "**exponentially**, ", "and ", "`MAX_ATTEMPTS` ", "caps ", "it.\n\n"];
    let one: Vec<Sample> = (0..600).map(|i| timed(&mut rt, |rt| rt.update(|s, _| s.apply(ids[0], Event::TextDelta(words[i % 10].into()))))).collect();
    summary("selected thread only", &one);
    let off: Vec<Sample> = (0..600).map(|i| timed(&mut rt, |rt| rt.update(|s, _| {
        for &id in &ids[1..] { s.apply(id, Event::TextDelta(words[i % 10].into())); }
    }))).collect();
    summary("15 off-screen threads only", &off);
    let all: Vec<Sample> = (0..600).map(|i| timed(&mut rt, |rt| rt.update(|s, _| {
        for &id in &ids { s.apply(id, Event::TextDelta(words[i % 10].into())); }
    }))).collect();
    summary("all 16 threads", &all);
}

/// The counting allocator is installed (other tests' threads move `LIVE`, so only the
/// monotonic counters are checked).
#[test]
fn allocator_counts() {
    let (a, b) = (ALLOCS.load(Relaxed), ALLOC_BYTES.load(Relaxed));
    let v: Vec<u8> = std::hint::black_box(Vec::with_capacity(4096));
    assert!(ALLOCS.load(Relaxed) > a);
    assert!(ALLOC_BYTES.load(Relaxed) - b >= 4096);
    drop(v);
}
