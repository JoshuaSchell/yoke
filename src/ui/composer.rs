//! The prompt box: queued follow-ups, an auto-growing field over a row of pickers (the model
//! picker's chip, and access), and the send / queue / steer / stop buttons.

use rune::{El, TextStyle, Tx, col, icon, row, text, text_input};
use rune_kit::{ControlSize, IconButton, IconButtonVariant, Kit};
use rune_theme_opencode::icons;

use crate::app::{App, Thread};
use crate::harness;
use crate::shortcuts::{self, Cmd};
use crate::ui::transcript::MAX_W;

/// `floating`: over the transcript (a thread), the card is frosted glass, and a fade behind the
/// hint line keeps it legible over the text scrolling under it.
pub fn view(s: &App, k: &Kit, floating: bool, layer: &mut Vec<El>) -> El {
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
        None => format!("Ask {} anything about {}…", h.name(), s.project().map_or("this project", |p| p.name.as_str())),
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

    // Narrower than this (as last laid out), the model chip drops its effort before the
    // pickers' labels are cut.
    let narrow = rune::memo::layout_with("composer", |r| r[2] < 520.0).unwrap_or(false);
    // Harness, model and effort: one chip and its popover (`ui::picker`).
    let model_pick = crate::ui::picker::view(s, k, narrow, layer);

    // While a turn runs: Stop, and once something is typed, Steer (now) and Queue (after).
    let mut actions = row().gap(6.0).center_y();
    if running {
        if typed {
            actions = actions
                .child(round(IconButton::new(&icons::ARROW_UP_RIGHT)
                    .variant(IconButtonVariant::GhostMuted)
                    .control_size(ControlSize::Large)
                    .label(if s.thread().is_some_and(|t| t.harness == crate::harness::Harness::Claude) { "Steer: send this into the turn now" } else { "Steer: stop the turn and send this now" })
                    .shortcut(&steer_key)
                    .on_press_tx(|s: &mut App, tx: &mut Tx| s.steer(tx))
                    .build(k))
                    .key("steer"))
                .child(round(IconButton::new(&SEND)
                    .variant(IconButtonVariant::Submit)
                    .control_size(ControlSize::Large)
                    .label("Queue: send when the turn ends")
                    .shortcut(&queue_key)
                    .on_press_tx(|s: &mut App, tx: &mut Tx| s.send(tx))
                    .build(k))
                    .key("queue"));
        }
        actions = actions.child(round(IconButton::new(&STOP)
            .variant(IconButtonVariant::Contrast)
            .control_size(ControlSize::Large)
            .label("Stop")
            .shortcut(&stop_key)
            .on_press(|s: &mut App| if let Some(id) = s.selected { s.stop(id) })
            .build(k))
            .key("stop"));
    } else if typed {
        actions = actions.child(round(IconButton::new(&SEND)
            .variant(IconButtonVariant::Submit)
            .control_size(ControlSize::Large)
            .label("Send")
            .shortcut(&send_key)
            .on_press_tx(|s: &mut App, tx: &mut Tx| s.send(tx))
            .build(k))
            .key("send"));
    } else {
        // Nothing to send yet: a quiet outline of the button, rather than the filled one
        // greyed out, which read as a stray square.
        actions = actions.child(row().key("send").role("button").label("Send").disabled(true)
            .size(SEND_SIZE, SEND_SIZE).radius(SEND_SIZE / 2.0).center()
            .fill(c.surface_raised_base).border(1.0, c.border_weak)
            .child(icon(&SEND, 16.0).key("icon").color(c.icon_faint).a11y_hidden()));
    }

    // In a narrow window the pickers give way (their labels cut with "…"); the context gauge
    // and the buttons keep their size, so send never falls outside the card.
    // The paperclip, then harness, model, effort and access: one chip (access is in its popover).
    let clip = IconButton::new(&PAPERCLIP)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Large)
        .label("Attach a file")
        .help("Attach a file (or drop files on the window, or paste an image)")
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.choose_attachment(tx))
        .build(k)
        .key("attach");
    let mut bar = row().w_full().gap(2.0).center_y()
        .child(clip)
        .child(model_pick);
    // Local or a new worktree, for a new thread in a git repository (as in T3 Code).
    if s.thread().is_none() && s.project().is_some_and(|p| p.repo) {
        bar = bar.child(dot(k)).child(worktree_chip(s, k));
    }
    let mut bar = bar.child(rune::el().grow(1.0));
    // How full the thread's context window is.
    if let Some(gauge) = crate::ui::usage::context_gauge(s, k, layer) {
        bar = bar.child(gauge.mr(4.0));
    }
    let bar = bar.child(actions);

    let (border, width) = if s.drop_over { (c.border_focus, 2.0) } else { (c.border_weak, 1.0) };
    let mut card = col().key("composer").w_full().max_w(MAX_W).radius(18.0).border(width, border)
        .shadow(8.0, 24.0, rune::Color::BLACK.alpha(0.10));
    card = if floating { card.backdrop_blur(GLASS_BLUR).fill(c.surface_raised_base.alpha(GLASS)) } else { card.fill(c.surface_raised_base) };
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
        let wt = s.thread().and_then(|t| t.worktree.as_ref());
        let path = match wt {
            Some(w) => format!("{} ({})", w.path.display(), w.branch),
            None if s.thread().is_none() && s.worktree_mode && s.project().is_some_and(|p| p.repo) => "In a new worktree".to_owned(),
            None => s.project().map_or_else(String::new, |p| p.path.display().to_string()),
        };
        format!("{path}  ·  {send_key} send  ·  {newline} newline")
    };
    let hint = text(hint).font(k.ty.text_xsmall).color(c.text_faint).truncate();
    let mut stack = col().w_full().max_w(MAX_W).gap(8.0).items_center();
    // What the thread's CLI still runs in the background after the turn's result.
    if let Some(bg) = s.thread().and_then(|t| crate::ui::agents::background(s, t, k)) {
        stack = stack.child(bg);
    }
    if let Some(t) = s.thread().filter(|t| !t.queue.is_empty()) {
        stack = stack.child(queue(t, running, s.is_live(t.id), &steer_key, k));
    }
    let dock = row().key("composer-dock").w_full().justify_center().px(24.0).pb(14.0).pt(4.0);
    let dock = if floating {
        // Under the card's lower half and the hint: the pane's colour, faded in from clear.
        let bg = c.bg_base;
        dock.child(rune::el().key("dock-solid").pin(Some(0.0), None, Some(0.0), Some(0.0)).h(DOCK_SOLID).fill(bg).a11y_hidden())
            .child(rune::el().key("dock-fade").pin(Some(0.0), None, Some(0.0), Some(DOCK_SOLID)).h(DOCK_FADE).gradient(bg.alpha(0.0), bg).a11y_hidden())
    } else {
        dock
    };
    dock.child(stack.child(card).child(hint))
}

/// The floating card's fill (the rest shows the blurred conversation behind) and its blur.
const GLASS: f32 = 0.72;
const GLASS_BLUR: f32 = 6.0;
/// The band behind the hint line: solid from the pane's foot this high, then fading out above.
const DOCK_SOLID: f32 = 30.0;
const DOCK_FADE: f32 = 28.0;

/// Where a new thread runs: the project's folder, or a worktree of its own on a new branch.
fn worktree_chip(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let on = s.worktree_mode;
    row().key("worktree-chip").h(28.0).px(8.0).gap(6.0).center_y().radius(8.0).shrink()
        .cursor(rune::Cursor::Pointer)
        .hover(c.hover)
        .role("switch").checked(on)
        .label("New worktree")
        .help(if on { "Runs in a new git worktree on its own branch. Click to run in the project's folder." } else { "Runs in the project's folder. Click to run in a new git worktree on its own branch." })
        .child(icon(&icons::BRANCH, 14.0).color(if on { c.text_strong } else { c.icon_weaker }))
        .child(text(if on { "New worktree" } else { "Local" }).font(k.ty.ui_medium).color(c.text_strong).truncate())
        .on_press(|s: &mut App| s.toggle_worktree_mode())
}

/// Files waiting to go with the next message (images as thumbnails), each with a remove button.
fn attachments(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let mut strip = row().key("attachments").w_full().px(14.0).pt(12.0).gap(8.0).wrap();
    for a in &s.attachments {
        let id = a.id;
        let name = a.path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let thumb = match a.thumb.as_ref().filter(|t| t.error().is_none()) {
            Some(t) => rune::el().size(56.0, 56.0).radius(10.0).fill(c.layer_02).pixels_fit(t.pixels(), rune::SurfaceFit::Fill),
            None => {
                let glyph = if crate::app::is_image(&a.path) { &icons::PHOTO } else if a.dir { &icons::FOLDER } else { &icons::FILE_DOCUMENT };
                row().size(56.0, 56.0).radius(10.0).fill(c.layer_02).center().child(icon(glyph, 18.0).color(c.icon_weaker))
            }
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
/// running turn) or be removed; one steered into a live CLI (`live`) shows until the CLI takes
/// it in.
fn queue(t: &Thread, running: bool, live: bool, steer_key: &str, k: &Kit) -> El {
    let c = &k.c;
    let id = t.id;
    let steering = t.queue.iter().filter(|q| q.sent.is_some()).count();
    let head = if t.preparing {
        "Making the worktree · sent when it's ready".to_owned()
    } else if steering == t.queue.len() {
        "Steering · Claude Code takes it in at the turn's next step".to_owned()
    } else if let Some(why) = &t.held {
        format!("{} queued · {why}", t.queue.len())
    } else if running {
        format!("{} queued · sent in order when the turn ends", t.queue.len())
    } else {
        format!("{} queued · paused after the stop", t.queue.len())
    };
    let mut list = col().key("queue").w_full().px(14.0).gap(2.0)
        .child(text(head).font(k.ty.text_xsmall).color(c.text_faint).pl(4.0).pb(2.0));
    for q in &t.queue {
        let qid = q.id;
        let steer = if live { "Steer: send this into the turn now" } else { "Steer: stop the turn and send this now" };
        let now = if q.sent.is_some() {
            text("Steering…").font(k.ty.text_xsmall).color(c.text_muted).px(6.0).key(format!("now-{qid}"))
        } else {
            IconButton::new(&icons::ARROW_UP_RIGHT)
                .variant(IconButtonVariant::GhostMuted)
                .label(if running { steer } else { "Send now" })
                .shortcut(if running { steer_key } else { "" })
                .on_press_tx(move |s: &mut App, tx: &mut Tx| s.send_queued_now(id, qid, tx))
                .build(k)
                .key(format!("now-{qid}"))
        };
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
            // One already at the CLI can't be taken back.
            .child(if q.sent.is_some() { rune::el() } else { remove }));
    }
    list
}

/// A paperclip, on the set's 16-point grid and 1.25-point line.
static PAPERCLIP: rune::Icon = rune::Icon {
    name: "yoke-paperclip",
    view: 16.0,
    body: r#"<path d="M13.25 7.6L8.1 12.75a3.25 3.25 0 0 1-4.6-4.6l5.5-5.5a2.15 2.15 0 0 1 3.05 3.05l-5.45 5.45a1.08 1.08 0 0 1-1.52-1.52L10 4.7" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

/// The send, queue, steer and stop buttons: circles, a little larger than the pickers' row.
const SEND_SIZE: f32 = 30.0;

fn round(b: El) -> El {
    b.size(SEND_SIZE, SEND_SIZE).radius(SEND_SIZE / 2.0)
}

/// Send: a 1.75-point arrow with round ends, heavier than the set's hairline one so it holds
/// its own on the filled circle.
static SEND: rune::Icon = rune::Icon {
    name: "yoke-send",
    view: 16.0,
    body: r#"<path d="M8 12.75V3.5M3.75 7.5L8 3.25L12.25 7.5" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

/// Stop: a rounded square, as the send arrow's weight.
static STOP: rune::Icon = rune::Icon {
    name: "yoke-stop",
    view: 16.0,
    body: r#"<rect x="4.5" y="4.5" width="7" height="7" rx="1.5" fill="currentColor"/>"#,
};

fn dot(k: &Kit) -> El {
    text("·").font(k.ty.small).color(k.c.text_faint).px(2.0)
}
