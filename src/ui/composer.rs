//! The prompt box: queued follow-ups, an auto-growing field over a row of pickers (the model
//! picker's chip, and access), and the send / queue / steer / stop buttons.

use rune::{El, TextStyle, Tx, col, icon, row, text, text_input};
use rune_kit::{ControlSize, IconButton, IconButtonVariant, Kit, Select};
use rune_theme_opencode::icons;

use crate::app::{App, Thread};
use crate::harness::{self, ACCESS, Access};
use crate::shortcuts::{self, Cmd};
use crate::ui::transcript::MAX_W;

pub fn view(s: &App, k: &Kit, layer: &mut Vec<El>) -> El {
    let c = &k.c;
    let running = s.thread().is_some_and(|t| t.busy);
    let typed = !s.draft.text.trim().is_empty() || !s.attachments.is_empty();
    let h = s.active_harness();
    let (send_key, other_key) = s.send_labels();
    let steers = s.defaults.follow_up == crate::settings::FollowUp::Steer;
    let (steer_key, queue_key) = if steers { (send_key.clone(), other_key) } else { (other_key, send_key.clone()) };
    let stop_key = s.keys.label(Cmd::Stop);
    let style = TextStyle { line_height: 22.0, ..k.ty.body };
    let placeholder = match s.thread() {
        Some(_) if running && steers => format!("Steer {}…", h.name()),
        Some(_) if running => format!("Queue a follow-up for {}…", h.name()),
        Some(_) => format!("Reply to {}…", h.name()),
        None => format!("Ask {} anything about {}…", h.name(), s.project.name),
    };
    let field = text_input(&s.draft, style)
        .key("prompt")
        .w_full()
        .color(c.text_strong)
        .placeholder(placeholder, c.text_faint)
        .caret(c.text_strong, k.selection())
        .label("Prompt")
        .on_edit(|s: &mut App| &mut s.draft)
        .on_submit(|s: &mut App, tx: &mut Tx| s.submit(tx))
        // A pasted image (a screenshot) attaches; text pastes as usual.
        .on_input_paste(|s: &mut App, content: &rune::ClipboardContent, _: &mut Tx| {
            let Some(image) = content.image() else { return false };
            s.attach_png(image.png());
            true
        })
        // The field sees keys before the window does (it would take Escape and ⌘↩ itself), so
        // the shortcut table runs here first.
        // With ⌘↩ sending (Settings › Chat), ↩ and ⇧⌘↩ are the prompt's own.
        .on_input_key(|s: &mut App, ev: &rune::KeyEvent, tx: &mut Tx| s.prompt_key(ev, tx) || shortcuts::dispatch(s, ev, tx));
    let field = rune::auto_grow(field, 2, 12);

    // Harness, model and effort: one chip and its popover (`ui::picker`).
    let model_pick = crate::ui::picker::view(s, k, layer);
    // "Ask first" only for harnesses that can ask; the others would treat it as read only.
    let access: Vec<&str> = ACCESS.iter().filter(|a| **a != Access::Ask || h.asks()).map(|a| a.label()).collect();
    let access_pick = Select::new("access", &access)
        .current(Some(s.access.for_harness(h).label()))
        .label("Access")
        .open(s.open_menu == Some("access"))
        .on_toggle(|s: &mut App| s.toggle_menu("access"))
        .on_close(|s: &mut App| s.open_menu = None)
        .on_choose(|s: &mut App, v: &str, _: &mut Tx| {
            if let Some(a) = Access::from_label(v) { s.set_access(a); }
            s.open_menu = None;
        })
        .build(k, layer);

    // While a turn runs: Stop, and once something is typed, Steer (now) and Queue (after).
    let mut actions = row().gap(6.0).center_y();
    if running {
        if typed {
            actions = actions
                .child(IconButton::new(&icons::ARROW_UP_RIGHT)
                    .variant(IconButtonVariant::GhostMuted)
                    .control_size(ControlSize::Large)
                    .label("Steer: stop the turn and send this now")
                    .shortcut(&steer_key)
                    .on_press_tx(|s: &mut App, tx: &mut Tx| s.steer(tx))
                    .build(k)
                    .key("steer"))
                .child(IconButton::new(&icons::ARROW_UP)
                    .variant(IconButtonVariant::Submit)
                    .control_size(ControlSize::Large)
                    .label("Queue: send when the turn ends")
                    .shortcut(&queue_key)
                    .on_press_tx(|s: &mut App, tx: &mut Tx| s.send(tx))
                    .build(k)
                    .key("queue"));
        }
        actions = actions.child(IconButton::new(&icons::STOP)
            .variant(IconButtonVariant::Contrast)
            .control_size(ControlSize::Large)
            .label("Stop")
            .shortcut(&stop_key)
            .on_press(|s: &mut App| if let Some(id) = s.selected { s.stop(id) })
            .build(k)
            .key("stop"));
    } else {
        actions = actions.child(IconButton::new(&icons::ARROW_UP)
            .variant(IconButtonVariant::Submit)
            .control_size(ControlSize::Large)
            .label("Send")
            .shortcut(&send_key)
            .disabled(!typed)
            .on_press_tx(|s: &mut App, tx: &mut Tx| s.send(tx))
            .build(k)
            .key("send"));
    }

    let mut bar = row().w_full().gap(2.0).center_y()
        .child(model_pick)
        .child(dot(k))
        .child(access_pick)
        .child(rune::el().grow(1.0));
    // How close the Claude plan is to its limits, opening the windows' meters.
    if let Some(chip) = crate::ui::usage::chip(s, k, layer) {
        bar = bar.child(chip.mr(4.0));
    }
    let bar = bar.child(actions);

    let (border, width) = if s.drop_over { (c.border_focus, 2.0) } else { (c.border_weak, 1.0) };
    let mut card = col().key("composer").w_full().max_w(MAX_W).radius(18.0).fill(c.surface_raised_base).border(width, border)
        .shadow(8.0, 24.0, rune::Color::BLACK.alpha(0.10));
    if s.drop_over {
        card = card.child(row().w_full().px(16.0).pt(12.0).gap(8.0).center_y()
            .child(icon(&icons::PHOTO, 14.0).color(c.icon_base))
            .child(text("Drop to attach").font(k.ty.ui_medium).color(c.text_strong)));
    }
    if !s.attachments.is_empty() {
        card = card.child(attachments(s, k));
    }
    if let Some(e) = &s.composer_error {
        card = card.child(text(e.clone()).font(k.ty.text_xsmall).color(c.danger.fg).px(16.0).pt(10.0));
    }
    let card = card
        .child(col().w_full().px(16.0).pt(14.0).pb(6.0).child(field))
        .child(col().w_full().px(10.0).pb(10.0).child(bar));
    let newline = if s.defaults.send == crate::settings::SendKey::ModReturn { "↩" } else { "⇧↩" };
    let hint = if running {
        // The send key first, then the other of queue and steer.
        let (first, second) = if steers { (("steer", &steer_key), ("queue", &queue_key)) } else { (("queue", &queue_key), ("steer", &steer_key)) };
        let other = if second.1.is_empty() { String::new() } else { format!("  ·  {} {}", second.1, second.0) };
        let stop = if stop_key.is_empty() { String::new() } else { format!("  ·  {stop_key} stop") };
        format!("{} {}{other}{stop}", first.1, first.0)
    } else {
        format!("{}  ·  {send_key} send  ·  {newline} newline", s.project.path.display())
    };
    let hint = text(hint).font(k.ty.text_xsmall).color(c.text_faint).truncate();
    let mut stack = col().w_full().max_w(MAX_W).gap(8.0).items_center();
    if let Some(t) = s.thread().filter(|t| !t.queue.is_empty()) {
        stack = stack.child(queue(t, running, &steer_key, k));
    }
    row().w_full().justify_center().px(24.0).pb(14.0).pt(4.0)
        .child(stack.child(card).child(hint))
}

/// Images waiting to go with the next message, each with a remove button.
fn attachments(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let mut strip = row().key("attachments").w_full().px(14.0).pt(12.0).gap(8.0).wrap();
    for a in &s.attachments {
        let id = a.id;
        let name = a.path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let thumb = match a.thumb.as_ref().filter(|t| t.error().is_none()) {
            Some(t) => rune::el().size(56.0, 56.0).radius(10.0).fill(c.layer_02).pixels_fit(t.pixels(), rune::SurfaceFit::Fill),
            None => row().size(56.0, 56.0).radius(10.0).fill(c.layer_02).center().child(icon(&icons::PHOTO, 18.0).color(c.icon_weaker)),
        };
        let remove = IconButton::new(&icons::CLOSE_SMALL)
            .variant(IconButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .label(format!("Remove {name}"))
            .on_press(move |s: &mut App| s.detach(id))
            .build(k)
            .key(format!("detach-{id}"));
        strip = strip.child(row().key(format!("att-{id}")).h(64.0).pl(4.0).pr(4.0).gap(8.0).center_y().radius(12.0)
            .fill(c.surface_base).border(1.0, c.border_weak)
            .child(thumb)
            .child(text(harness::first_line(&name, 28)).font(k.ty.small).color(c.text_base).max_w(140.0).truncate())
            .child(remove));
    }
    strip
}

/// Queued follow-ups, oldest first, tucked above the prompt. Each can go now (steering a
/// running turn) or be removed.
fn queue(t: &Thread, running: bool, steer_key: &str, k: &Kit) -> El {
    let c = &k.c;
    let id = t.id;
    let head = if running {
        format!("{} queued · sent in order when the turn ends", t.queue.len())
    } else {
        format!("{} queued · paused after the stop", t.queue.len())
    };
    let mut list = col().key("queue").w_full().px(14.0).gap(2.0)
        .child(text(head).font(k.ty.text_xsmall).color(c.text_faint).pl(4.0).pb(2.0));
    for q in &t.queue {
        let qid = q.id;
        let now = IconButton::new(&icons::ARROW_UP_RIGHT)
            .variant(IconButtonVariant::GhostMuted)
            .label(if running { "Steer: stop the turn and send this now" } else { "Send now" })
            .shortcut(if running { steer_key } else { "" })
            .on_press_tx(move |s: &mut App, tx: &mut Tx| s.send_queued_now(id, qid, tx))
            .build(k)
            .key(format!("now-{qid}"));
        let remove = IconButton::new(&icons::CLOSE_SMALL)
            .variant(IconButtonVariant::GhostMuted)
            .label("Remove from queue")
            .on_press(move |s: &mut App| s.unqueue(id, qid))
            .build(k)
            .key(format!("remove-{qid}"));
        list = list.child(row().key(format!("q{qid}")).w_full().h(30.0).pl(10.0).pr(4.0).gap(8.0).center_y().radius(10.0)
            .fill(c.surface_raised_base).border(1.0, c.border_weak)
            .child(icon(&icons::ENTER, 13.0).color(c.icon_weaker))
            .child(text(harness::first_line(&q.text, 200)).font(k.ty.small).color(c.text_base).truncate().shrink().grow(1.0))
            .child(now)
            .child(remove));
    }
    list
}

fn dot(k: &Kit) -> El {
    text("·").font(k.ty.small).color(k.c.text_faint).px(2.0)
}
