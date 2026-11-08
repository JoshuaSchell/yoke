//! The window: sidebar of threads, and a main pane with the transcript over the composer.

pub mod composer;
pub mod hud;
#[cfg(target_os = "macos")]
pub mod mac;
pub mod settings;
pub mod sidebar;
pub mod theme;
pub mod transcript;
pub mod usage;

use std::time::Duration;

use rune::{Cx, El, KeyEvent, Spring, Tx, col, el, row, text};
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
        main.child(settings::view(s, k))
    } else if s.usage_open {
        main.child(usage::page(s, k))
    } else {
        let body = match s.thread() {
            Some(_) => transcript::view(s, k, (s.theme, dark)),
            None => hero(s, k),
        };
        main.child(body).child(composer::view(s, k, &mut layer))
    };
    if s.hud {
        let items = s.threads.iter().map(|t| t.items.len()).sum();
        main = main.child(el().absolute(0.0, 12.0).w_full().child(row().w_full().justify_end().pr(12.0).child(hud::view(k, s.threads.len(), items))));
    }

    let title = if s.settings { "Settings".to_owned() } else if s.usage_open { "Usage".to_owned() } else { s.thread().map_or("New thread".to_owned(), |t| t.title.clone()) };
    // The toolbar band is 48 points tall inside the card, which starts `FRAME` down, so its
    // middle is 32 down. The traffic lights sit 24 down, where macOS apps put them; the
    // toolbar's contents lift `FRAME` to line up with them.
    let crumbs = breadcrumb(&s.project.name, &title, k).offset(0.0, -FRAME);
    let (hud_key, settings_key, sidebar_key) = (s.keys.label(Cmd::FrameStats), s.keys.label(Cmd::Settings), s.keys.label_static(Cmd::ToggleSidebar));
    let hud_button = IconButton::new(&icons::WINDOW_ANALYTICS)
        .variant(IconButtonVariant::GhostMuted)
        .expanded(s.hud)
        .label(if s.hud { "Hide frame stats" } else { "Show frame stats" })
        .shortcut(&hud_key)
        .on_press(|s: &mut App| s.hud = !s.hud)
        .build(k)
        .key("hud-toggle");
    let settings_button = IconButton::new(&icons::SETTINGS_GEAR)
        .variant(IconButtonVariant::GhostMuted)
        .expanded(s.settings)
        .label(if s.settings { "Close Settings" } else { "Settings" })
        .shortcut(&settings_key)
        .on_press(|s: &mut App| if s.settings { s.close_settings() } else { s.open_settings() })
        .build(k)
        .key("settings-open");
    let toolbar = Toolbar::new().leading(crumbs).trailing(row().gap(2.0).center_y().offset(0.0, -FRAME).child(usage::toolbar_button(s, k)).child(hud_button).child(settings_button));
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

/// The new-thread screen's centre.
fn hero(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let h = s.active_harness();
    let installed: Vec<&str> = s.installed.iter().filter(|(_, p)| p.is_some()).map(|(h, _)| h.name()).collect();
    let found = if installed.is_empty() { "No agent CLIs found on PATH".to_owned() } else { format!("Found {}", installed.join(", ")) };
    col().key("hero").w_full().grow(1.0).min_h(0.0).center().gap(10.0).px(24.0)
        .child(sidebar::avatar(h, k).size(36.0, 36.0).radius(10.0))
        .child(text("What should we build?").font(rune::TextStyle { size: 26.0, line_height: 32.0, ..k.ty.body_medium }).color(c.text_strong))
        .child(text(format!("{}  ·  {}", s.project.name, found)).font(k.ty.small).color(c.text_faint))
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
