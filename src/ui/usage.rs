//! Usage: how close the Claude plan is to its limits, and what Claude Code has spent.
//!
//! - The composer's gauge shows the window nearest its limit; it opens a popover with every
//!   window, as T3 Code's usage banner does.
//! - The Usage page (⇧⌘U, or the gauge in the toolbar) takes the main pane, as Settings does:
//!   the plan's meters, then spend at API prices from Claude Code's transcripts by day, model
//!   and project.
//!
//! A meter fills with how much of the window is used, in the accent, then the warning and
//! danger colours (with a label, so it isn't colour alone). The tick across it is where even
//! spending would be by now; a fill past it is spending faster than the window passes.

use std::time::SystemTime;

use chrono::{DateTime, Local};
use rune::{El, Icon, ScrollSpec, TextStyle, Tx, col, el, icon, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, Popover, SegmentedControl, Spinner};
use rune_theme_opencode::icons;

use crate::app::{App, USAGE_PERIODS};
use crate::harness::Harness;
use crate::harness::limits::{Limits, Window};
use crate::usage::{self, Summary, compact, dollars};

/// A gauge: an arc with a needle. For the toolbar button and the composer's chip.
pub static GAUGE: Icon = Icon { name: "gauge", view: 20.0, body: r##"<path d="M3.6 14.6a7 7 0 1 1 12.8 0" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/><path d="M10 11.8l3.2-3.6" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/><circle cx="10" cy="11.8" r="1.4" fill="currentColor"/>"## };

/// The page's reading column, as Settings'.
const W: f32 = 720.0;
/// The daily chart's plot height.
const CHART_H: f32 = 132.0;

// ---------------------------------------------------------------------------------------------
// Meters

/// At or past these, a window is near (warning) or at (danger) its limit.
const NEAR: f64 = 75.0;
const AT: f64 = 90.0;

fn fill_color(w: &Window, k: &Kit) -> rune::Color {
    if w.used >= AT || w.severity == "critical" { k.c.danger.icon } else if w.used >= NEAR || w.severity == "warning" { k.c.warning.icon } else { k.c.bg_accent }
}

fn status(w: &Window) -> Option<&'static str> {
    if w.used >= 100.0 { Some("At limit") } else if w.used >= AT || w.severity == "critical" { Some("Nearly out") } else if w.used >= NEAR || w.severity == "warning" { Some("Near limit") } else { None }
}

/// "Ahead of pace" when the fill is past the even-spending tick by more than a few points.
fn pace(w: &Window, now: SystemTime) -> Option<&'static str> {
    let even = w.elapsed(now)? * 100.0;
    Some(if w.used > even + 5.0 { "Ahead of pace" } else if w.used < even - 5.0 { "Under pace" } else { "On pace" })
}

/// "1h 58m", "3d 4h", "12m", "<1m".
pub fn span(secs: u64) -> String {
    let (d, h, m) = (secs / 86400, secs % 86400 / 3600, secs % 3600 / 60);
    match (d, h, m) {
        (0, 0, 0) => "<1m".into(),
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

/// "resets in 1h 58m · 9:20 PM", or "Tue 3:00 PM" past today.
fn resets(w: &Window, now: SystemTime) -> String {
    let Some(at) = w.resets else { return String::new() };
    let left = at.duration_since(now).map_or(0, |d| d.as_secs());
    let local: DateTime<Local> = at.into();
    let today = local.date_naive() == Local::now().date_naive();
    let clock = local.format(if today { "%-I:%M %p" } else { "%a %-I:%M %p" });
    format!("Resets in {} · {clock}", span(left))
}

/// The bar alone: the used fill on a track of the same colour, and the even-pace tick.
fn bar(w: &Window, now: SystemTime, h: f32, k: &Kit) -> El {
    let fill = fill_color(w, k);
    let frac = (w.used / 100.0).clamp(0.0, 1.0) as f32;
    let mut track = row().w_full().h(h).radius(h / 2.0).fill(fill.alpha(0.18)).clip_rounded(h / 2.0);
    if frac > 0.0 {
        track = track.child(el().w_frac(frac).min_w(h).h_full().radius(h / 2.0).fill(fill));
    }
    let mut stack = col().w_full().h(h + 6.0).justify_center().child(track);
    if let Some(e) = w.elapsed(now) {
        // A 2-point tick in strong ink, with a gap in the page's colour either side.
        let tick = el().w(2.0).h(h + 6.0).radius(1.0).fill(k.c.text_strong).border(1.0, k.c.surface_raised_base);
        stack = stack.child(row().absolute(0.0, 0.0).w_full().h(h + 6.0)
            .child(el().w_frac(e as f32).h_full())
            .child(tick.offset(-1.0, 0.0)));
    }
    let even = w.elapsed(now).map_or(String::new(), |e| format!(". The tick is where even spending would be: {:.0}% of the window has passed", e * 100.0));
    stack.label(format!("{}: {:.0}% used{even}", w.label, w.used)).help(format!("{:.0}% used{even}.", w.used))
}

/// A window's meter: its name and how much is used, the bar, then when it resets and the pace.
fn meter(w: &Window, now: SystemTime, k: &Kit, compact: bool) -> El {
    let c = &k.c;
    let (label, small) = if compact { (k.ty.small, k.ty.text_xsmall) } else { (TextStyle { size: 14.0, line_height: 20.0, ..k.ty.body }, TextStyle { size: 12.0, line_height: 16.0, ..k.ty.body }) };
    let mut head = row().w_full().gap(8.0).center_y()
        .child(text(w.label.clone()).font(label).color(c.text_base).truncate().shrink().grow(1.0).min_w(0.0));
    if let Some(s) = status(w) {
        head = head.child(row().gap(4.0).center_y()
            .child(icon(&icons::WARNING, 12.0).color(fill_color(w, k)))
            .child(text(s).font(small).color(c.text_base)));
    }
    head = head.child(text(format!("{:.0}% used", w.used)).font(TextStyle { ..label }.features("'tnum' 1")).color(c.text_strong));
    let mut foot = row().w_full().gap(8.0).center_y()
        .child(text(resets(w, now)).font(small).color(c.text_faint).truncate().shrink().grow(1.0).min_w(0.0));
    if let Some(p) = pace(w, now) {
        foot = foot.child(text(p).font(small).color(c.text_faint));
    }
    col().key(format!("meter-{}", w.id)).w_full().gap(if compact { 4.0 } else { 6.0 })
        .child(head)
        .child(bar(w, now, if compact { 6.0 } else { 8.0 }, k))
        .child(foot)
}

fn plan_title(l: &Limits) -> String {
    match &l.plan {
        Some(p) => format!("Claude {p}"),
        None => "Claude".into(),
    }
}

fn ago(t: SystemTime) -> String {
    let s = t.elapsed().map_or(0, |d| d.as_secs());
    if s < 60 { "just now".into() } else { format!("{} ago", span(s)) }
}

// ---------------------------------------------------------------------------------------------
// The composer's gauge

/// The chip in the composer's bar: the tightest window's fill and percentage, opening the
/// popover. Nothing without Claude, or before the plan has been read.
/// `compact` (a narrow composer) leaves out the little bar, keeping the gauge and the percent.
pub fn chip(s: &App, k: &Kit, compact: bool, layer: &mut Vec<El>) -> Option<El> {
    if s.active_harness() != Harness::Claude { return None; }
    let l = s.limits.as_ref().filter(|l| l.available)?;
    let w = l.tightest()?;
    let now = SystemTime::now();
    let c = &k.c;
    let open = s.open_menu == Some("usage");
    let fill = fill_color(w, k);
    let frac = (w.used / 100.0).clamp(0.0, 1.0) as f32;
    let mini = row().w(28.0).h(4.0).radius(2.0).fill(fill.alpha(0.2)).clip_rounded(2.0)
        .child(el().w_frac(frac).min_w(if frac > 0.0 { 4.0 } else { 0.0 }).h_full().radius(2.0).fill(fill));
    let mut trigger = row().key("usage-chip").h(28.0).px(8.0).gap(6.0).center_y().radius(8.0)
        .cursor(rune::Cursor::Pointer)
        .fill(if open { c.surface_hover } else { rune::Color::TRANSPARENT })
        .label(format!("Usage: {} {:.0}% used", w.label, w.used))
        .help(format!("{} {:.0}% used · {}", w.label, w.used, resets(w, now).to_lowercase()))
        .child(icon(&GAUGE, 14.0).color(c.icon_muted));
    if !compact { trigger = trigger.child(mini); }
    let trigger = trigger
        .child(text(format!("{:.0}%", w.used)).font(k.ty.small.features("'tnum' 1")).color(c.text_muted));

    let mut body = col().w(272.0).gap(12.0);
    for w in &l.windows {
        body = body.child(meter(w, now, k, true));
    }
    if let Some(sp) = &l.spend { body = body.child(spend_line(sp, k)); }
    let today = s.usage.summary(usage::today(), 1).cost;
    let week = s.usage.summary(usage::today(), 7).cost;
    body = body.child(el().w_full().h(1.0).fill(c.border_weak))
        .child(row().w_full().gap(8.0).center_y()
            .child(text(format!("API value: {} today · {} this week", dollars(today), dollars(week))).font(k.ty.text_xsmall).color(c.text_faint).grow(1.0).min_w(0.0).truncate()))
        .child(row().w_full().gap(8.0).center_y()
            .child(text(format!("Read {}", ago(l.checked))).font(k.ty.text_xsmall).color(c.text_faint).grow(1.0))
            .child(Button::new("Open usage")
                .variant(ButtonVariant::Outline)
                .control_size(ControlSize::Small)
                .on_press(|s: &mut App| s.open_usage())
                .build(k)
                .key("usage-open-page")));
    let pop = Popover::new("usage")
        .trigger(trigger)
        .title(format!("{} plan", plan_title(l)))
        .child(body)
        .open(open)
        .on_close(|s: &mut App| s.open_menu = None)
        .build(k, layer)
        .on_press(|s: &mut App| s.toggle_menu("usage"));
    Some(pop)
}

fn spend_line(sp: &crate::harness::limits::Spend, k: &Kit) -> El {
    let limit = sp.limit.map_or(String::new(), |l| format!(" of {}", dollars(l)));
    text(format!("Extra usage: {}{limit} {}", dollars(sp.used), if sp.currency == "USD" { "" } else { sp.currency.as_str() }).trim().to_owned())
        .font(k.ty.small).color(k.c.text_base)
}

// ---------------------------------------------------------------------------------------------
// The page

pub fn page(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let now = SystemTime::now();
    let busy = s.limits_busy() || s.usage_scanning();
    let mut refresh = row().gap(8.0).center_y();
    if busy { refresh = refresh.child(Spinner::new().size(12.0).color(c.icon_muted).build(k)); }
    refresh = refresh.child(Button::new("Refresh")
        .variant(ButtonVariant::Ghost)
        .disabled(busy)
        .on_press(|s: &mut App| { s.limits_error = None; s.refresh_limits(); s.rescan_usage(); })
        .build(k)
        .key("usage-refresh"));
    let done = Button::new("Done")
        .variant(ButtonVariant::Outline)
        .on_press(|s: &mut App| s.close_usage())
        .build(k)
        .key("usage-done");
    let sub = match &s.limits {
        Some(l) if l.plan.is_some() => format!("{} plan · Claude Code. Esc closes.", plan_title(l)),
        _ => "Claude Code. Esc closes.".to_owned(),
    };
    let head = row().w_full().center_y().gap(12.0).pb(4.0)
        .child(col().grow(1.0).min_w(0.0).gap(4.0)
            .child(text("Usage").font(TextStyle { size: 26.0, line_height: 32.0, ..k.ty.body_medium }).color(c.text_strong))
            .child(text(sub).font(help_style(k)).color(c.text_muted)))
        .child(refresh)
        .child(done);

    let sum = s.usage.summary(usage::today(), s.usage_days);
    let periods = SegmentedControl::new(&USAGE_PERIODS.iter().map(|(_, l)| *l).collect::<Vec<_>>(), USAGE_PERIODS.iter().position(|(d, _)| *d == s.usage_days))
        .label("Period")
        .fit()
        .on_pick(|s: &mut App, i: usize, _: &mut Tx| s.set_usage_days(USAGE_PERIODS[i].0))
        .build(k)
        .key("usage-period");
    let title = section_title(k, "Spend at API prices", Some("What Claude Code's tokens would cost on the API, from its transcripts: terminal sessions and this app's alike. On a subscription it measures use; it isn't a bill."));
    // Beside the title, or under it once the page (as last laid out) is too narrow for both.
    let width = rune::memo::layout_with("usage-page", |r| r[2]);
    let narrow = width.is_some_and(|w| w < 560.0);
    // Four tiles need about 150 points each before their figures are cut.
    let paired = width.is_some_and(|w| w < 640.0);
    let spend_head = if narrow {
        col().w_full().gap(12.0).child(title).child(row().px(4.0).child(periods))
    } else {
        row().w_full().gap(12.0)
            .child(title.grow(1.0).min_w(0.0))
            .child(col().pt(18.0).child(periods))
    };

    let mut page = col().key("usage-page").w_full().max_w(W).gap(12.0)
        .child(head)
        .child(section_title(k, "Plan limits", Some("Read from Claude Code (as /usage shows them) every 5 minutes and after each turn.")))
        .child(limits_card(s, now, k))
        .child(spend_head)
        .child(kpis(&sum, s.usage_days, paired, k));
    if s.usage_days > 1 {
        page = page.child(card(k, vec![chart(&sum, k)]));
    }
    page = page
        .child(section_title(k, "By model", None))
        .child(models_card(&sum, k))
        .child(section_title(k, "By project", None))
        .child(projects_card(&sum, k))
        .child(footnote(s, &sum, k));
    col().key("usage").w_full().grow(1.0).min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .scroll_edge_fade(18.0)
        .child(row().w_full().justify_center().px(32.0).pt(28.0).pb(40.0).child(page))
}

fn limits_card(s: &App, now: SystemTime, k: &Kit) -> El {
    let c = &k.c;
    let note = |t: String| row().w_full().px(18.0).py(16.0).child(text(t).font(help_style(k)).color(c.text_muted).wrap());
    if s.binary(Harness::Claude).is_none() {
        return card(k, vec![note("Claude Code isn't installed, so there's no plan to read.".into())]);
    }
    let Some(l) = &s.limits else {
        let t = match &s.limits_error {
            Some(e) => format!("Couldn't read the plan's limits: {e}"),
            None => "Reading the plan's limits…".into(),
        };
        return card(k, vec![note(t)]);
    };
    if !l.available {
        return card(k, vec![note("Claude Code is signed in without a plan (an API key or a cloud provider), so it has no usage windows: every token is billed. The spend below is what it has cost.".into())]);
    }
    let mut rows: Vec<El> = l.windows.iter().map(|w| row().w_full().px(18.0).py(14.0).child(meter(w, now, k, false))).collect();
    if let Some(sp) = &l.spend {
        rows.push(row().w_full().px(18.0).py(12.0).child(spend_line(sp, k)));
    }
    let mut checked = format!("Read {}", ago(l.checked));
    if let Some(e) = &s.limits_error { checked = format!("{checked} · the last read failed: {e}"); }
    rows.push(row().w_full().px(18.0).py(10.0).child(text(checked).font(TextStyle { size: 12.0, line_height: 16.0, ..k.ty.body }).color(c.text_faint).truncate()));
    card(k, rows)
}

/// Cost, tokens, cache share and messages for the period, as stat tiles.
/// Four tiles in a row, or two rows of two when `paired` (a page too narrow for four).
fn kpis(sum: &Summary, days: u32, paired: bool, k: &Kit) -> El {
    let c = &k.c;
    let per_day = if days > 1 { format!("{} a day on average", dollars(sum.cost / days as f64)) } else { "so far today".into() };
    let input = sum.tokens.input + sum.tokens.cache_read + sum.tokens.cache_write + sum.tokens.cache_write_1h;
    let cached = if input > 0 { format!("{:.0}%", sum.tokens.cache_read as f64 * 100.0 / input as f64) } else { "–".into() };
    let tile = |key: &str, label: &str, value: String, note: String| {
        col().key(key.to_owned()).w(0.0).grow(1.0).min_w(0.0).gap(4.0).px(16.0).py(14.0)
            .radius(12.0).fill(c.surface_raised_base).border(1.0, c.border_weak)
            .child(text(label.to_owned()).font(TextStyle { size: 12.0, line_height: 16.0, ..k.ty.body }).color(c.text_muted))
            .child(text(value).font(TextStyle { size: 24.0, line_height: 30.0, ..k.ty.body_medium }).color(c.text_strong).truncate())
            .child(text(note).font(TextStyle { size: 12.0, line_height: 16.0, ..k.ty.body }).color(c.text_faint).truncate())
    };
    let cost = tile("kpi-cost", "Cost", dollars(sum.cost), per_day);
    let tokens = tile("kpi-tokens", "Tokens", compact(sum.tokens.total()), format!("{} out", compact(sum.tokens.output)));
    let cache = tile("kpi-cache", "Read from cache", cached, "of input tokens".into());
    let messages = tile("kpi-messages", "Responses", compact(sum.messages as u64), format!("{} model{}", sum.models.len(), if sum.models.len() == 1 { "" } else { "s" }));
    if paired {
        col().w_full().gap(10.0)
            .child(row().w_full().gap(10.0).child(cost).child(tokens))
            .child(row().w_full().gap(10.0).child(cache).child(messages))
    } else {
        row().w_full().gap(10.0).child(cost).child(tokens).child(cache).child(messages)
    }
}

/// Cost by day: one column a day from a single baseline, one hue, the largest day labelled,
/// every day's figures on hover.
fn chart(sum: &Summary, k: &Kit) -> El {
    let c = &k.c;
    let max = sum.days.iter().map(|d| d.1).fold(0.0, f64::max);
    let small = TextStyle { size: 11.0, line_height: 14.0, ..k.ty.body }.features("'tnum' 1");
    let top = nice_ceiling(max);
    let mut plot = row().key("chart-plot").w_full().h(CHART_H).gap(if sum.days.len() > 45 { 1.0 } else { 2.0 }).items_end();
    for (day, cost, tokens) in &sum.days {
        let h = if top > 0.0 { (*cost / top) as f32 * CHART_H } else { 0.0 };
        let h = if *cost > 0.0 { h.max(2.0) } else { 0.0 };
        // The hover target is the whole slot, taller and wider than the column.
        let slot = col().key(format!("day-{day}")).w(0.0).grow(1.0).min_w(0.0).h_full().justify_end().items_center()
            .help(format!("{} · {} · {} tokens", day.format("%a %b %-d"), dollars(*cost), compact(*tokens)))
            .child(el().w_full().max_w(24.0).h(h).corners([3.0, 3.0, 0.0, 0.0]).fill(c.bg_accent));
        plot = plot.child(slot);
    }
    // A recessive top line with its value, and the baseline.
    let grid = col().absolute(0.0, 0.0).w_full().h(CHART_H)
        .child(el().w_full().h(1.0).fill(c.border_weak))
        .child(el().grow(1.0))
        .child(el().w_full().h(1.0).fill(c.border_base));
    let first = sum.days.first().map(|d| d.0.format("%b %-d").to_string()).unwrap_or_default();
    let last = sum.days.last().map(|d| d.0.format("%b %-d").to_string()).unwrap_or_default();
    let peak = sum.days.iter().max_by(|a, b| a.1.total_cmp(&b.1)).filter(|d| d.1 > 0.0);
    let head = row().w_full().gap(8.0).center_y()
        .child(text("Cost by day").font(TextStyle { size: 14.0, line_height: 20.0, ..k.ty.body_medium }).color(c.text_strong).grow(1.0))
        .child(text(peak.map_or("No spend in this period".into(), |p| format!("Most: {} on {}", dollars(p.1), p.0.format("%a %b %-d")))).font(small).color(c.text_muted));
    col().key("chart").w_full().px(18.0).py(16.0).gap(10.0)
        .label(format!("Cost by day, {first} to {last}"))
        .child(head)
        .child(row().w_full().gap(8.0)
            .child(col().w(44.0).h(CHART_H).justify_between()
                .child(text(if top > 0.0 { short_dollars(top) } else { String::new() }).font(small).color(c.text_faint).offset(0.0, -7.0))
                .child(text("$0").font(small).color(c.text_faint).offset(0.0, 7.0)))
            .child(col().grow(1.0).min_w(0.0).h(CHART_H).child(grid).child(plot)))
        .child(row().w_full().pl(52.0)
            .child(text(first).font(small).color(c.text_faint).grow(1.0))
            .child(text(last).font(small).color(c.text_faint)))
}

/// A round number at or above `x` for the chart's top line: 1, 2 or 5 times a power of ten.
fn nice_ceiling(x: f64) -> f64 {
    if x <= 0.0 { return 0.0; }
    let p = 10f64.powf(x.log10().floor());
    [1.0, 2.0, 2.5, 5.0, 10.0].into_iter().map(|m| m * p).find(|v| *v >= x).unwrap_or(10.0 * p)
}

fn short_dollars(x: f64) -> String {
    if x >= 1000.0 { format!("${}K", trim_float(x / 1000.0)) } else if x >= 1.0 { format!("${}", trim_float(x)) } else { format!("${x:.2}") }
}

fn trim_float(x: f64) -> String {
    let s = format!("{x:.1}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// `claude-opus-5-5` → "Opus 5.5", `claude-haiku-4-5-20251001` → "Haiku 4.5".
pub fn model_name(id: &str) -> String {
    let Some(rest) = id.strip_prefix("claude-") else { return id.to_owned() };
    let (rest, variant) = match rest.split_once('[') {
        Some((r, v)) => (r, format!(" [{}", v)),
        None => (rest, String::new()),
    };
    let mut words = Vec::new();
    let mut nums = Vec::new();
    for part in rest.split('-') {
        if part.len() == 8 && part.bytes().all(|b| b.is_ascii_digit()) { continue; }
        if part.bytes().all(|b| b.is_ascii_digit()) { nums.push(part); } else {
            let mut c = part.chars();
            words.push(c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default());
        }
    }
    if words.is_empty() { return id.to_owned(); }
    let mut name = words.join(" ");
    if !nums.is_empty() { name = format!("{name} {}", nums.join(".")); }
    name + &variant
}

fn models_card(sum: &Summary, k: &Kit) -> El {
    let c = &k.c;
    if sum.models.is_empty() {
        return card(k, vec![row().w_full().px(18.0).py(16.0).child(text("Nothing in this period.").font(help_style(k)).color(c.text_muted))]);
    }
    let num = TextStyle { size: 13.0, line_height: 18.0, ..k.ty.body }.features("'tnum' 1");
    let cell = |s: String, color: rune::Color, w: f32| text(s).font(num).color(color).w(w).align(rune::Align::End);
    let header = row().w_full().px(18.0).py(8.0).gap(8.0).center_y()
        .child(text("Model").font(TextStyle { size: 12.0, line_height: 16.0, ..k.ty.body }).color(c.text_faint).grow(1.0).min_w(0.0))
        .child(cell("Input".into(), c.text_faint, 72.0))
        .child(cell("Output".into(), c.text_faint, 72.0))
        .child(cell("Cache read".into(), c.text_faint, 84.0))
        .child(cell("Cache write".into(), c.text_faint, 84.0))
        .child(cell("Cost".into(), c.text_faint, 88.0));
    let mut rows = vec![header];
    for (m, cost, t, n) in &sum.models {
        let writes = t.cache_write + t.cache_write_1h;
        rows.push(row().key(format!("model-{m}")).w_full().px(18.0).py(10.0).gap(8.0).center_y()
            .child(col().grow(1.0).min_w(0.0).gap(1.0)
                .child(text(model_name(m)).font(TextStyle { size: 14.0, line_height: 20.0, ..k.ty.body }).color(c.text_strong).truncate())
                .child(text(format!("{m} · {} responses", compact(*n as u64))).font(TextStyle { size: 12.0, line_height: 16.0, ..k.ty.body }).color(c.text_faint).truncate()))
            .child(cell(compact(t.input), c.text_base, 72.0))
            .child(cell(compact(t.output), c.text_base, 72.0))
            .child(cell(compact(t.cache_read), c.text_base, 84.0))
            .child(cell(compact(writes), c.text_base, 84.0))
            .child(cell(if *cost > 0.0 || t.total() == 0 { dollars(*cost) } else { "No price".into() }, c.text_strong, 88.0)));
    }
    card(k, rows)
}

/// Projects as ranked bars, one hue, largest first; the rest fold into one row.
fn projects_card(sum: &Summary, k: &Kit) -> El {
    let c = &k.c;
    if sum.projects.is_empty() {
        return card(k, vec![row().w_full().px(18.0).py(16.0).child(text("Nothing in this period.").font(help_style(k)).color(c.text_muted))]);
    }
    const SHOWN: usize = 8;
    let mut list: Vec<(String, f64, u64)> = sum.projects.iter().take(SHOWN).cloned().collect();
    if sum.projects.len() > SHOWN {
        let rest = &sum.projects[SHOWN..];
        list.push((format!("{} more", rest.len()), rest.iter().map(|p| p.1).sum(), rest.iter().map(|p| p.2).sum()));
    }
    let top = list.iter().map(|p| p.1).fold(0.0, f64::max);
    let num = TextStyle { size: 13.0, line_height: 18.0, ..k.ty.body }.features("'tnum' 1");
    let rows = list.iter().map(|(name, cost, tokens)| {
        let frac = if top > 0.0 { (*cost / top) as f32 } else { 0.0 };
        row().key(format!("project-{name}")).w_full().px(18.0).py(10.0).gap(12.0).center_y()
            .child(text(name.clone()).font(TextStyle { size: 14.0, line_height: 20.0, ..k.ty.body }).color(c.text_base).w(180.0).truncate())
            .child(row().grow(1.0).min_w(0.0).h(8.0)
                .child(el().w_frac(frac).min_w(if frac > 0.0 { 3.0 } else { 0.0 }).h_full().corners([0.0, 3.0, 3.0, 0.0]).fill(c.bg_accent)))
            .child(text(compact(*tokens)).font(num).color(c.text_muted).w(64.0).align(rune::Align::End))
            .child(text(dollars(*cost)).font(num).color(c.text_strong).w(88.0).align(rune::Align::End))
    }).collect();
    card(k, rows)
}

fn footnote(s: &App, sum: &Summary, k: &Kit) -> El {
    let prices = match s.prices.fetched {
        Some(t) => format!("Prices from LiteLLM's table, fetched {}", ago(t)),
        None => "Prices from the built-in copy of LiteLLM's table (fetched daily when online)".into(),
    };
    let unpriced = if sum.unpriced > 0 { format!(" {} tokens are from models with no price and aren't counted in the cost.", compact(sum.unpriced)) } else { String::new() };
    let scanned = if s.usage.files > 0 { format!(" Read {} transcripts in {} ms.", s.usage.files, s.usage.took.as_millis()) } else { String::new() };
    text(format!("{prices}.{unpriced}{scanned}")).font(TextStyle { size: 12.0, line_height: 16.0, ..k.ty.body }).color(k.c.text_faint).wrap().px(4.0).pt(4.0)
}

// ---------------------------------------------------------------------------------------------
// Shared with Settings' look

fn help_style(k: &Kit) -> TextStyle {
    TextStyle { size: 13.0, line_height: 18.0, ..k.ty.body }
}

fn section_title(k: &Kit, title: &str, help: Option<&str>) -> El {
    let mut c = col().w_full().gap(2.0).pt(16.0).pb(2.0).px(4.0)
        .child(text(title.to_owned()).font(TextStyle { size: 16.0, line_height: 22.0, ..k.ty.body_medium }).color(k.c.text_strong));
    if let Some(h) = help { c = c.child(text(h.to_owned()).font(help_style(k)).color(k.c.text_muted).wrap()); }
    c
}

fn card(k: &Kit, rows: Vec<El>) -> El {
    let mut c = col().w_full().radius(12.0).fill(k.c.surface_raised_base).border(1.0, k.c.border_weak).clip_rounded(12.0);
    let n = rows.len();
    for (i, r) in rows.into_iter().enumerate() {
        c = c.child(r);
        if i + 1 < n { c = c.child(el().w_full().h(1.0).fill(k.c.border_weak)); }
    }
    c
}

/// The toolbar's Usage button.
pub fn toolbar_button(s: &App, k: &Kit) -> El {
    let key = s.keys.label(crate::shortcuts::Cmd::Usage);
    IconButton::new(&GAUGE)
        .variant(IconButtonVariant::GhostMuted)
        .expanded(s.usage_open)
        .label(if s.usage_open { "Close Usage" } else { "Usage and plan limits" })
        .shortcut(&key)
        .on_press(|s: &mut App| if s.usage_open { s.close_usage() } else { s.open_usage() })
        .build(k)
        .key("usage-toggle")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_names_read_like_the_picker() {
        assert_eq!(model_name("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(model_name("claude-haiku-4-5-20251001"), "Haiku 4.5");
        assert_eq!(model_name("claude-fable-5-1"), "Fable 5.1");
        assert_eq!(model_name("claude-opus-5-5[1m]"), "Opus 5.5 [1m]");
        assert_eq!(model_name("gpt-5"), "gpt-5");
    }

    #[test]
    fn spans_and_chart_tops() {
        assert_eq!(span(7080), "1h 58m");
        assert_eq!(span(3 * 86400 + 4 * 3600 + 59), "3d 4h");
        assert_eq!(span(30), "<1m");
        assert_eq!(nice_ceiling(37.0), 50.0);
        assert_eq!(nice_ceiling(120.0), 200.0);
        assert_eq!(nice_ceiling(0.8), 1.0);
        assert_eq!(short_dollars(2500.0), "$2.5K");
    }

    #[test]
    fn pace_compares_the_fill_with_the_window() {
        let now = SystemTime::now();
        let w = |used: f64| Window { id: "session".into(), label: "Session".into(), used, resets: Some(now + std::time::Duration::from_secs(2 * 3600)), length: Some(std::time::Duration::from_secs(5 * 3600)), severity: String::new() };
        // 60% of the window has gone.
        assert_eq!(pace(&w(80.0), now), Some("Ahead of pace"));
        assert_eq!(pace(&w(60.0), now), Some("On pace"));
        assert_eq!(pace(&w(20.0), now), Some("Under pace"));
        assert_eq!(status(&w(80.0)), Some("Near limit"));
        assert_eq!(status(&w(100.0)), Some("At limit"));
    }
}
