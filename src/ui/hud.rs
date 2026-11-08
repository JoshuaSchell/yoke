//! A small performance HUD (⇧⌘P): frames drawn per second, and the update (events, poll,
//! view, layout, paint) and encode time of recent frames, from `App::on_frame_encoded`.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rune::{El, FrameEncoded, FrameOutcome, col, row, text};
use rune_kit::Kit;

struct Sample {
    at: Instant,
    update_ms: f64,
    encode_ms: f64,
}

static FRAMES: Mutex<VecDeque<Sample>> = Mutex::new(VecDeque::new());

/// Records a drawn frame (call from `on_frame_encoded`).
pub fn record(f: &FrameEncoded) {
    if f.outcome() != FrameOutcome::Drawn { return; }
    let mut q = FRAMES.lock().unwrap();
    q.push_back(Sample { at: f.at(), update_ms: f.update_ms().unwrap_or(0.0), encode_ms: f.encode_ms().unwrap_or(0.0) });
    while q.len() > 600 { q.pop_front(); }
}

pub struct Stats {
    pub fps: usize,
    pub update_p50: f64,
    pub update_max: f64,
    pub encode_p50: f64,
}

pub fn stats() -> Stats {
    let q = FRAMES.lock().unwrap();
    let now = Instant::now();
    let recent: Vec<&Sample> = q.iter().filter(|s| now.duration_since(s.at) < Duration::from_secs(1)).collect();
    let pick = |f: fn(&Sample) -> f64| {
        let mut v: Vec<f64> = recent.iter().map(|s| f(s)).collect();
        v.sort_by(f64::total_cmp);
        (v.get(v.len() / 2).copied().unwrap_or(0.0), v.last().copied().unwrap_or(0.0))
    };
    let (update_p50, update_max) = pick(|s| s.update_ms);
    let (encode_p50, _) = pick(|s| s.encode_ms);
    Stats { fps: recent.len(), update_p50, update_max, encode_p50 }
}

pub fn view(k: &Kit, threads: usize, items: usize) -> El {
    let c = &k.c;
    let s = stats();
    let line = |label: &str, value: String| {
        row().gap(10.0).justify_between()
            .child(text(label.to_owned()).font(k.ty.text_xsmall).color(c.text_faint))
            .child(text(value).font(k.ty.mono).color(c.text_base))
    };
    col().key("hud").w(196.0).padding(10.0).gap(3.0).radius(10.0).fill(c.surface_float).border(1.0, c.border_weak)
        .shadow(6.0, 18.0, rune::Color::BLACK.alpha(0.18))
        .child(text("Rune frames, last second").font(k.ty.text_xsmall_medium).color(c.text_muted))
        .child(line("drawn", format!("{}", s.fps)))
        .child(line("update p50", format!("{:.2} ms", s.update_p50)))
        .child(line("update max", format!("{:.2} ms", s.update_max)))
        .child(line("encode p50", format!("{:.2} ms", s.encode_p50)))
        .child(line("threads / items", format!("{threads} / {items}")))
}
