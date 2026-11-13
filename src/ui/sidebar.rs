//! The thread list, as an inbox (after T3 Code's): every project's threads that need you in
//! one list with no heading, the last to come back to you first and each naming its project;
//! then the ones working, then a Settled shelf you put threads on when you're done with them
//! (⇧⌘S, or ✓ on the row). A settled thread comes back to the inbox as soon as it does
//! anything. Projects are chosen, added and removed in the new-thread screen's headline.

use rune::{El, Icon, Tx, col, el, icon, row, text};
use rune_kit::shell::{Sidebar, SidebarRow};
use rune_kit::{Button, ButtonVariant, ControlSize, Dialog, IconButton, IconButtonVariant, Kit, Spinner};
use rune_theme_opencode::{icons, provider_icons};

use crate::app::{App, Lane, Project, SETTLED_PAGE, Thread};
use crate::harness::Harness;
use crate::shortcuts::Cmd;

/// A thread row: two lines, 44 points.
const ROW_H: f32 = 44.0;
/// The trailing column (time over status) and the hover actions that replace it.
const META_W: f32 = 44.0;
/// A shelf's tallest, open; past that it scrolls.
const SHELF_MAX: f32 = 320.0;
/// How far a thread that needs nothing fades.
const DIM: f32 = 0.55;

/// Claude's spark, as T3 Code draws it (`apps/web/src/components/Icons.tsx`, MIT), in place of
/// the Anthropic wordmark-A the OpenCode set has. Its viewBox is 256 × 257, so the square 257
/// view leaves it a hair narrower than tall, as there.
pub static CLAUDE: Icon = Icon { name: "claude", view: 257.0, body: r##"<path d="m50.228 170.321 50.357-28.257.843-2.463-.843-1.361h-2.462l-8.426-.518-28.775-.778-24.952-1.037-24.175-1.296-6.092-1.297L0 125.796l.583-3.759 5.12-3.434 7.324.648 16.202 1.101 24.304 1.685 17.629 1.037 26.118 2.722h4.148l.583-1.685-1.426-1.037-1.101-1.037-25.147-17.045-27.22-18.017-14.258-10.37-7.713-5.25-3.888-4.925-1.685-10.758 7-7.713 9.397.649 2.398.648 9.527 7.323 20.35 15.75L94.817 91.9l3.889 3.24 1.555-1.102.195-.777-1.75-2.917-14.453-26.118-15.425-26.572-6.87-11.018-1.814-6.61c-.648-2.723-1.102-4.991-1.102-7.778l7.972-10.823L71.42 0 82.05 1.426l4.472 3.888 6.61 15.101 10.694 23.786 16.591 32.34 4.861 9.592 2.592 8.879.973 2.722h1.685v-1.556l1.36-18.211 2.528-22.36 2.463-28.776.843-8.1 4.018-9.722 7.971-5.25 6.222 2.981 5.12 7.324-.713 4.73-3.046 19.768-5.962 30.98-3.889 20.739h2.268l2.593-2.593 10.499-13.934 17.628-22.036 7.778-8.749 9.073-9.657 5.833-4.601h11.018l8.1 12.055-3.628 12.443-11.342 14.388-9.398 12.184-13.48 18.147-8.426 14.518.778 1.166 2.01-.194 30.46-6.481 16.462-2.982 19.637-3.37 8.88 4.148.971 4.213-3.5 8.62-20.998 5.184-24.628 4.926-36.682 8.685-.454.324.519.648 16.526 1.555 7.065.389h17.304l32.21 2.398 8.426 5.574 5.055 6.805-.843 5.184-12.962 6.611-17.498-4.148-40.83-9.721-14-3.5h-1.944v1.167l11.666 11.406 21.387 19.314 26.767 24.887 1.36 6.157-3.434 4.86-3.63-.518-23.526-17.693-9.073-7.972-20.545-17.304h-1.36v1.814l4.73 6.935 25.017 37.59 1.296 11.536-1.814 3.76-6.481 2.268-7.13-1.297-14.647-20.544-15.1-23.138-12.185-20.739-1.49.843-7.194 77.448-3.37 3.953-7.778 2.981-6.48-4.925-3.436-7.972 3.435-15.749 4.148-20.544 3.37-16.333 3.046-20.285 1.815-6.74-.13-.454-1.49.194-15.295 20.999-23.267 31.433-18.406 19.702-4.407 1.75-7.648-3.954.713-7.064 4.277-6.286 25.47-32.405 15.36-20.092 9.917-11.6-.065-1.686h-.583L44.07 198.125l-12.055 1.555-5.185-4.86.648-7.972 2.463-2.593 20.35-13.999-.064.065Z" fill="currentColor"/>"## };

/// Claude's terracotta (`#d97757`), which T3 Code fills the spark with in both appearances.
pub fn claude_orange() -> rune::Color {
    rune::color::hex("#d97757").unwrap_or(rune::Color::WHITE)
}

pub fn provider_icon(h: Harness) -> &'static Icon {
    match h {
        Harness::Claude => &CLAUDE,
        _ => provider_icons::named(h.provider()).unwrap_or(provider_icons::FALLBACK),
    }
}

/// The harness's mark on a small tile. Claude's spark keeps its own colour, as in T3 Code;
/// the others take the theme's icon colour.
pub fn avatar(h: Harness, k: &Kit) -> El {
    let color = if h == Harness::Claude { claude_orange() } else { k.c.icon_base };
    row().size(20.0, 20.0).radius(6.0).center().fill(k.c.layer_02).child(icon(provider_icon(h), 13.0).color(color))
}

/// A project's mark on a `size`-point tile, as T3 Code draws it: its image when it has one
/// (and it has decoded), else its letters (chosen, or its initials) in its tint, on a wash of
/// that tint.
pub fn project_avatar(p: &Project, size: f32, k: &Kit) -> El {
    let tile = row().key("project-avatar").size(size, size).radius(size * 0.3).center().a11y_hidden();
    if let Some(img) = p.icon.as_ref().filter(|i| i.is_ready() && i.error().is_none()) {
        return tile.pixels_fit(img.pixels(), rune::SurfaceFit::Fit);
    }
    let tint = crate::project_icon::tint(p.tint, k.dark);
    let font = rune::TextStyle { size: (size * 0.48).round(), weight: 700.0, line_height: size, ..k.ty.mono };
    let letters = match &p.choice {
        crate::project_icon::Choice::Letters { text, .. } if !text.is_empty() => text.clone(),
        _ => crate::project_icon::monogram(&p.name),
    };
    tile.fill(tint.alpha(0.16)).child(text(letters).font(font).color(tint))
}

/// A thread's mark: its project's, else (a thread in no project) its agent's.
fn thread_avatar(s: &App, t: &Thread, k: &Kit) -> El {
    match s.project_of(t) {
        Some(p) => project_avatar(p, 20.0, k),
        None => avatar(t.harness, k),
    }
}

fn ago(t: &Thread) -> String {
    let s = t.updated.elapsed().map_or(0, |d| d.as_secs());
    match s {
        0..60 => "now".into(),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

/// The second line: why the thread is where it is, then which agent and model. With more
/// than one project, it starts with the thread's.
fn subtitle(s: &App, t: &Thread, k: &Kit) -> (String, Option<rune::Color>) {
    let model = t.resolved_model.as_deref().unwrap_or(if t.model == crate::harness::models::DEFAULT { "" } else { t.model.as_str() });
    // A worktree thread names its branch instead of the model.
    let who = match (&t.worktree, model.is_empty()) {
        (Some(w), _) => format!("{} · {}", t.harness.name(), w.branch),
        (None, true) => t.harness.name().to_owned(),
        (None, false) => format!("{} · {model}", t.harness.name()),
    };
    let project = match s.project_of(t) {
        Some(p) if s.projects.len() > 1 => format!("{} · ", p.name),
        _ => String::new(),
    };
    let queued = if t.queue.is_empty() { String::new() } else { format!(" · {} queued", t.queue.len()) };
    if !t.asks.is_empty() {
        return (format!("Wants to use {}", t.asks[0].ask.tool), Some(k.c.warning.icon));
    }
    if t.preparing {
        return (format!("{project}Making a worktree…"), None);
    }
    if t.busy {
        let secs = t.turn_started.map_or(0, |s| s.elapsed().as_secs());
        return (format!("{project}Working {}{queued}", crate::ui::transcript::duration(secs as f64)), None);
    }
    // The turn ended, but what it started runs on: "Waiting on 2 agents".
    if t.has_background() {
        return (format!("{project}Waiting on {}{queued}", t.background_counts()), None);
    }
    (format!("{project}{who}{queued}"), None)
}

/// The status labels' icons, one outline family at 24 × 24 and 2-point strokes (the shapes
/// T3 Code's labels use: circle-check, circle-x, shield-question), so the three match.
static DONE_ICON: Icon = Icon { name: "status-done", view: 24.0, body: r##"<circle cx="12" cy="12" r="10" stroke="currentColor" fill="none" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/><path d="m8.5 12 2.5 2.5 4.5-5" stroke="currentColor" fill="none" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>"## };
static FAILED_ICON: Icon = Icon { name: "status-failed", view: 24.0, body: r##"<circle cx="12" cy="12" r="10" stroke="currentColor" fill="none" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/><path d="m15 9-6 6M9 9l6 6" stroke="currentColor" fill="none" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>"## };
static APPROVAL_ICON: Icon = Icon { name: "status-approval", view: 24.0, body: r##"<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z" stroke="currentColor" fill="none" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/><path d="M9.6 9.2a2.5 2.5 0 0 1 4.86.83c0 1.67-2.46 2.5-2.46 2.5M12 16.5h.01" stroke="currentColor" fill="none" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>"## };

/// Why a thread wants you, as T3 Code labels it in the card's corner: an approval waiting,
/// then a turn that failed or finished since you last looked.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Status {
    Approval,
    Failed,
    Done,
}

pub fn status(t: &Thread) -> Option<Status> {
    if !t.asks.is_empty() {
        Some(Status::Approval)
    } else if t.unread && t.failed {
        Some(Status::Failed)
    } else if t.unread && !t.has_background() {
        // Not done while its subagents or commands run on (it's on the Working shelf).
        Some(Status::Done)
    } else {
        None
    }
}

/// The status as an icon and a word, in its colour.
fn status_label(st: Status, k: &Kit) -> El {
    let c = &k.c;
    let (glyph, word, color) = match st {
        Status::Approval => (&APPROVAL_ICON, "Approval", c.warning.icon),
        Status::Failed => (&FAILED_ICON, "Failed", c.danger.icon),
        Status::Done => (&DONE_ICON, "Done", c.success.icon),
    };
    row().key("status").gap(3.0).center_y()
        .child(icon(glyph, 12.0).color(color))
        .child(text(word).font(k.ty.small_medium).color(color))
}

fn thread_row(s: &App, t: &Thread, lane: Lane, k: &Kit) -> El {
    let c = &k.c;
    let id = t.id;
    let (sub, tint) = subtitle(s, t, k);
    // On the title's line, flush right: why the thread wants you (as T3 Code shows it), else
    // how long ago it last did something. A spinner on the subtitle's line while it works.
    // Under the pointer they give way to the row's actions.
    let st = status(t);
    let corner = match st {
        Some(st) => status_label(st, k),
        None => text(ago(t)).key("ago").font(k.ty.small).color(c.text_faint),
    };
    let mark = t.at_work().then(|| Spinner::new().size(10.0).color(c.icon_muted).build(k));
    let meta = col()
        .key("meta")
        .min_w(META_W)
        .h(ROW_H - 12.0)
        .items_end()
        .justify_between()
        .group_opacity(1.0, 0.0)
        .a11y_hidden()
        .child(corner)
        .child(row().key("mark").h(14.0).center_y().children(mark));
    let settle_key = s.keys.label(Cmd::SettleThread);
    let mut actions = row().key("actions").pin(None, Some(0.0), Some(0.0), Some(0.0)).gap(2.0).center_y().justify_end().group_opacity(0.0, 1.0);
    match lane {
        Lane::Inbox if t.asks.is_empty() => {
            actions = actions.child(
                action(&icons::CHECK, "Settle", &settle_key, k).on_press_tx(move |s: &mut App, _: &mut Tx| { s.settle(id); }).key("settle"),
            );
        }
        Lane::Settled => {
            actions = actions.child(
                action(&icons::OUTLINE_UNDO, "Move to inbox", "", k).on_press_tx(move |s: &mut App, _: &mut Tx| s.unsettle(id)).key("unsettle"),
            );
        }
        _ => {}
    }
    let delete_key = s.keys.label(Cmd::DeleteThread);
    actions = actions.child(action(&icons::CLOSE_SMALL, "Delete thread", &delete_key, k).on_press_tx(move |s: &mut App, _: &mut Tx| s.ask_delete(id)).key("delete"));
    let trailing = el().key("trailing").min_w(META_W).h(ROW_H - 12.0).child(meta).child(actions);

    let mut r = SidebarRow::new(t.title.clone())
        .height(ROW_H)
        .avatar(thread_avatar(s, t, k))
        .subtitle(sub)
        .trailing(trailing)
        .selected(s.selected == Some(id))
        .on_select(move |s: &mut App, _: &mut Tx| s.select(id));
    if let Some(color) = tint {
        r = r.subtitle_color(color);
    }
    // Settled threads step back a little.
    if lane == Lane::Settled && s.selected != Some(id) {
        r = r.title_style(rune::TextStyle { weight: 400.0, ..k.ty.ui_compact });
    }
    let built = r.build(k).key(format!("thread-{id}"));
    // In the list, a thread that wants nothing of you fades back, so the ones that do stand
    // out (as in T3 Code).
    if lane == Lane::Inbox && st.is_none() && !t.busy && s.selected != Some(id) { built.opacity(DIM) } else { built }
}

/// Asks before deleting a thread (the row's ✕, or ⇧⌘⌫), naming what goes with it. Return
/// deletes; Escape, Cancel or a click outside keeps it.
pub fn delete_dialog(s: &App, k: &Kit, layer: &mut Vec<El>) {
    let Some(id) = s.confirm_delete else { return };
    let Some(t) = s.threads.iter().find(|t| t.id == id) else { return };
    let what = if s.saved_session(id).is_some() {
        format!("\u{201c}{}\u{201d} and its Claude Code transcript are deleted from this computer. This can't be undone.", t.title)
    } else {
        format!("\u{201c}{}\u{201d} is removed from Yoke. This can't be undone.", t.title)
    };
    // Deleting ends the CLI, and what it still runs in the background with it.
    let what = if s.background_of(id) {
        let named: Vec<String> = t.background.iter().take(4).map(|b| crate::ui::agents::task_name(&b.description)).collect();
        format!("{what} Its background work stops too: {} ({}).", t.background_counts(), named.join(", "))
    } else {
        what
    };
    let buttons = row().key("delete-buttons").w_full().padding(16.0).gap(8.0).justify_end()
        .child(Button::new("Cancel").variant(ButtonVariant::Ghost).on_press(|s: &mut App| s.confirm_delete = None).build(k).key("delete-cancel"))
        .child(Button::new("Delete").variant(ButtonVariant::Danger).on_press(move |s: &mut App| s.delete(id)).build(k).default_button().key("delete-confirm"));
    Dialog::new("delete", "Delete thread?").description(what).width(420.0).fit().open(true)
        .on_close(|s: &mut App| s.confirm_delete = None)
        .child(buttons)
        .build(k, layer);
}

/// A small hover button on a row. Pointer only: in the list the arrows step through rows.
fn action(glyph: &'static Icon, label: &str, shortcut: &str, k: &Kit) -> El {
    let mut b = IconButton::new(glyph).control_size(ControlSize::Small).variant(IconButtonVariant::GhostMuted).label(label).help(label);
    if !shortcut.is_empty() {
        b = b.shortcut(shortcut);
    }
    b.build(k).focusable(false)
}

pub fn view(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let new = SidebarRow::new("New thread")
        .icon(&icons::NEW_SESSION)
        .detail(s.keys.label(Cmd::NewThread))
        .height(32.0)
        .selected(s.selected.is_none() && !s.settings)
        .on_select(|s: &mut App, _: &mut Tx| s.new_thread())
        .build(k)
        .key("new-thread");

    let inbox = s.lane_threads(Lane::Inbox);
    let working = s.lane_threads(Lane::Working);
    let settled_count = s.threads.iter().filter(|t| App::lane(t) == Lane::Settled).count();

    let mut bar = Sidebar::new().label("Threads").inset(0.0).header_gap(4.0).header_padding(0.0, 12.0).row_gap(2.0).header(new);
    // Every project's threads that want you, in one list, as T3 Code's Active list: the
    // project is on each row, and chosen for a new thread in the new-thread screen's headline.
    for t in &inbox {
        bar = bar.child(thread_row(s, t, Lane::Inbox, k));
    }
    if inbox.is_empty() {
        let note = if s.threads.is_empty() { "No threads yet" } else { "All caught up" };
        bar = bar.child(row().key("inbox-empty").w_full().h(28.0).px(8.0).center_y()
            .child(text(note).font(k.ty.small).color(c.text_faint)));
    }

    let list = bar.build(k).grow(1.0).shrink().min_h(0.0);
    let mut side = col().key("sidebar-inbox").w_full().h_full().min_h(0.0).child(list);
    // The shelves sit at the sidebar's foot, shut until opened: Working, then Settled.
    if !working.is_empty() {
        side = side.child(working_shelf(s, working.len(), k));
    }
    if settled_count > 0 {
        side = side.child(settled_shelf(s, settled_count, k));
    }
    side
}

/// The Working shelf: threads with a turn running and nothing asked of you, as in T3 Code.
fn working_shelf(s: &App, count: usize, k: &Kit) -> El {
    let header = rune_kit::shell::SidebarHeader::new("Working")
        .count(count)
        .collapsed(!s.working_open)
        .on_toggle(|s: &mut App, _: &mut Tx| s.toggle_working_shelf())
        .build(k)
        .key("working-header");
    let rows = col().key("working-rows").w_full().gap(2.0).children(s.working_listed().iter().map(|t| thread_row(s, t, Lane::Working, k)));
    shelf("working", "Working", header, rows, k)
}

/// A shelf at the sidebar's foot: its header, and a list of its own that scrolls past
/// `SHELF_MAX`.
fn shelf(key: &str, label: &str, header: El, rows: El, k: &Kit) -> El {
    let list = col()
        .key(format!("{key}-scroll"))
        .w_full()
        .max_h(SHELF_MAX)
        .shrink()
        .min_h(0.0)
        .scroll(rune::ScrollSpec { stick_bottom: false })
        .scroll_edge_fade(12.0)
        .child(rows);
    col()
        .key(key.to_owned())
        .role("group")
        .label(label.to_owned())
        .w_full()
        .shrink()
        .min_h(0.0)
        .pt(4.0)
        .pb(8.0)
        .gap(2.0)
        .border_sides([1.0, 0.0, 0.0, 0.0], k.c.border_muted)
        .motion(k.motion.disclosure)
        .child(header)
        .child(list)
}

/// The Settled shelf, anchored to the sidebar's foot: its header, and while it's open a list
/// of its own that scrolls, up to `SHELF_MAX` tall.
fn settled_shelf(s: &App, count: usize, k: &Kit) -> El {
    let listed = s.settled_listed();
    let header = rune_kit::shell::SidebarHeader::new("Settled")
        .count(count)
        .collapsed(!s.settled_open)
        .on_toggle(|s: &mut App, _: &mut Tx| s.toggle_settled_shelf())
        .build(k)
        .key("settled-header");
    let mut rows = col().key("settled-rows").w_full().gap(2.0).children(listed.iter().map(|t| thread_row(s, t, Lane::Settled, k)));
    let more = count.saturating_sub(s.settled_shown);
    if s.settled_open && more > 0 {
        let n = more.min(SETTLED_PAGE);
        rows = rows.child(
            SidebarRow::new(format!("Show {n} more"))
                .height(28.0)
                .on_select(move |s: &mut App, _: &mut Tx| s.settled_shown += n)
                .build(k)
                .key("settled-more"),
        );
    }
    shelf("settled", "Settled", header, rows, k)
}
