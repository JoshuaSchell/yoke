//! The window: sidebar of threads, and a main pane with the transcript over the composer.

pub mod composer;
pub mod hud;
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
use rune_kit::{ControlSize, IconButton, IconButtonVariant, Kit};
use rune_theme_opencode::icons;

use crate::app::App;
use crate::shortcuts::Cmd;

pub fn view(s: &App, cx: &mut Cx<App>) -> El {
    let dark = s.appearance.dark(cx.system_dark());
    let k: &Kit = theme::kit(s.theme, dark);
    let mut layer = Vec::new();

    let mut main = col().key("main").w_full().h_full().min_h(0.0);
    main = if s.settings {
        main.child(settings::view(s, k, &mut layer))
    } else if s.usage_open {
        main.child(usage::page(s, k))
    } else {
        match s.thread() {
            Some(_) => main.child(transcript::view(s, k, (s.theme, dark))).child(composer::view(s, k, &mut layer)),
            None if s.projects.is_empty() => main.child(no_projects(s, k)),
            None => main.child(hero(s, k)).child(composer::view(s, k, &mut layer)),
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
    let crumbs = breadcrumb(s.project().map_or("No project", |p| p.name.as_str()), &title, k).offset(0.0, -FRAME);
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
    if let Some(button) = update_button(s, k) {
        trailing = trailing.child(button.mr(6.0));
    }
    let toolbar = Toolbar::new().leading(crumbs).trailing(trailing.child(usage::toolbar_button(s, k)).child(hud_button).child(settings_button));
    let attention = Attention::of(s, k);
    let window = SplitView::new(main)
        .sidebar(&s.sidebar, |s: &mut App| &mut s.sidebar, sidebar::view(s, k))
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
    // Images dropped anywhere in the window go to the composer; anything else is refused with
    // a note there.
    sidebar::delete_dialog(s, k, &mut layer);
    rune_kit::root(window, layer)
        .on_drop_files(|s: &mut App, files: &[std::path::PathBuf], _: &mut Tx| s.attach(files))
        .accepts_extensions(&crate::app::IMAGE_EXTENSIONS)
        .on_drop_hover(|s: &mut App, over: bool, _: &mut Tx| { s.drop_over = over; s.composer_error = None; })
        .on_drop_rejected(|s: &mut App, files: &[std::path::PathBuf], _: &mut Tx| {
            s.drop_over = false;
            let name = files.first().and_then(|f| f.file_name()).map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            s.composer_error = Some(format!("{name} isn't an image. Drop a PNG, JPEG, GIF or WebP."));
        })
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

/// The toolbar's leading title, as in T3 Code: the project (folder icon, muted), a chevron,
/// then the thread.
fn breadcrumb(project: &str, title: &str, k: &Kit) -> El {
    let c = &k.c;
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
            .child(rune::icon(&icons::FOLDER, 14.0).key("crumb-icon").color(c.icon_faint).a11y_hidden())
            .child(text(project.to_owned()).key("crumb-project").font(k.ty.ui).color(c.text_muted).a11y_hidden()))
        .child(rune::icon(&icons::CHEVRON_RIGHT, 12.0).key("crumb-sep").color(c.icon_faint).a11y_hidden())
        .child(text(title.to_owned()).key("crumb-title").font(k.ty.ui_medium).color(c.text_strong).truncate().shrink().a11y_hidden())
}

/// "Restart to update", once an update is installed and waits for a restart. Held back while
/// a turn runs, since quitting would cut it off.
fn update_button(s: &App, k: &Kit) -> Option<El> {
    let version = s.update_ready()?;
    let busy = s.threads.iter().any(|t| t.busy);
    let help = if busy { format!("Yoke {version} is installed. Finish or stop the running turns, then restart.") } else { format!("Yoke {version} is installed. Restart to use it.") };
    Some(rune_kit::Button::new("Restart to update")
        .variant(rune_kit::ButtonVariant::Contrast)
        .control_size(ControlSize::Small)
        .disabled(busy)
        .on_press(|s: &mut App| { s.restart_to_update(); })
        .build(k)
        .help(help)
        .key("restart-to-update"))
}

/// The new-thread screen's centre.
fn hero(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let h = s.active_harness();
    let installed: Vec<&str> = s.installed.iter().filter(|(_, p)| p.is_some()).map(|(h, _)| h.name()).collect();
    let found = if installed.is_empty() { "No agent CLIs found on PATH".to_owned() } else { format!("Found {}", installed.join(", ")) };
    col().key("hero").w_full().grow(1.0).min_h(0.0).center().gap(10.0).px(24.0)
        .child(sidebar::avatar(h, k).size(36.0, 36.0).radius(10.0))
        .child(text("What should we build?").font(rune::TextStyle { size: 26.0, line_height: 32.0, ..k.ty.body_medium }).color(c.text_strong))
        .child(text(format!("{}  ·  {}", s.project().map_or("", |p| p.name.as_str()), found)).font(k.ty.small).color(c.text_faint))
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
