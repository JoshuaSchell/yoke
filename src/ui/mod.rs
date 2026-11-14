//! The window: sidebar of threads, and a main pane with the transcript over the composer.

pub mod agent_view;
pub mod agents;
pub mod composer;
pub mod diff;
pub mod hud;
pub mod icon_editor;
pub mod picker;
#[cfg(target_os = "macos")]
pub mod mac;
pub mod settings;
pub mod sidebar;
pub mod theme;
pub mod transcript;
pub mod usage;

use std::time::Duration;

use rune::{Cx, El, Icon, KeyEvent, Spring, Tx, col, el, row, text};
use rune_kit::shell::{PaneMotion, SidebarPeek, SidebarToggle, SplitView, Toolbar};
use rune_kit::{ControlSize, IconButton, IconButtonVariant, Kit, TooltipAppearance, TooltipStyle};
use rune_theme_opencode::icons;

use crate::app::App;
use crate::shortcuts::Cmd;

/// Seconds the pointer rests on something before its tooltip shows.
const HELP_DELAY: f64 = 0.25;

pub fn view(s: &App, cx: &mut Cx<App>) -> El {
    let dark = s.appearance.dark(cx.system_dark());
    let k: &Kit = theme::kit(s.theme, dark);
    // Tooltips in the theme's look and on its own short wait, rather than AppKit's: its second
    // or so felt slow on the gauges, and its timer starts over whenever a region is registered
    // again, as the context gauge's is with every reply streaming in.
    cx.help_look(TooltipStyle::from_kit(k, TooltipAppearance::Compact).max_width(320.0).help_look().delay(HELP_DELAY));
    let mut layer = Vec::new();

    let mut main = col().key("main").w_full().h_full().min_h(0.0);
    main = if s.settings {
        main.child(settings::view(s, k, &mut layer))
    } else if s.usage_open {
        main.child(usage::page(s, k))
    } else {
        match s.thread() {
            // The transcript runs the pane's full height and the composer floats over its foot,
            // as in T3 Code: scrolled up, the conversation shows through the frosted card. The
            // transcript's last row clears the composer (as last laid out) at the bottom.
            Some(_) => {
                let dock = rune::memo::layout_with("composer-dock", |r| r[3]).unwrap_or(150.0);
                let mut conversation = transcript::view(s, k, (s.theme, dark), dock);
                let mut composer = composer::view(s, k, true, &mut layer).pin(Some(0.0), None, Some(0.0), Some(0.0));
                // An agent opened over the thread covers it; the thread stays laid out underneath,
                // out of sight and out of reach, so going back finds it scrolled where it was.
                let agent = agent_view::view(s, k, (s.theme, dark));
                if agent.is_some() {
                    conversation = conversation.inert().opacity(0.0);
                    composer = composer.inert().opacity(0.0);
                }
                let mut pane = col().key("thread-pane").w_full().grow(1.0).min_h(0.0).child(conversation).child(composer);
                if let Some(a) = agent { pane = pane.child(a.pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))); }
                main.child(pane)
            }
            None if s.projects.is_empty() => main.child(no_projects(s, k)),
            None => main.child(hero(s, k, &mut layer)).child(composer::view(s, k, false, &mut layer)),
        }
    };
    if s.hud {
        let items = s.threads.iter().map(|t| t.items.len()).sum();
        main = main.child(el().absolute(0.0, 12.0).w_full().child(row().w_full().justify_end().pr(12.0).child(hud::view(k, s.threads.len(), items))));
    }

    let title = if s.settings { "Settings".to_owned() } else if s.usage_open { "Usage".to_owned() } else { s.thread().map_or("New thread".to_owned(), |t| t.title.clone()) };
    // The toolbar band is 48 points tall inside the card, which starts `FRAME` down, so its
    // middle is 32 down. The traffic lights sit 24 down, where macOS apps put them; the
    // toolbar's contents lift `FRAME` to line up with them.
    let crumbs = breadcrumb(s.project(), &title, k).offset(0.0, -FRAME);
    let (hud_key, settings_key, sidebar_key) = (s.keys.label(Cmd::FrameStats), s.keys.label(Cmd::Settings), s.keys.label_static(Cmd::ToggleSidebar));
    let hud_button = IconButton::new(&icons::WINDOW_ANALYTICS)
        .variant(IconButtonVariant::GhostMuted)
        .expanded(s.hud)
        .label(if s.hud { "Hide frame stats" } else { "Show frame stats" })
        .shortcut(&hud_key)
        .on_press(|s: &mut App| s.hud = !s.hud)
        .build(k)
        .key("hud-toggle");
    let settings_button = IconButton::new(&COG)
        .variant(IconButtonVariant::GhostMuted)
        .expanded(s.settings)
        .label(if s.settings { "Close Settings" } else { "Settings" })
        .shortcut(&settings_key)
        .on_press(|s: &mut App| if s.settings { s.close_settings() } else { s.open_settings() })
        .build(k)
        .key("settings-open");
    let mut trailing = row().gap(2.0).center_y().offset(0.0, -FRAME);
    if !s.settings && !s.usage_open && let Some(open) = open_in(s, k, &mut layer) {
        trailing = trailing.child(open.mr(6.0));
    }
    if let Some(button) = update_button(s, k) {
        trailing = trailing.child(button);
    }
    let toolbar = Toolbar::new().leading(crumbs).trailing(trailing.child(diff::toolbar_button(s, k)).child(usage::toolbar_button(s, k, &mut layer)).child(hud_button).child(settings_button));
    let attention = Attention::of(s, k);
    // The diff panel is built only while it's open.
    let inspector = if s.diff.pane.is_open() { diff::view(s, k, &mut layer) } else { el() };
    let window = SplitView::new(main)
        .sidebar(&s.sidebar, |s: &mut App| &mut s.sidebar, sidebar::view(s, k))
        .inspector(&s.diff.pane, |s: &mut App| &mut s.diff.pane, inspector)
        .toolbar(toolbar)
        .titlebar_inset(cx.titlebar_inset())
        .frame_inset(FRAME)
        .toolbar_in_panel()
        .sidebar_padding(18.0, TOGGLE_END)
        // SplitView centres its own toggle on the band, 32 down; `sidebar_toggle` below draws
        // one 24 down instead. These widths keep the toolbar's title clear of it.
        .hide_sidebar_toggle()
        .sidebar_toggle(SidebarToggle::new().past_lights(TOGGLE_PAST + TOGGLE + 6.0).edge(TOGGLE_EDGE + TOGGLE + 6.0))
        .pane_motion(PaneMotion::new().spring(Spring::new(0.3, 0.0)).scale_with_width(480.0, 0.15, 0.3).fade_tail(0.15))
        .sidebar_peek(SidebarPeek::new().edge(16.0, ms(40)).toggle_delay(ms(80)).below(56.0).hold(15.0, ms(100)).spring(Spring::new(0.18, 0.0)))
        .reduced_motion(cx.is_reduced_motion())
        .build(k)
        .child(sidebar_toggle(s, cx.titlebar_inset(), attention, sidebar_key, cx.is_reduced_motion(), k));
    // Files dropped anywhere in the window go to the composer.
    sidebar::delete_dialog(s, k, &mut layer);
    trust_dialog(s, k, &mut layer);
    stop_dialog(s, k, &mut layer);
    icon_editor::dialog(s, k, &mut layer);
    rune_kit::root(window, layer)
        .on_drop_files(|s: &mut App, files: &[std::path::PathBuf], _: &mut Tx| s.attach(files))
        .on_drop_hover(|s: &mut App, over: bool, _: &mut Tx| { s.drop_over = over; s.composer_error = None; })
}

/// "Trust this project?", before a project's first turn: what an agent working there can run,
/// and Trust (Return) or Cancel (Escape, which keeps the draft).
fn trust_dialog(s: &App, k: &Kit, layer: &mut Vec<El>) {
    use rune_kit::{Button, ButtonVariant, Dialog};
    let Some(project) = &s.confirm_trust else { return };
    let what = format!(
        "Agents will work in {} and can run what it sets up for them: Claude Code hooks, MCP servers and other project settings in that folder can run commands as soon as a turn starts. Only trust folders whose contents you know.",
        tilde(project)
    );
    let buttons = row().key("trust-buttons").w_full().padding(16.0).gap(8.0).justify_end()
        .child(Button::new("Cancel").variant(ButtonVariant::Ghost).on_press(|s: &mut App| s.confirm_trust = None).build(k).key("trust-cancel"))
        .child(Button::new("Trust and send").variant(ButtonVariant::Contrast).on_press_tx(|s: &mut App, tx: &mut Tx| s.trust(tx)).build(k).default_button().key("trust-confirm"));
    Dialog::new("trust", "Trust this project?").description(what).width(440.0).fit().open(true)
        .on_close(|s: &mut App| s.confirm_trust = None)
        .child(buttons)
        .build(k, layer);
}

/// "Stop background work?": before a stop that would end what a thread's CLI still runs in the
/// background (or, quitting, any CLI at work), naming what stops, as T3 Code refuses to replace a
/// process with background work. Return confirms, Esc keeps it all running.
fn stop_dialog(s: &App, k: &Kit, layer: &mut Vec<El>) {
    use rune_kit::{Button, ButtonVariant, Dialog};
    use crate::app::StopWhat;
    let Some((id, what)) = s.confirm_stop else { return };
    let (title, description, action) = match what {
        StopWhat::Quit => {
            let working = s.at_work();
            let names: Vec<String> = working.iter().take(3).map(|t| format!("\u{201c}{}\u{201d}", crate::harness::first_line(&t.title, 40))).collect();
            let more = if working.len() > 3 { format!(" and {} more", working.len() - 3) } else { String::new() };
            let bg: Vec<String> = working.iter().filter(|t| t.has_background()).map(|t| t.background_counts()).collect();
            let bg = if bg.is_empty() { String::new() } else { format!(" That includes work in the background ({}).", bg.join("; ")) };
            let n = if working.len() == 1 { "1 thread is".to_owned() } else { format!("{} threads are", working.len()) };
            ("Quit Yoke?", format!("{n} still working: {}{more}. Quitting stops them.{bg}", names.join(", ")), "Quit")
        }
        _ => {
            let Some(t) = s.threads.iter().find(|t| t.id == id) else { return };
            let named: Vec<String> = t.background.iter().take(4).map(|b| crate::ui::agents::task_name(&b.description)).collect();
            let more = if t.background.len() > 4 { format!(" and {} more", t.background.len() - 4) } else { String::new() };
            let running = format!("Claude Code is still running {} in the background: {}{more}.", t.background_counts(), named.join(", "));
            let (why, action) = match what {
                StopWhat::Turn => ("It hasn't stopped the turn yet. Forcing the stop ends the CLI, and they stop with it.", "Force stop"),
                StopWhat::Restart => ("This message needs Claude Code restarted (another access level or setting, or the CLI's own effort), which stops them. Wait for them to finish and send it then, or stop them.", "Stop and send"),
                _ => ("What they've done so far stays.", "Stop them"),
            };
            ("Stop background work?", format!("{running} {why}"), action)
        }
    };
    let buttons = row().key("stop-buttons").w_full().padding(16.0).gap(8.0).justify_end()
        .child(Button::new("Cancel").variant(ButtonVariant::Ghost).on_press(|s: &mut App| s.confirm_stop = None).build(k).key("stop-cancel"))
        .child(Button::new(action).variant(ButtonVariant::Danger).on_press_tx(|s: &mut App, tx: &mut Tx| s.confirm_stop(tx)).build(k).default_button().key("stop-confirm"));
    Dialog::new("stop", title).description(description).width(460.0).fit().open(true)
        .on_close(|s: &mut App| s.confirm_stop = None)
        .child(buttons)
        .build(k, layer);
}

/// Settings' toolbar icon: an eight-toothed cog round a hub, on the 16-point grid and 1-point
/// line of the OpenCode set (whose own settings glyph, a hexagon round a circle, reads as a
/// nut). Tooth tips at radius 6.5, roots on a 5.0 circle, each tooth 17° across at the tip
/// and 26° at the root.
pub static COG: Icon = Icon {
    name: "cog",
    view: 16.0,
    body: r#"<path d="M6.88 3.13L7.04 1.57L8.96 1.57L9.12 3.13A5.0 5.0 0 0 1 10.65 3.76L11.87 2.77L13.23 4.13L12.24 5.35A5.0 5.0 0 0 1 12.87 6.88L14.43 7.04L14.43 8.96L12.87 9.12A5.0 5.0 0 0 1 12.24 10.65L13.23 11.87L11.87 13.23L10.65 12.24A5.0 5.0 0 0 1 9.12 12.87L8.96 14.43L7.04 14.43L6.88 12.87A5.0 5.0 0 0 1 5.35 12.24L4.13 13.23L2.77 11.87L3.76 10.65A5.0 5.0 0 0 1 3.13 9.12L1.57 8.96L1.57 7.04L3.13 6.88A5.0 5.0 0 0 1 3.76 5.35L2.77 4.13L4.13 2.77L5.35 3.76A5.0 5.0 0 0 1 6.88 3.13Z" stroke="currentColor" stroke-linejoin="round"/><circle cx="8" cy="8" r="2" stroke="currentColor"/>"#,
};

/// Window background around the card.
const FRAME: f32 = 8.0;
/// Where the traffic lights' middle is (`main.rs` places them at y = 17), and the toolbar's
/// contents and the sidebar toggle with them.
const LIGHTS_MID: f32 = 24.0;
/// The sidebar toggle: a Large icon button, this far past the traffic lights (or the window's
/// edge in full screen) while the sidebar is shut, or this far in from the sidebar's edge
/// while it's open.
const TOGGLE: f32 = 28.0;
const TOGGLE_PAST: f32 = 16.0;
const TOGGLE_EDGE: f32 = 18.0;
const TOGGLE_END: f32 = 10.0;

/// What the sidebar would tell you if it were open: how many threads want you, and the most
/// pressing reason (an approval, then a failure, then a finished turn you haven't read).
#[derive(Clone, Copy)]
struct Attention {
    count: usize,
    color: rune::Color,
    what: &'static str,
}

impl Attention {
    fn of(s: &App, k: &Kit) -> Option<Attention> {
        let asks = s.threads.iter().filter(|t| !t.asks.is_empty()).count();
        let failed = s.threads.iter().filter(|t| t.asks.is_empty() && t.unread && t.failed).count();
        let unread = s.threads.iter().filter(|t| t.asks.is_empty() && t.unread && !t.failed).count();
        let count = asks + failed + unread;
        let (color, what) = if asks > 0 {
            (k.c.warning.icon, "needs approval")
        } else if failed > 0 {
            (k.c.danger.icon, "failed")
        } else {
            (k.c.bg_accent, "unread")
        };
        (count > 0).then_some(Attention { count, color, what })
    }
}

/// The badge on the sidebar glyph's top-right corner (the glyph is 16 points in the 28-point
/// button, its panel's corner at (20, 9)), ringed in the card's colour so it reads as part of
/// the icon rather than a stray dot.
const BADGE: f32 = 11.0;
const BADGE_RING: f32 = 2.0;
const GLYPH_CORNER: (f32, f32) = (20.0, 9.0);

/// The sidebar's show/hide button, travelling between the traffic lights and the sidebar's
/// header as it opens, as SplitView's own does, but on the traffic lights' line.
fn sidebar_toggle(s: &App, lights: f32, attention: Option<Attention>, shortcut: &str, reduced: bool, k: &Kit) -> El {
    let open = s.sidebar.is_open();
    let label = match attention.filter(|_| !open) {
        Some(a) if a.count == 1 => format!("Show sidebar (1 thread {})", a.what),
        Some(a) => format!("Show sidebar ({} threads want you)", a.count),
        None if open => "Hide sidebar".to_owned(),
        None => "Show sidebar".to_owned(),
    };
    let mut b = IconButton::new(&rune_kit::shell::SIDEBAR)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Large)
        .label(&label)
        .help(&label)
        .shortcut(shortcut)
        .on_press(|s: &mut App| s.sidebar.toggle())
        .build(k)
        .expanded(open);
    if let Some(a) = attention.filter(|_| !open) {
        let (x, y) = (GLYPH_CORNER.0 - BADGE / 2.0, GLYPH_CORNER.1 - BADGE / 2.0);
        b = b.child(
            el().key("sidebar-toggle-badge").size(BADGE, BADGE).radius(BADGE / 2.0).fill(a.color).border(BADGE_RING, k.c.bg_base)
                .absolute(x, y).a11y_hidden(),
        );
    }
    let x = if open {
        s.sidebar.width() - TOGGLE_END - TOGGLE
    } else if lights > 0.0 {
        lights + TOGGLE_PAST
    } else {
        TOGGLE_EDGE
    };
    let b = b.key("sidebar-toggle").absolute(x, LIGHTS_MID - TOGGLE / 2.0);
    if reduced { b } else { b.motion(Spring::new(0.3, 0.0)) }
}

/// The toolbar's leading title, as in T3 Code: the project (its icon, its name muted), a
/// chevron, then the thread.
fn breadcrumb(p: Option<&crate::app::Project>, title: &str, k: &Kit) -> El {
    let c = &k.c;
    let project = p.map_or("No project", |p| p.name.as_str());
    let mark = match p {
        Some(p) => sidebar::project_avatar(p, 16.0, k).key("crumb-icon"),
        None => rune::icon(&icons::FOLDER, 14.0).key("crumb-icon").color(c.icon_faint).a11y_hidden(),
    };
    row()
        .key("breadcrumb")
        .label(format!("{project}, {title}"))
        .gap(6.0)
        .center_y()
        .shrink()
        .min_w(0.0)
        // The folder half opens another folder, as ⌘O does.
        .child(row().key("crumb-folder").gap(6.0).center_y().cursor(rune::Cursor::Pointer).help("Open Folder…")
            .on_press_tx(|s: &mut App, tx: &mut Tx| s.choose_folder(tx))
            .child(mark)
            .child(text(project.to_owned()).key("crumb-project").font(k.ty.ui).color(c.text_muted).a11y_hidden()))
        .child(rune::icon(&icons::CHEVRON_RIGHT, 12.0).key("crumb-sep").color(c.icon_faint).a11y_hidden())
        .child(text(title.to_owned()).key("crumb-title").font(k.ty.ui_medium).color(c.text_strong).truncate().shrink().a11y_hidden())
}

/// "Open in …", as T3 Code's toolbar has: the editor picked last, opening the thread's folder
/// (its worktree, if it has one), and a menu of the others on this computer.
fn open_in(s: &App, k: &Kit, layer: &mut Vec<El>) -> Option<El> {
    use crate::editors::{self, Kind};
    let c = &k.c;
    let folder = s.open_target()?;
    let current = s.editor();
    let key = s.keys.label(Cmd::OpenInEditor);
    let where_ = tilde(&folder);
    let mark = editor_icon(current, k);
    let open = rune_kit::Button::new(format!("Open in {}", current.name))
        .variant(rune_kit::ButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .help(format!("Open {where_} in {}", current.name))
        .shortcut(&key)
        .label_with({
            let ty = k.ty.small_medium;
            let ink = c.text_base;
            move |_, _| row().gap(6.0).center_y().child(mark).child(text("Open").font(ty).color(ink))
        })
        .on_press(move |s: &mut App| s.open_in(current))
        .build(k)
        .px(8.0)
        .key("open-in");
    let open_menu = s.open_menu == Some("open-in");
    let chevron = IconButton::new(&icons::CHEVRON_DOWN)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .label("Open in another app")
        .expanded(open_menu)
        .on_press(|s: &mut App| s.toggle_menu("open-in"))
        .build(k)
        .key("open-in-more");
    let mut menu = rune_kit::Menu::new("open-in-menu")
        .trigger(chevron)
        .open(open_menu)
        .placement(rune_kit::Placement::BottomEnd)
        .min_width(200.0)
        .on_close(|s: &mut App| s.open_menu = None);
    let mut last = None;
    for e in editors::installed().iter().copied() {
        if last.is_some_and(|l: Kind| l != e.kind) { menu = menu.separator(); }
        last = Some(e.kind);
        menu = menu.item(rune_kit::MenuItem::new(e.name)
            .lead(editor_icon(e, k))
            .checked(e.id == current.id)
            .on_select(move |s: &mut App| s.open_in(e)));
    }
    let chevron = menu.build(k, layer);
    Some(row().key("open-in-group").h(24.0).gap(0.0).center_y().radius(7.0).border(1.0, c.border_weak)
        .child(open)
        .child(el().w(1.0).h(14.0).fill(c.border_weak))
        .child(chevron))
}

/// An app's mark (OpenCode's set has the common editors and terminals), else a plain glyph.
fn editor_icon(e: crate::editors::Editor, k: &Kit) -> El {
    use crate::editors::Kind;
    match rune_theme_opencode::app_icons::named(e.id, k.dark) {
        Some(mark) => rune::icon(mark, 14.0).a11y_hidden(),
        None => {
            let glyph = match e.kind { Kind::Terminal => &icons::CONSOLE, Kind::Files => &icons::FOLDER, Kind::Editor => &icons::CODE };
            rune::icon(glyph, 14.0).color(k.c.icon_base).a11y_hidden()
        }
    }
}

/// Updates, at the toolbar's end as in T3 Code: a circling arrow that checks for one, turning
/// while it checks and downloads. Once an update is installed the same button grows a label and
/// restarts into it (held back while a thread is at work, a turn or its background work, since
/// quitting would cut it off). A build run
/// from source doesn't update itself, so it has no button.
fn update_button(s: &App, k: &Kit) -> Option<El> {
    use crate::updater::Status;
    let c = &k.c;
    if !crate::updater::installed() && s.update.status == Status::Idle { return None; }
    if let Status::Ready(version) = &s.update.status {
        let busy = !s.at_work().is_empty();
        let help = if busy { format!("Yoke {version} is downloaded. Restart once the running work (turns, agents, background commands) finishes or is stopped.") } else { format!("Yoke {version} is downloaded. Restart to use it.") };
        return Some(rune_kit::Button::new("Restart to update")
            .variant(rune_kit::ButtonVariant::Contrast)
            .control_size(ControlSize::Small)
            .icon(&ROTATE)
            .disabled(busy)
            .help(help)
            .on_press(|s: &mut App| { s.restart_to_update(); })
            .build(k)
            .key("update"));
    }
    let ago = s.update.checked().map(|d| crate::ui::usage::span(d.as_secs()));
    let version = env!("CARGO_PKG_VERSION");
    let (label, help, ink) = match &s.update.status {
        Status::Checking => ("Checking for updates", format!("Checking for updates…\nYoke {version}"), c.icon_muted),
        Status::Installing(v) => ("Downloading update", format!("Downloading Yoke {v}…\nIt installs in the background; restart when it's ready."), c.icon_muted),
        Status::Failed(e) => ("Check for updates", format!("Couldn't update: {e}\nClick to try again."), c.warning.icon),
        Status::UpToDate => ("Check for updates", format!("Yoke {version} is up to date{}.\nClick to check again.", ago.map_or(String::new(), |a| format!(" · checked {a} ago"))), c.icon_muted),
        _ => ("Check for updates", format!("Yoke {version}\nChecks at launch and every 6 hours."), c.icon_muted),
    };
    let glyph = rune::icon(&ROTATE, 16.0).key("update-glyph").color(ink).a11y_hidden();
    let glyph = if s.update.working() { glyph.looping(rune::LoopSpec::rotation(1.0)) } else { glyph };
    Some(rune_kit::Button::new(label)
        .variant(rune_kit::ButtonVariant::GhostMuted)
        .help(help)
        .label_with(move |_, _| glyph)
        .on_press(|s: &mut App| s.check_for_update())
        .build(k)
        .px(6.0)
        .key("update"))
}

/// A clockwise circling arrow, centred so it can turn in place: a 300° arc round (8, 8) from
/// two o'clock to twelve, the head at twelve pointing on round.
pub static ROTATE: Icon = Icon {
    name: "yoke-rotate",
    view: 16.0,
    body: r#"<path d="M12.55 5.38A5.25 5.25 0 1 1 8 2.75" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" fill="none"/><path d="M5.9 0.9L8 2.75L5.9 4.6" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

/// The new-thread screen's centre.
/// The new-thread screen: "What should we build in yoke?", the project a picker (as T3 Code's
/// headline is), since the sidebar lists every project's threads together.
fn hero(s: &App, k: &Kit, layer: &mut Vec<El>) -> El {
    let c = &k.c;
    let h = s.active_harness();
    let installed: Vec<&str> = s.installed.iter().filter(|(_, p)| p.is_some()).map(|(h, _)| h.name()).collect();
    let found = if installed.is_empty() { "No agent CLIs found on PATH".to_owned() } else { format!("Found {}", installed.join(", ")) };
    let head = rune::TextStyle { size: 26.0, line_height: 32.0, ..k.ty.body_medium };
    let headline = row().key("headline").center_y()
        .child(text("What should we build in").font(head).color(c.text_strong).mr(2.0))
        .child(picker::project_view(s, k, head, layer))
        .child(text("?").font(head).color(c.text_strong));
    let path = s.project().map(|p| tilde(&p.path)).unwrap_or_default();
    col().key("hero").w_full().grow(1.0).min_h(0.0).center().gap(10.0).px(24.0)
        .child(sidebar::avatar(h, k).size(36.0, 36.0).radius(10.0))
        .child(headline)
        .child(text(format!("{path}  ·  {found}")).font(k.ty.small).color(c.text_faint))
}

/// `path` with the home folder as "~".
pub fn tilde(path: &std::path::Path) -> String {
    match std::env::var_os("HOME").map(std::path::PathBuf::from) {
        Some(home) if path.starts_with(&home) => format!("~/{}", path.strip_prefix(&home).unwrap_or(path).display()),
        _ => path.display().to_string(),
    }
}

/// The main pane with no projects yet (a first launch with no Claude Code history).
fn no_projects(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let key = s.keys.label(Cmd::OpenFolder);
    let add = rune_kit::Button::new("Add project")
        .variant(rune_kit::ButtonVariant::Contrast)
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.choose_folder(tx))
        .build(k)
        .key("add-first-project");
    col().key("hero").w_full().grow(1.0).min_h(0.0).center().gap(12.0).px(24.0)
        .child(rune::icon(&icons::FOLDER, 28.0).color(c.icon_weaker))
        .child(text("Add a project to start").font(rune::TextStyle { size: 22.0, line_height: 28.0, ..k.ty.body_medium }).color(c.text_strong))
        .child(text(if key.is_empty() { "Pick a folder for the agents to work in.".to_owned() } else { format!("Pick a folder for the agents to work in ({key}).") }).font(k.ty.small).color(c.text_faint))
        .child(add)
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// Window keys: everything in the shortcut table (`crate::shortcuts`). The prompt field runs
/// the same table first while it has focus. The native Settings… item takes its menu
/// equivalent before either.
pub fn on_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    crate::shortcuts::dispatch(s, ev, tx)
}
