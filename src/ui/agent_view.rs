//! An agent opened in the main pane, as T3 Code opens a subagent as a thread of its own: what
//! it was asked, every message and tool call of its own (opening as the conversation's do), the
//! agents it started as cards that open in turn, following it live. Each run of it is here, an
//! earlier one (before a `SendMessage` resumed it) above the next. A header says what it is and
//! how it's doing, and goes back (as Esc does) to where it was opened from.

use std::borrow::Cow;
use std::rc::Rc;

use rune::{El, GrowthSpec, ScrollSpec, col, icon, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, Markdown, Spinner};
use rune_theme_opencode::icons;

use crate::app::{Agent, AgentStatus, AgentView, App, Item, Kind, Thread};
use crate::ui::agents::{detail, tools_label};
use crate::ui::theme::{self, Theme};
use crate::ui::transcript::{MAX_W, duration};

/// Where a run's rows come from: a card in the thread, or the saved transcript
/// (`AgentView::earlier`).
#[derive(Clone, Copy)]
enum Run {
    Card(u64),
    Saved(usize),
}

/// A row of the view: a run's start (a divider for a resume, and what it was asked), one of
/// its rows, its report where none of its messages says it, and the agent at work.
#[derive(Clone, Copy)]
enum Line {
    Start(usize),
    Item(usize, usize),
    Report(usize),
    Working,
}

/// The agent view over the selected thread, if one is open: its header over its rows.
pub fn view(s: &App, k: &Kit, look: (Theme, bool)) -> Option<El> {
    let v = s.open_agent_view()?;
    let t = s.threads.iter().find(|t| t.id == v.thread)?;
    let cards = runs_of(t, v);
    let latest = *cards.last()?;
    let mut runs: Vec<Run> = (0..v.earlier.len()).map(Run::Saved).collect();
    runs.extend(cards.iter().map(|a| Run::Card(a.id)));
    let working = !latest.status(t.task_for(&latest.call)).settled();
    let mut lines = Vec::new();
    for (r, run) in runs.iter().enumerate() {
        lines.push(Line::Start(r));
        let n = match run {
            Run::Card(id) => t.card(*id).map_or(0, |a| a.items.len()),
            Run::Saved(i) => v.earlier[*i].1.len(),
        };
        lines.extend((0..n).map(|i| Line::Item(r, i)));
        if let Run::Card(id) = run && t.card(*id).is_some_and(|a| report(t, a).is_some()) { lines.push(Line::Report(r)); }
    }
    if working { lines.push(Line::Working); }
    let keys: Vec<String> = lines.iter().map(|l| match l {
        Line::Start(r) => format!("start-{r}"),
        Line::Item(r, i) => item_of(t, v, runs[*r], *i).map_or_else(|| format!("x{r}-{i}"), |it| format!("i{}", it.id)),
        Line::Report(r) => format!("report-{r}"),
        Line::Working => "working".to_owned(),
    }).collect();
    let count = lines.len();
    let shared = Rc::new((runs, lines, keys));
    let rows_key = format!("agent-rows-{}-{}", v.thread, cards[0].id);
    let list = col().key(format!("agent-scroll-{}-{}", v.thread, cards[0].id)).w_full().grow(1.0).min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: true })
        .scroll_edge_fade(18.0)
        .child(col().key(rows_key).w_full().growth(GrowthSpec::default()).virtual_rows(
            count,
            64.0,
            {
                let shared = shared.clone();
                move |_: &App, i| Cow::Owned(shared.2[i].clone())
            },
            {
                let k = k.clone();
                move |s: &App, i| {
                    let (runs, lines, _) = &*shared;
                    let body = s.open_agent_view()
                        .and_then(|v| Some((v, s.threads.iter().find(|t| t.id == v.thread)?)))
                        .map_or_else(rune::el, |(v, t)| line(s, t, v, runs, lines[i], &k, look));
                    row().w_full().justify_center().px(24.0).pt(if i == 0 { 18.0 } else { 6.0 }).pb(if i + 1 == lines.len() { 28.0 } else { 6.0 })
                        .child(col().w_full().max_w(MAX_W).child(body))
                }
            },
        ));
    Some(col().key("agent-view").w_full().h_full().min_h(0.0)
        .child(header(s, t, latest, cards.len() + v.earlier.len(), k))
        .child(rune::el().w_full().h(1.0).fill(k.c.border_weak))
        .child(list))
}

/// The runs of the agent `v` shows, oldest first: every card of it (by its id), else the card
/// it was opened from.
fn runs_of<'a>(t: &'a Thread, v: &AgentView) -> Vec<&'a Agent> {
    match &v.task {
        Some(id) => {
            let runs = t.runs(id);
            if runs.is_empty() { t.card(v.card).into_iter().collect() } else { runs }
        }
        None => t.card(v.card).into_iter().collect(),
    }
}

fn item_of<'a>(t: &'a Thread, v: &'a AgentView, run: Run, i: usize) -> Option<&'a Item> {
    match run {
        Run::Card(id) => t.card(id)?.items.get(i),
        Run::Saved(r) => v.earlier.get(r)?.1.get(i),
    }
}

/// A finished run's report, unless one of its own messages says it already.
fn report(t: &Thread, a: &Agent) -> Option<String> {
    let task = t.task_for(&a.call);
    let report = a.report(task);
    let said = a.items.iter().any(|i| matches!(&i.kind, Kind::Assistant { text, .. } if text.trim() == report.trim()));
    (!report.is_empty() && !said && a.status(task).settled()).then_some(report)
}

fn line(s: &App, t: &Thread, v: &AgentView, runs: &[Run], l: Line, k: &Kit, look: (Theme, bool)) -> El {
    match l {
        Line::Start(r) => {
            let (resume, asked) = match runs[r] {
                Run::Card(id) => match t.card(id) {
                    Some(a) => (a.resumed || r > 0, a.asked(t.task_for(&a.call))),
                    None => (r > 0, None),
                },
                Run::Saved(i) => (i > 0, v.earlier[i].0.clone()),
            };
            let saved = matches!(runs[r], Run::Saved(_));
            start(resume, saved, asked.as_deref(), k)
        }
        Line::Item(r, i) => match item_of(t, v, runs[r], i) {
            Some(item) => crate::ui::transcript::item_view(s, t, item, k, look),
            None => rune::el(),
        },
        Line::Report(r) => {
            let Run::Card(id) = runs[r] else { return rune::el() };
            match t.card(id).and_then(|a| report(t, a)) {
                Some(rep) => Markdown::new(rep.as_str()).style(theme::markdown(look.0, k)).selectable(true).build(k),
                None => rune::el(),
            }
        }
        Line::Working => {
            let latest = runs.iter().rev().find_map(|r| match r { Run::Card(id) => t.card(*id), _ => None });
            working(t, latest, k)
        }
    }
}

/// A run's start: a divider for a resume (or a run read back from the saved transcript), then
/// what it was asked, as the person's messages show in a thread.
fn start(resume: bool, saved: bool, asked: Option<&str>, k: &Kit) -> El {
    let c = &k.c;
    let mut out = col().w_full().gap(10.0);
    if resume || saved {
        let label = match (resume, saved) {
            (true, true) => "Resumed · from the saved transcript",
            (true, false) => "Resumed",
            _ => "From the saved transcript",
        };
        out = out.child(row().key("divider").w_full().gap(10.0).center_y().py(4.0)
            .child(rule(k))
            .child(row().gap(6.0).center_y()
                .child(icon(&icons::OUTLINE_ROTATE_CLOCKWISE, 12.0).color(c.icon_weaker))
                .child(text(label).font(k.ty.text_xsmall).color(c.text_muted)))
            .child(rule(k)));
    }
    if let Some(p) = asked {
        out = out.child(crate::ui::transcript::user(p, k).key("asked"));
    }
    out
}

fn rule(k: &Kit) -> El {
    rune::el().h(1.0).grow(1.0).fill(k.c.border_weak)
}

/// The agent at work, at the foot of its rows: how long its run has gone on (the header says
/// what it's doing).
fn working(t: &Thread, a: Option<&Agent>, k: &Kit) -> El {
    let secs = a.and_then(|a| t.task_for(&a.call)).map_or(0.0, |x| x.elapsed().as_secs_f64());
    row().key("working").w_full().gap(8.0).center_y().h(24.0).px(2.0)
        .child(Spinner::new().size(12.0).build(k))
        .child(text(format!("Working…  {}", duration(secs))).font(k.ty.small).color(k.c.text_muted))
}

/// Back to where it was opened from; what the agent is (its description and kind), how it's
/// doing (its status, tool calls, time and model) and what it's doing now.
fn header(s: &App, t: &Thread, latest: &Agent, runs: usize, k: &Kit) -> El {
    let c = &k.c;
    let task = t.task_for(&latest.call);
    let status = latest.status(task);
    // Opened from another agent's view: back to that one.
    let back_to = match s.agent_view.len() {
        n if n >= 2 => {
            let prev = &s.agent_view[n - 2];
            t.card(prev.card).map(|a| a.description.clone()).filter(|d| !d.is_empty()).unwrap_or_else(|| "agent".to_owned())
        }
        _ => t.title.clone(),
    };
    let back_to = crate::harness::first_line(&back_to, 48);
    let back = Button::new(format!("Back to {back_to}"))
        .variant(ButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .icon(&icons::ARROW_LEFT)
        .help(format!("Back to {back_to} (esc)"))
        .on_press(|s: &mut App| { s.close_agent(); })
        .build(k)
        .key("agent-back");
    let mark = match status {
        AgentStatus::Running | AgentStatus::Background => Spinner::new().size(14.0).build(k),
        _ => {
            let color = match status {
                AgentStatus::Completed => c.success.icon,
                AgentStatus::Failed => c.danger.icon,
                _ => c.icon_faint,
            };
            rune::el().size(9.0, 9.0).radius(4.5).fill(color).a11y_hidden()
        }
    };
    let name = if latest.description.is_empty() { "Subagent".to_owned() } else { latest.description.clone() };
    let mut title = row().key("agent-title").w_full().gap(10.0).center_y()
        .child(row().w(16.0).center().child(mark))
        .child(text(name).font(rune::TextStyle { size: 17.0, line_height: 24.0, ..k.ty.body_medium }).color(c.text_strong).truncate().shrink());
    if let Some(kind) = task.and_then(|x| x.subagent_type.as_deref()) {
        title = title.child(row().h(20.0).px(7.0).radius(10.0).border(1.0, c.border_weak).center_y()
            .child(text(kind.to_owned()).font(k.ty.text_xsmall).color(c.text_muted)));
    }
    // How it's doing, as T3 Code's subagent bar says it.
    let mut facts = vec![status.label().to_owned()];
    if let Some(n) = task.and_then(|x| x.usage).map(|u| u.tool_uses).filter(|n| *n > 0) { facts.push(tools_label(n)); }
    if let Some(x) = task { facts.push(duration(x.elapsed().as_secs_f64())); }
    if runs > 1 { facts.push(format!("{runs} runs")); }
    if let Some(m) = latest.model.as_deref().or_else(|| t.runs(latest.task.as_deref().unwrap_or_default()).iter().rev().find_map(|a| a.model.as_deref())) {
        facts.push(m.to_owned());
    }
    let failed = status == AgentStatus::Failed;
    let facts = text(facts.join("  ·  ")).key("agent-facts").font(k.ty.text_xsmall).color(if failed { c.danger.fg } else { c.text_muted }).truncate().shrink();
    let top = row().w_full().gap(8.0).center_y()
        .child(back)
        .child(rune::el().grow(1.0))
        .child(text("esc").font(k.ty.text_xsmall).color(c.text_faint));
    let mut body = col().w_full().max_w(MAX_W).gap(6.0)
        .child(top)
        .child(col().w_full().gap(3.0).pl(4.0).child(title).child(row().pl(26.0).w_full().child(facts)));
    // What it's doing now, or what it found.
    if let Some(d) = detail(latest, task) {
        body = body.child(text(d).key("agent-doing").font(k.ty.small).color(if failed { c.danger.fg } else { c.text_base }).truncate().pl(30.0));
    }
    row().key("agent-header").w_full().justify_center().px(24.0).pt(10.0).pb(12.0).child(body)
}
