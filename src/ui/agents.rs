//! Subagents and background work, after T3 Code: a compact card per subagent (its status, what
//! it's doing or what it found, how long, how many tool calls; opened, its own text and tool
//! rows), "N subagents" over consecutive launches, and above the composer, what still runs in
//! the background after a turn's result, each with a stop control.

use rune::{El, Tx, col, icon, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, Spinner};
use rune_theme_opencode::icons;

use crate::app::{Agent, AgentStatus, App, Item, Kind, Task, Thread};
use crate::ui::theme::Theme;
use crate::ui::transcript::duration;

/// A row of consecutive subagent launches: one card, or "N subagents" over several.
pub fn group(s: &App, t: &Thread, item: u64, agents: &[Agent], k: &Kit, look: (Theme, bool)) -> El {
    if let [one] = agents { return card(s, t, one, k, look); }
    let c = &k.c;
    let statuses: Vec<AgentStatus> = agents.iter().map(|a| a.status(t.task_for(&a.call))).collect();
    let active = statuses.iter().any(|st| !st.settled());
    let failed = statuses.contains(&AgentStatus::Failed);
    let folded = s.folded.contains(&item);
    // From the first start to the last end (or now).
    let tasks: Vec<&Task> = agents.iter().filter_map(|a| t.task_for(&a.call)).collect();
    let elapsed = tasks.iter().map(|x| x.started).min().map(|start| {
        let end = if tasks.iter().any(|x| x.running()) { std::time::Instant::now() } else { tasks.iter().filter_map(|x| x.ended).max().unwrap_or(start) };
        end.saturating_duration_since(start).as_secs_f64()
    });
    let summary_color = if active { c.info.fg } else if failed { c.danger.fg } else { c.text_muted };
    let mut head = row().key("agents-head").w_full().h(28.0).px(4.0).gap(8.0).center_y().radius(8.0)
        .cursor(rune::Cursor::Pointer).hover(c.surface_hover)
        .role("button").label(format!("{} subagents", agents.len())).expanded(!folded)
        .child(icon(&icons::TASK, 14.0).color(c.icon_muted))
        .child(text(format!("{} subagents", agents.len())).font(k.ty.small_medium).color(c.text_base))
        .child(text(status_summary(&statuses)).font(k.ty.text_xsmall).color(summary_color).truncate().shrink().grow(1.0));
    if let Some(secs) = elapsed { head = head.child(text(duration(secs)).font(k.ty.text_xsmall).color(c.text_muted)); }
    let head = head.child(icon(if folded { &icons::CHEVRON_RIGHT } else { &icons::CHEVRON_DOWN }, 12.0).color(c.icon_weaker).a11y_hidden())
        .on_press(move |s: &mut App| if !s.folded.remove(&item) { s.folded.insert(item); });
    let mut out = col().w_full().gap(6.0).child(head);
    if !folded {
        for a in agents {
            out = out.child(card(s, t, a, k, look).key(format!("agent-{}", a.id)));
        }
    }
    out
}

/// "1 running · 2 done", as T3 Code counts them.
pub fn status_summary(statuses: &[AgentStatus]) -> String {
    let count = |f: &dyn Fn(AgentStatus) -> bool| statuses.iter().filter(|s| f(**s)).count();
    let parts = [
        (count(&|s| s == AgentStatus::Running), "running"),
        (count(&|s| s == AgentStatus::Background), "in the background"),
        (count(&|s| s == AgentStatus::Completed), "done"),
        (count(&|s| s == AgentStatus::Failed), "failed"),
        (count(&|s| s == AgentStatus::Stopped), "stopped"),
    ];
    parts.iter().filter(|(n, _)| *n > 0).map(|(n, w)| format!("{n} {w}")).collect::<Vec<_>>().join(" · ")
}

/// A card's one line: what it's doing while it runs, what it found once it's done.
pub fn detail(a: &Agent, task: Option<&Task>) -> Option<String> {
    let status = a.status(task);
    let progress = task.and_then(|x| x.progress.as_deref()).map(plain).filter(|p| !p.is_empty());
    let report = Some(plain(&a.report(task))).filter(|r| !r.is_empty());
    if status.settled() { report.or(progress) } else { progress.or(report) }
}

/// One line of text from Markdown: links to their words, no code ticks or bullets, the
/// spacing folded.
fn plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    // [words](link) → words
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find("](").map(|i| open + i) else { break };
        let Some(end) = rest[close..].find(')').map(|i| close + i) else { break };
        out.push_str(&rest[..open]);
        out.push_str(&rest[open + 1..close]);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    let lines: Vec<&str> = out.lines().map(|l| l.trim().trim_start_matches(['-', '*']).trim_start()).collect();
    lines.join(" ").replace('`', "").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A subagent's card, as T3 Code's link to a subagent's thread: a status mark, its
/// description and one line, its tool calls and time, and a chevron; pressing it opens the
/// agent's view (`App::open_agent`), all its runs and their rows. A run a `SendMessage` started
/// says it's a resume; an earlier run, that it was resumed below. A tool inside waiting for
/// approval shows under it, so it can be answered from here.
pub fn card(s: &App, t: &Thread, a: &Agent, k: &Kit, look: (Theme, bool)) -> El {
    let c = &k.c;
    let task = t.task_for(&a.call);
    let status = a.status(task);
    let line = detail(a, task);
    let failed = status == AgentStatus::Failed;
    let later = t.resumed_since(a);
    let mark = match status {
        AgentStatus::Running | AgentStatus::Background => Spinner::new().size(12.0).build(k),
        _ => status_dot(status, k),
    };
    let mut title = row().w_full().gap(6.0).center_y()
        .child(text(if a.description.is_empty() { "Subagent".to_owned() } else { a.description.clone() }).font(k.ty.small_medium).color(c.text_base).truncate().shrink());
    if a.resumed {
        title = title.child(row().gap(3.0).center_y()
            .child(icon(&icons::OUTLINE_ROTATE_CLOCKWISE, 11.0).color(c.icon_weaker))
            .child(text("resumed").font(k.ty.text_xsmall).color(c.text_faint)));
    }
    if let Some(kind) = task.and_then(|x| x.subagent_type.as_deref()).filter(|k| *k != "general-purpose") {
        title = title.child(text(kind.to_owned()).font(k.ty.text_xsmall).color(c.text_faint).truncate().shrink());
    }
    let words = col().grow(1.0).shrink().min_w(0.0).gap(1.0)
        .child(title)
        .child(text(line.clone().unwrap_or_else(|| status.label().to_owned())).font(k.ty.text_xsmall).color(if failed { c.danger.fg } else { c.text_muted }).truncate());
    let mut head = row().key("head").w_full().px(10.0).py(7.0).gap(10.0).center_y().radius(8.0)
        .child(row().w(14.0).center().child(mark))
        .child(words);
    // Where it went on: a later run's card, further down.
    if later {
        head = head.child(row().gap(4.0).center_y()
            .child(icon(&icons::ARROW_DOWN_TO_LINE, 12.0).color(c.icon_weaker))
            .child(text("Resumed below").font(k.ty.text_xsmall).color(c.text_muted)));
    } else if line.is_some() && status != AgentStatus::Completed {
        // The status in words, unless the line already is it, or it's the expected "done".
        head = head.child(text(status.label()).font(k.ty.text_xsmall).color(if failed { c.danger.fg } else { c.text_muted }));
    }
    if let Some(n) = task.and_then(|x| x.usage).map(|u| u.tool_uses).filter(|n| *n > 0) {
        head = head.child(text(tools_label(n)).font(k.ty.text_xsmall).color(c.text_faint));
    }
    if let Some(x) = task {
        head = head.child(text(duration(x.elapsed().as_secs_f64())).font(k.ty.text_xsmall).color(c.text_muted));
    }
    let (thread, id) = (t.id, a.id);
    let head = head.child(icon(&icons::CHEVRON_RIGHT, 12.0).color(c.icon_weaker).a11y_hidden())
        .hover(c.surface_hover).cursor(rune::Cursor::Pointer)
        .role("button").label(format!("Open {}", if a.description.is_empty() { "subagent" } else { a.description.as_str() }))
        .help("Open this agent")
        .on_press(move |s: &mut App| s.open_agent(thread, id));
    let mut card = col().w_full().radius(9.0).border(1.0, c.border_weak).clip_rounded(9.0).child(head);
    let asking = waiting(t, &a.items);
    if !asking.is_empty() {
        let mut body = col().key("asking").w_full().px(10.0).pt(4.0).pb(10.0).gap(6.0);
        for item in asking {
            body = body.child(crate::ui::transcript::tool_row(s, t, item, k, look).key(format!("ask-{}", item.id)));
        }
        card = card.child(rune::el().w_full().h(1.0).fill(c.border_weak)).child(body);
    }
    card
}

/// "1 tool", "12 tools".
pub fn tools_label(n: u64) -> String {
    if n == 1 { "1 tool".to_owned() } else { format!("{n} tools") }
}

/// The tool rows in `items` (and in the cards inside them) waiting for approval.
fn waiting<'a>(t: &Thread, items: &'a [Item]) -> Vec<&'a Item> {
    let mut out = Vec::new();
    for i in items {
        match &i.kind {
            Kind::Tool { .. } if t.ask_on(i.id).is_some() => out.push(i),
            Kind::Agents(g) => for a in g { out.extend(waiting(t, &a.items)); },
            _ => {}
        }
    }
    out
}

fn status_dot(status: AgentStatus, k: &Kit) -> El {
    let c = &k.c;
    let color = match status {
        AgentStatus::Running | AgentStatus::Background => c.info.icon,
        AgentStatus::Completed => c.success.icon,
        AgentStatus::Failed => c.danger.icon,
        AgentStatus::Stopped => c.icon_faint,
    };
    rune::el().size(8.0, 8.0).radius(4.0).fill(color).a11y_hidden()
}

/// A key for what's open inside a row of subagent cards (cards, their tool rows, nested
/// groups), so its memo is rebuilt when one opens.
pub fn open_key(s: &App, item: u64, agents: &[Agent]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.folded.contains(&item).hash(&mut h);
    fn walk(s: &App, agents: &[Agent], h: &mut std::collections::hash_map::DefaultHasher) {
        for a in agents {
            s.expanded.contains(&a.id).hash(h);
            for i in &a.items {
                s.expanded.contains(&i.id).hash(h);
                s.folded.contains(&i.id).hash(h);
                if let Kind::Agents(g) = &i.kind { walk(s, g, h); }
            }
        }
    }
    walk(s, agents, &mut h);
    h.finish()
}

/// Whether a row of cards is still changing on its own (a running agent's time).
pub fn live(t: &Thread, agents: &[Agent]) -> bool {
    agents.iter().any(|a| !a.status(t.task_for(&a.call)).settled() || a.items.iter().any(|i| match &i.kind {
        Kind::Agents(g) => live(t, g),
        Kind::Assistant { streaming, .. } => *streaming,
        _ => false,
    }))
}

/// Above the composer while the thread's process still runs work in the background after a
/// turn's result: what runs, for how long and what it's doing, each with a stop control.
pub fn background(s: &App, t: &Thread, k: &Kit) -> Option<El> {
    if !t.has_background() { return None; }
    let c = &k.c;
    let id = t.id;
    let stop_all = Button::new("Stop all")
        .variant(ButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .on_press(move |s: &mut App| s.ask_stop_background(id))
        .build(k)
        .key("stop-background");
    let head = row().key("bg-head").w_full().h(30.0).pl(12.0).pr(4.0).gap(8.0).center_y()
        .child(Spinner::new().size(12.0).build(k))
        .child(text(format!("Running in background · {}", t.background_counts())).font(k.ty.small_medium).color(c.text_base).truncate().shrink().grow(1.0))
        .child(stop_all);
    let mut card = col().key("background").w_full().radius(12.0).fill(c.surface_raised_base).border(1.0, c.border_weak).pb(4.0).child(head);
    for b in &t.background {
        let task = t.task(&b.id);
        let agent = b.kind == "local_agent";
        let label = if b.description.trim().is_empty() { if agent { "Subagent".to_owned() } else { "Command".to_owned() } } else { crate::harness::first_line(&b.description, 120) };
        let progress = task.and_then(|x| x.progress.clone()).filter(|_| agent).map(|p| plain(&p)).unwrap_or_default();
        let tid = b.id.clone();
        let stop = IconButton::new(&icons::CLOSE_SMALL)
            .variant(IconButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .label(format!("Stop {label}"))
            .help(format!("Stop {label}"))
            .on_press_tx(move |s: &mut App, _: &mut Tx| s.stop_task(id, &tid))
            .build(k)
            .key(format!("stop-{}", b.id));
        let mut r = row().key(format!("bg-{}", b.id)).w_full().h(26.0).pl(8.0).pr(4.0).gap(8.0).center_y().radius(7.0)
            .child(icon(if agent { &icons::TASK } else { &icons::CONSOLE }, 13.0).color(c.icon_weaker))
            .child(text(label.clone()).font(if agent { k.ty.small } else { k.ty.mono }).color(c.text_base).truncate().shrink());
        r = if progress.is_empty() {
            r.child(rune::el().grow(1.0))
        } else {
            r.child(text(progress).font(k.ty.text_xsmall).color(c.text_muted).truncate().shrink().grow(1.0))
        };
        if let Some(x) = task {
            r = r.child(text(duration(x.elapsed().as_secs_f64())).font(k.ty.text_xsmall).color(c.text_muted));
        }
        // An agent opens, as its card does: what it's doing, all of it.
        let opens = agent && !t.runs(&b.id).is_empty();
        if opens {
            let task = b.id.clone();
            r = r.child(icon(&icons::CHEVRON_RIGHT, 12.0).color(c.icon_weaker).a11y_hidden())
                .hover(c.surface_hover).cursor(rune::Cursor::Pointer)
                .role("button").label(format!("Open {label}")).help("Open this agent")
                .on_press(move |s: &mut App| s.open_agent_task(id, &task));
        } else {
            // Its time lines up with the rows that open.
            r = r.child(rune::el().w(12.0));
        }
        card = card.child(row().w_full().px(4.0).child(r.child(stop)));
    }
    let _ = s;
    Some(card)
}

/// How a task is named in a sentence: its description, quoted.
pub fn task_name(description: &str) -> String {
    format!("\u{201c}{}\u{201d}", crate::harness::first_line(description, 60))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_reads_as_one_line() {
        assert_eq!(plain("Found **3** files in [src](src/)\n- `a.rs`\n- b.rs"), "Found **3** files in src a.rs b.rs");
        assert_eq!(plain("  485  "), "485");
    }

    #[test]
    fn statuses_are_counted_as_t3_does() {
        use AgentStatus::*;
        assert_eq!(status_summary(&[Running, Completed, Completed, Failed]), "1 running · 2 done · 1 failed");
        assert_eq!(status_summary(&[Background]), "1 in the background");
    }
}
