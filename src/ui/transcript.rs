//! The conversation: a virtualised list that sticks to the bottom while a turn streams.
//!
//! Only the rows near the viewport are built (`virtual_rows`), and every finished row sits in a
//! `memo` keyed by its revision, so a streaming delta rebuilds one Markdown block, not the
//! whole transcript.

use std::borrow::Cow;

use rune::{El, GrowthSpec, Icon, ScrollSpec, col, icon, memo, row, text};
use crate::ui::theme::{self, Theme};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, Markdown, Spinner};
use rune_theme_opencode::icons;

use crate::app::{App, Item, Kind, Pending, Thread, ToolState};
use crate::harness::Decision;
use crate::shortcuts::Cmd;

/// The column the conversation reads in.
pub const MAX_W: f32 = 760.0;

fn rows(s: &App) -> usize {
    s.thread().map_or(0, |t| t.items.len() + t.busy as usize)
}

/// `look` is the theme and appearance: finished messages are memoised on it.
pub fn view(s: &App, k: &Kit, look: (Theme, bool)) -> El {
    let n = rows(s);
    let thread = s.thread().map_or(0, |t| t.id);
    col().key("transcript").w_full().grow(1.0).min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: true })
        .scroll_edge_fade(18.0)
        .child(
            col().key(format!("rows-{thread}")).w_full().growth(GrowthSpec::default()).virtual_rows(
                n,
                64.0,
                |s: &App, i| {
                    let t = s.thread().unwrap();
                    match t.items.get(i) {
                        Some(item) => Cow::Owned(format!("i{}", item.id)),
                        None => Cow::Borrowed("working"),
                    }
                },
                {
                    let k = k.clone();
                    move |s: &App, i| {
                        let t = s.thread().unwrap();
                        let last = i + 1 == rows(s);
                        let body = match t.items.get(i) {
                            Some(item) => item_view(s, t, item, &k, look),
                            None => working(t, &s.keys.label(crate::shortcuts::Cmd::Stop), &k),
                        };
                        // Rows carry their own vertical padding: virtual rows take no gaps.
                        row().w_full().justify_center().px(24.0).pt(if i == 0 { 20.0 } else { 6.0 }).pb(if last { 24.0 } else { 6.0 })
                            .child(col().w_full().max_w(MAX_W).child(body))
                    }
                },
            ),
        )
}

fn item_view(s: &App, t: &Thread, item: &Item, k: &Kit, look: (Theme, bool)) -> El {
    let streaming = matches!(item.kind, Kind::Assistant { streaming: true, .. });
    let open = s.expanded.contains(&item.id);
    let build = || match &item.kind {
        Kind::User(m) => user(m, k),
        Kind::Assistant { text, streaming } => {
            Markdown::new(text.as_str()).style(theme::markdown(look.0, k)).selectable(true).streaming(*streaming).build(k)
        }
        Kind::Tool { name, detail, state, output, .. } => {
            let card = tool(item.id, name, detail, *state, output, open, t.ask_on(item.id).is_some(), k);
            match t.ask_on(item.id) {
                Some(p) => card.child(approval(s, t.id, p, k)),
                None => card,
            }
        }
        Kind::Error(m) => error(m, k),
        Kind::TurnEnd { secs, cost_usd, stopped } => turn_end(*secs, *cost_usd, *stopped, t, k),
    };
    if streaming {
        // A streaming message changes every delta: building it is the work.
        build().streaming(true)
    } else {
        memo(format!("item-{}", item.id), (item.rev, open, look), build)
    }
}

fn user(m: &str, k: &Kit) -> El {
    let c = &k.c;
    row().w_full().justify_end().child(
        col().max_w(MAX_W * 0.78).px(14.0).py(10.0).radius(16.0).fill(c.surface_raised_base).border(1.0, c.border_weak)
            .child(text(m.to_owned()).font(k.ty.body).color(c.text_strong).wrap().selectable(true).selection_color(k.selection())),
    )
}

fn tool_icon(name: &str) -> &'static Icon {
    match name.to_ascii_lowercase().as_str() {
        "bash" | "shell" | "run_shell_command" => &icons::CONSOLE,
        "edit" | "write" | "multiedit" | "notebookedit" | "replace" | "write_file" => &icons::PENCIL_LINE,
        "read" | "read_file" | "read_many_files" => &icons::FILE_DOCUMENT,
        "grep" | "glob" | "search" | "search_file_content" | "list_directory" | "ls" => &icons::MAGNIFYING_GLASS,
        "webfetch" | "websearch" | "web_fetch" | "google_web_search" => &icons::GLOBE,
        "task" | "agent" => &icons::TASK,
        "todowrite" => &icons::CHECKLIST,
        _ => &icons::CODE,
    }
}

#[allow(clippy::too_many_arguments)]
fn tool(id: u64, name: &str, detail: &str, state: ToolState, output: &str, open: bool, waiting: bool, k: &Kit) -> El {
    let c = &k.c;
    let status = match state {
        ToolState::Running if waiting => text("Needs approval").font(k.ty.text_xsmall_medium).color(c.text_base),
        ToolState::Running => Spinner::new().size(12.0).build(k),
        ToolState::Ok => icon(&icons::CHECK_SMALL, 14.0).color(c.icon_weaker),
        ToolState::Failed => icon(&icons::CIRCLE_X, 13.0).color(c.danger.fg),
    };
    let has_output = !output.trim().is_empty();
    let mut head = row().key("head").w_full().h(30.0).px(10.0).gap(8.0).center_y().radius(8.0)
        .child(icon(tool_icon(name), 14.0).color(c.icon_muted))
        .child(text(name.to_owned()).font(k.ty.small_medium).color(c.text_base))
        .child(text(detail.to_owned()).font(k.ty.mono).color(c.text_weak).truncate().shrink().grow(1.0))
        .child(status);
    if has_output {
        head = head.hover(c.surface_hover).cursor(rune::Cursor::Pointer).on_press(move |s: &mut App| {
            if !s.expanded.remove(&id) { s.expanded.insert(id); }
        });
    }
    let mut card = col().w_full().radius(9.0).border(1.0, c.border_weak).child(head);
    if open && has_output {
        let shown: String = output.lines().take(40).collect::<Vec<_>>().join("\n");
        card = card.child(
            col().key("output").w_full().px(12.0).py(8.0).max_h(320.0).scroll(ScrollSpec { stick_bottom: false })
                .child(text(shown).font(k.ty.mono).color(c.text_weak).wrap_anywhere().selectable(true)),
        );
    }
    card
}

/// Under a tool row waiting for approval: the full command (or path), why the CLI asks, and
/// Deny / Always allow / Allow. Esc stops the whole turn instead.
fn approval(s: &App, thread: u64, p: &Pending, k: &Kit) -> El {
    let c = &k.c;
    let item = p.item;
    let mut body = col().key("ask").w_full().px(12.0).pt(2.0).pb(10.0).gap(8.0);
    let shown = ask_body(&p.ask.input);
    if !shown.is_empty() {
        body = body.child(col().w_full().px(10.0).py(8.0).radius(6.0).fill(c.layer_02)
            .child(text(shown).font(k.ty.mono).color(c.text_base).wrap_anywhere().selectable(true)));
    }
    if let Some(why) = &p.ask.reason {
        body = body.child(text(why.clone()).font(k.ty.small).color(c.text_muted).wrap());
    }
    let allow_key = s.keys.label(Cmd::Allow);
    let always_key = s.keys.label(Cmd::AlwaysAllow);
    let mut hints = Vec::new();
    if !allow_key.is_empty() { hints.push(format!("{allow_key} allow")); }
    if !always_key.is_empty() && !p.ask.suggestions.is_empty() { hints.push(format!("{always_key} always")); }
    let stop_key = s.keys.label(Cmd::Stop);
    if !stop_key.is_empty() { hints.push(format!("{} stops the turn", stop_key.to_lowercase())); }

    let deny = Button::new("Deny")
        .variant(ButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .on_press(move |s: &mut App| s.answer(thread, item, Decision::Deny))
        .build(k)
        .key("deny");
    let allow = Button::new("Allow")
        .variant(ButtonVariant::Contrast)
        .control_size(ControlSize::Small)
        .on_press(move |s: &mut App| s.answer(thread, item, Decision::Allow))
        .build(k)
        .key("allow");
    let mut buttons = row().gap(6.0).center_y().child(deny);
    // "Always" adds the rules the CLI suggests (to .claude/settings.local.json); without any
    // there's nothing to remember.
    if !p.ask.suggestions.is_empty() {
        buttons = buttons.child(Button::new("Always allow")
            .variant(ButtonVariant::Outline)
            .control_size(ControlSize::Small)
            .on_press(move |s: &mut App| s.answer(thread, item, Decision::AlwaysAllow))
            .build(k)
            .key("always-allow"));
    }
    body.child(row().w_full().gap(8.0).center_y()
        .child(text(hints.join("  ·  ")).font(k.ty.text_xsmall).color(c.text_faint).truncate().shrink().grow(1.0))
        .child(buttons.child(allow)))
}

/// What the approval is for, in full: a command, a path or a URL (twelve lines at most).
fn ask_body(input: &serde_json::Value) -> String {
    for key in ["command", "file_path", "filePath", "notebook_path", "path", "url", "pattern", "query", "plan"] {
        if let Some(v) = input.get(key).and_then(|v| v.as_str()) {
            let mut lines: Vec<&str> = v.trim().lines().take(13).collect();
            if lines.len() > 12 {
                lines.truncate(12);
                lines.push("…");
            }
            return lines.join("\n");
        }
    }
    String::new()
}

fn error(m: &str, k: &Kit) -> El {
    let c = &k.c;
    row().w_full().px(12.0).py(10.0).gap(8.0).radius(9.0).fill(c.danger.bg).border(1.0, c.danger.border)
        .child(icon(&icons::WARNING, 14.0).color(c.danger.fg))
        .child(text(m.to_owned()).font(k.ty.small).color(c.text_strong).wrap().shrink().grow(1.0).selectable(true))
}

fn turn_end(secs: f64, cost: Option<f64>, stopped: bool, t: &Thread, k: &Kit) -> El {
    let c = &k.c;
    let mut parts = vec![if stopped { format!("Stopped after {}", duration(secs)) } else { format!("Worked for {}", duration(secs)) }];
    if let Some(m) = &t.resolved_model { parts.push(m.clone()); }
    if let Some(usd) = cost { parts.push(format!("${usd:.4}")); }
    let rule = || el_rule(k);
    row().w_full().gap(10.0).center_y().py(4.0)
        .child(rule())
        .child(text(parts.join("  ·  ")).font(k.ty.text_xsmall).color(c.text_faint))
        .child(rule())
}

fn el_rule(k: &Kit) -> El {
    rune::el().h(1.0).grow(1.0).fill(k.c.border_weak)
}

fn working(t: &Thread, stop_key: &str, k: &Kit) -> El {
    let c = &k.c;
    let elapsed = t.turn_started.map_or(0.0, |s| s.elapsed().as_secs_f64());
    let status = if !t.asks.is_empty() { "Waiting for approval…" } else if t.steering { "Steering…" } else { "Working…" };
    let mut r = row().key("working").gap(8.0).center_y().h(24.0).px(2.0)
        .child(Spinner::new().size(12.0).build(k))
        .child(text(format!("{status}  {}", duration(elapsed))).font(k.ty.small).color(c.text_muted));
    if !stop_key.is_empty() {
        r = r.child(text(format!("{} to stop", stop_key.to_lowercase())).font(k.ty.text_xsmall).color(c.text_faint).ml(6.0));
    }
    r
}

pub fn duration(secs: f64) -> String {
    if secs < 60.0 { format!("{secs:.0}s") } else { format!("{}m {:02}s", (secs / 60.0) as u64, secs as u64 % 60) }
}
