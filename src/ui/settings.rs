//! Settings (⌘, or Rune Code › Settings…), after T3 Code's: a rail of pages down the left
//! (Appearance, New threads, Chat, Agents, Keyboard shortcuts, Advanced) and the chosen page
//! beside it. It takes the main pane while it's open, since nothing else there can be used
//! meanwhile; Esc, Done, or picking a thread closes it. What the pages change lives in
//! `crate::settings`.

use rune::{El, ScrollSpec, TextStyle, Tx, col, row, text};
use rune_kit::shell::SidebarRow;
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, SegmentedControl, Select, SelectAppearance, Switch};
use rune_theme_opencode::icons;

use crate::app::{APPEARANCES, App};
use crate::harness::{self, Access, Harness};
use crate::picker::effort_label;
use crate::settings::{FollowUp, PAGES, Page, SendKey};
use crate::shortcuts::{self, Cmd};
use crate::ui::theme::THEMES;

/// The page's reading column.
const W: f32 = 680.0;
const RAIL_W: f32 = 212.0;
/// The width of a select in a setting row.
const PICK_W: f32 = 200.0;

pub fn view(s: &App, k: &Kit, layer: &mut Vec<El>) -> El {
    let page = s.settings_page;
    let body = match page {
        Page::Appearance => appearance(s, k),
        Page::NewThreads => new_threads(s, k, layer),
        Page::Chat => chat(s, k),
        Page::Agents => agents(s, k),
        Page::Shortcuts => shortcuts_page(s, k),
        Page::Advanced => advanced(s, k),
    };
    let done = Button::new("Done")
        .variant(ButtonVariant::Outline)
        .on_press(|s: &mut App| s.close_settings())
        .build(k)
        .key("settings-done");
    let head = row().w_full().center_y().gap(12.0).pb(4.0)
        .child(text(page.title()).font(TextStyle { size: 24.0, line_height: 30.0, ..k.ty.body_medium }).color(k.c.text_strong).grow(1.0).min_w(0.0))
        .child(done);
    let content = col().key(format!("settings-{}", page.id())).w_full().grow(1.0).min_h(0.0).min_w(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .scroll_edge_fade(18.0)
        .child(row().w_full().justify_center().px(32.0).pt(28.0).pb(40.0)
            .child(col().w_full().max_w(W).gap(12.0).child(head).child(body)));
    row().key("settings").w_full().grow(1.0).min_h(0.0)
        .child(rail(s, k))
        .child(rune::el().w(1.0).h_full().fill(k.c.border_weak))
        .child(content)
        // A chord being recorded arrives here when the recorder button has focus.
        .on_key(|s: &mut App, ev: &rune::KeyEvent, tx: &mut Tx| s.recording.is_some() && shortcuts::dispatch(s, ev, tx))
}

/// The pages, down the left.
fn rail(s: &App, k: &Kit) -> El {
    let mut c = col().key("settings-rail").w(RAIL_W).h_full().px(10.0).pt(24.0).gap(2.0).role("tablist").label("Settings pages");
    for p in PAGES {
        c = c.child(SidebarRow::new(p.title())
            .icon(p.icon())
            .height(32.0)
            .selected(s.settings_page == p)
            .on_select(move |s: &mut App, _: &mut Tx| s.show_settings_page(p))
            .build(k)
            .key(format!("settings-page-{}", p.id())));
    }
    c
}

fn appearance(s: &App, k: &Kit) -> El {
    let themes = SegmentedControl::new(&THEMES.iter().map(|t| t.label()).collect::<Vec<_>>(), THEMES.iter().position(|t| *t == s.theme))
        .label("Theme")
        .fit()
        .on_pick(|s: &mut App, i: usize, _: &mut Tx| s.set_theme(THEMES[i]))
        .build(k)
        .key("theme");
    let appearance = SegmentedControl::new(&APPEARANCES.iter().map(|a| a.label()).collect::<Vec<_>>(), APPEARANCES.iter().position(|a| *a == s.appearance))
        .label("Appearance")
        .fit()
        .on_pick(|s: &mut App, i: usize, _: &mut Tx| s.set_appearance(APPEARANCES[i]))
        .build(k)
        .key("appearance");
    card(k, vec![
        setting(k, "Theme", "Colours for the whole window and code, in light and dark.", themes),
        setting(k, "Mode", "Follow the system, or always light or dark.", appearance),
    ])
}

/// Each harness's menus need their own `App::open_menu` id.
fn menu_id(model: bool, h: Harness) -> &'static str {
    match (model, h) {
        (true, Harness::Claude) => "default-model-claude",
        (true, Harness::Codex) => "default-model-codex",
        (true, Harness::OpenCode) => "default-model-opencode",
        (false, Harness::Claude) => "default-effort-claude",
        (false, Harness::Codex) => "default-effort-codex",
        (false, Harness::OpenCode) => "default-effort-opencode",
    }
}

fn pick(key: &str, options: &[&str], current: &str, label: &str, menu: &'static str, s: &App) -> Select {
    Select::new(key, options)
        .appearance(SelectAppearance::Field)
        .width(PICK_W)
        .current(Some(current))
        .label(label)
        .open(s.open_menu == Some(menu))
        .on_toggle(move |s: &mut App| s.toggle_menu(menu))
        .on_close(|s: &mut App| s.open_menu = None)
}

fn new_threads(s: &App, k: &Kit, layer: &mut Vec<El>) -> El {
    let c = &k.c;
    let found: Vec<Harness> = harness::ALL.into_iter().filter(|h| s.binary(*h).is_some()).collect();
    let default_h = s.default_harness();

    let mut start = Vec::new();
    if found.is_empty() {
        start.push(setting(k, "Agent", "No agent CLI was found. Install Claude Code, Codex or OpenCode, then look again in Agents.", rune::el()));
    } else {
        let names: Vec<&str> = found.iter().map(|h| h.name()).collect();
        let agent = pick("default-harness", &names, default_h.name(), "Default agent", "default-harness", s)
            .on_choose(|s: &mut App, v: &str, _: &mut Tx| {
                if let Some(h) = harness::ALL.into_iter().find(|h| h.name() == v) { s.set_default_harness(h); }
                s.open_menu = None;
            })
            .build(k, layer);
        start.push(setting(k, "Agent", "The CLI a new thread starts with.", agent));
    }
    let levels: Vec<&str> = harness::ACCESS.iter().map(|a| a.label()).collect();
    let access = pick("default-access", &levels, s.defaults.access.label(), "Default access", "default-access", s)
        .on_choose(|s: &mut App, v: &str, _: &mut Tx| {
            if let Some(a) = Access::from_label(v) { s.set_default_access(a); }
            s.open_menu = None;
        })
        .build(k, layer);
    let access_help = if s.defaults.access == Access::Ask {
        "What the agent may do without asking. Codex and OpenCode can't ask, so for them Ask first is Read only."
    } else {
        "What the agent may do without asking you first."
    };
    start.push(setting(k, "Access", access_help, access));

    let mut per_harness = Vec::new();
    for h in &found {
        let h = *h;
        let models: Vec<&str> = s.models(h).iter().map(|m| m.label.as_str()).collect();
        let current_model = s.default_model(h);
        let model = pick(&format!("default-model-{}", h.id()), &models, s.model_label(h, &current_model), &format!("{} default model", h.name()), menu_id(true, h), s)
            .on_choose(move |s: &mut App, v: &str, _: &mut Tx| {
                s.set_default_model(h, v);
                s.open_menu = None;
            })
            .build(k, layer);
        let mut controls = row().gap(8.0).center_y().child(model);
        let efforts = h.efforts();
        if !efforts.is_empty() {
            let labels: Vec<&str> = efforts.iter().map(|e| effort_label(e)).collect();
            let current = s.default_effort(h);
            let effort = pick(&format!("default-effort-{}", h.id()), &labels, effort_label(&current), &format!("{} default effort", h.name()), menu_id(false, h), s)
                .width(130.0)
                .on_choose(move |s: &mut App, v: &str, _: &mut Tx| {
                    if let Some(e) = h.efforts().iter().find(|e| effort_label(e) == v) { s.set_default_effort(h, e); }
                    s.open_menu = None;
                })
                .build(k, layer);
            controls = controls.child(effort);
        }
        per_harness.push(row().key(format!("defaults-{}", h.id())).w_full().min_h(56.0).px(18.0).py(10.0).gap(14.0).center_y()
            .child(crate::ui::sidebar::avatar(h, k))
            .child(text(h.name()).font(label_style(k)).color(c.text_strong).grow(1.0).min_w(0.0).truncate())
            .child(controls));
    }

    let reset = Button::new("Reset")
        .variant(ButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .disabled(!s.defaults_changed())
        .on_press(|s: &mut App| s.reset_defaults())
        .build(k)
        .key("defaults-reset");
    let new_key = paren(&s.keys.label(Cmd::NewThread));
    let mut page = col().w_full().gap(12.0)
        .child(row().w_full().center_y().gap(8.0)
            .child(section_title(k, "Starting point", Some(&format!("A new thread{new_key} starts with these. The composer can still change them before you send."))).grow(1.0).min_w(0.0))
            .child(col().pt(16.0).child(reset)))
        .child(card(k, start));
    if !per_harness.is_empty() {
        page = page
            .child(section_title(k, "Model and effort", Some("For each agent. Switching agent on the new-thread screen takes that agent's.")))
            .child(card(k, per_harness));
    }
    page
}

fn chat(s: &App, k: &Kit) -> El {
    let mod_return = if cfg!(target_os = "macos") { "⌘ Return" } else { "Ctrl+Return" };
    let sends = [SendKey::Return, SendKey::ModReturn];
    let send = SegmentedControl::new(&["Return", mod_return], sends.iter().position(|k| *k == s.defaults.send))
        .label("Send with")
        .fit()
        .on_pick(move |s: &mut App, i: usize, _: &mut Tx| s.set_send_key(sends[i]))
        .build(k)
        .key("send-key");
    let send_help = match s.defaults.send {
        SendKey::Return => "Return sends the prompt and ⇧Return starts a new line.".to_owned(),
        SendKey::ModReturn => format!("{} sends the prompt and Return starts a new line, for writing longer prompts.", s.send_labels().0),
    };
    let follow_ups = [FollowUp::Queue, FollowUp::Steer];
    let follow = SegmentedControl::new(&["Queue", "Steer"], follow_ups.iter().position(|f| *f == s.defaults.follow_up))
        .label("Follow-ups while a turn runs")
        .fit()
        .on_pick(move |s: &mut App, i: usize, _: &mut Tx| s.set_follow_up(follow_ups[i]))
        .build(k)
        .key("follow-up");
    let (_, other) = s.send_labels();
    let other = if other.is_empty() { String::new() } else { format!(" {other} does the other.") };
    let follow_help = match s.defaults.follow_up {
        FollowUp::Queue => format!("Sending while the agent works waits for the turn to end.{other}"),
        FollowUp::Steer => format!("Sending while the agent works stops the turn and sends straight away, on the same session.{other}"),
    };
    card(k, vec![
        setting(k, "Send with", &send_help, send),
        setting(k, "Follow-ups", &follow_help, follow),
    ])
}

fn agents(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let mut clis = Vec::new();
    for h in harness::ALL {
        let (found, color) = match s.binary(h) {
            Some(p) => (p.display().to_string(), c.text_muted),
            None => ("Not installed".to_owned(), c.text_faint),
        };
        // "Default" (let the CLI pick) isn't a model, and a CLI that isn't here has none.
        let models = s.models(h).iter().filter(|m| m.value != harness::models::DEFAULT).count();
        let models = match (s.binary(h), models) {
            (None, _) => String::new(),
            (Some(_), 0) => "Its own default model".to_owned(),
            (Some(_), n) => format!("{n} model{}", if n == 1 { "" } else { "s" }),
        };
        clis.push(row().key(format!("cli-{}", h.id())).w_full().min_h(52.0).px(18.0).py(10.0).gap(16.0).center_y()
            .child(crate::ui::sidebar::avatar(h, k))
            .child(col().grow(1.0).min_w(0.0).gap(2.0)
                .child(text(h.name()).font(label_style(k)).color(c.text_strong))
                .child(text(found).font(help_style(k)).color(color).truncate()))
            .child(text(models).font(help_style(k)).color(c.text_faint)));
    }
    let again = Button::new("Look again")
        .variant(ButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .on_press(|s: &mut App| s.rescan_clis())
        .build(k)
        .key("clis-rescan");
    let peers = Switch::new(s.defaults.peer_messages)
        .hide_label()
        .label("Messages from your other sessions")
        .on_toggle(|s: &mut App| s.set_peer_messages(!s.defaults.peer_messages))
        .build(k)
        .key("peer-messages");
    let peers_help = if s.defaults.peer_messages {
        "Your Claude Code sessions on this computer, here or in a terminal, can message a thread here, for example to say which files they're editing. A message can't give a thread more access than it has. Takes effect from each thread's next message."
    } else {
        "Threads here turn away messages from your other Claude Code sessions, and the sender is told. Takes effect from each thread's next message."
    };
    col().w_full().gap(12.0)
        .child(row().w_full().center_y().gap(8.0)
            .child(section_title(k, "Agent CLIs", Some("Found on PATH and in the usual install folders.")).grow(1.0).min_w(0.0))
            .child(col().pt(16.0).child(again)))
        .child(card(k, clis))
        .child(section_title(k, "Claude Code", None))
        .child(card(k, vec![setting(k, "Messages from your other sessions", peers_help, peers)]))
}

fn advanced(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let hud = Switch::new(s.hud)
        .hide_label()
        .label("Show frame stats")
        .on_toggle(|s: &mut App| s.hud = !s.hud)
        .build(k)
        .key("hud");
    let frame_stats = format!("Per-frame timings drawn over the transcript{}.", paren(&s.keys.label(Cmd::FrameStats)));
    let mut rows = vec![setting(k, "Frame stats", &frame_stats, hud)];
    if let Some(path) = s.prefs.path() {
        let reveal = Button::new(if cfg!(target_os = "macos") { "Show in Finder" } else { "Show folder" })
            .variant(ButtonVariant::Outline)
            .control_size(ControlSize::Small)
            .on_press(|s: &mut App| s.reveal_prefs())
            .build(k)
            .key("prefs-reveal");
        rows.push(setting(k, "Settings file", &path.display().to_string(), reveal));
    }
    rows.push(row().w_full().min_h(52.0).px(18.0).py(12.0).gap(20.0).center_y()
        .child(text("Version").font(label_style(k)).color(c.text_strong).grow(1.0))
        .child(text(format!("Rune Code {}", env!("CARGO_PKG_VERSION"))).font(help_style(k)).color(c.text_muted)));
    card(k, rows)
}

fn label_style(k: &Kit) -> TextStyle {
    TextStyle { size: 14.0, line_height: 20.0, ..k.ty.body_medium }
}

fn help_style(k: &Kit) -> TextStyle {
    TextStyle { size: 13.0, line_height: 18.0, ..k.ty.body }
}

fn paren(label: &str) -> String {
    if label.is_empty() { String::new() } else { format!(" ({label})") }
}

fn section_title(k: &Kit, title: &str, help: Option<&str>) -> El {
    let mut c = col().w_full().gap(2.0).pt(16.0).pb(2.0).px(4.0)
        .child(text(title.to_owned()).font(TextStyle { size: 16.0, line_height: 22.0, ..k.ty.body_medium }).color(k.c.text_strong));
    if let Some(h) = help { c = c.child(text(h.to_owned()).font(help_style(k)).color(k.c.text_muted).wrap()); }
    c
}

/// Rows in a rounded panel, with hairlines between them.
fn card(k: &Kit, rows: Vec<El>) -> El {
    let mut c = col().w_full().radius(12.0).fill(k.c.surface_raised_base).border(1.0, k.c.border_weak).clip_rounded(12.0);
    let n = rows.len();
    for (i, r) in rows.into_iter().enumerate() {
        c = c.child(r);
        if i + 1 < n { c = c.child(rune::el().w_full().h(1.0).fill(k.c.border_weak)); }
    }
    c
}

fn setting(k: &Kit, title: &str, help: &str, control: El) -> El {
    row().w_full().min_h(64.0).px(18.0).py(12.0).gap(20.0).center_y()
        .child(col().grow(1.0).min_w(0.0).gap(3.0)
            .child(text(title.to_owned()).font(label_style(k)).color(k.c.text_strong))
            .child(text(help.to_owned()).font(help_style(k)).color(k.c.text_muted).wrap()))
        .child(control)
}

/// Every command with its chord. Click a chord to record a new one: Escape cancels and
/// Backspace clears it.
fn shortcuts_page(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let changed = shortcuts::ALL.iter().any(|cmd| !s.keys.is_default(*cmd));
    let reset_all = Button::new("Reset all")
        .variant(ButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .disabled(!changed)
        .on_press(|s: &mut App| shortcuts::reset_all(s))
        .build(k)
        .key("shortcuts-reset-all");
    let head = row().w_full().center_y().gap(8.0)
        .child(section_title(k, "Commands", Some("Click a shortcut, then press the new keys. Esc cancels, ⌫ clears.")).grow(1.0).min_w(0.0))
        .child(col().pt(16.0).child(reset_all));

    let mut rows = Vec::new();
    for cmd in shortcuts::ALL {
        let recording = s.recording == Some(cmd);
        let label = s.keys.label(cmd);
        let shown = if recording { "Press keys…".to_owned() } else if label.is_empty() { "None".to_owned() } else { label };
        let recorder = Button::new(&shown)
            .variant(if recording { ButtonVariant::Contrast } else { ButtonVariant::Outline })
            .on_press(move |s: &mut App| shortcuts::begin_recording(s, cmd))
            .build(k)
            .key(format!("record-{}", cmd.id()));
        let mut controls = row().gap(4.0).center_y().child(recorder);
        if !s.keys.is_default(cmd) {
            controls = controls.child(IconButton::new(&icons::RESET)
                .variant(IconButtonVariant::GhostMuted)
                .label("Reset to default")
                .on_press(move |s: &mut App| shortcuts::reset(s, cmd))
                .build(k)
                .key(format!("reset-{}", cmd.id())));
        }
        let mut title = col().grow(1.0).min_w(0.0).gap(2.0)
            .child(text(cmd.title()).font(TextStyle { size: 14.0, line_height: 20.0, ..k.ty.body }).color(c.text_base).truncate());
        if let Some((_, why)) = s.key_error.as_ref().filter(|(e, _)| *e == cmd) {
            title = title.child(text(why.clone()).font(help_style(k)).color(c.danger.fg));
        }
        rows.push(row().key(format!("shortcut-{}", cmd.id())).w_full().min_h(48.0).px(18.0).py(6.0).gap(12.0).center_y()
            .child(title)
            .child(controls));
    }
    // The prompt's own keys, as Settings › Chat has them.
    let (send, _) = s.send_labels();
    let send_title = match s.defaults.follow_up {
        FollowUp::Queue => "Send, or queue while a turn runs",
        FollowUp::Steer => "Send, or steer while a turn runs",
    };
    let newline = if s.defaults.send == SendKey::ModReturn { "↩" } else { "⇧↩" };
    for (title, keys) in [(send_title, send.as_str()), ("New line", newline)] {
        rows.push(row().w_full().min_h(48.0).px(18.0).py(6.0).gap(12.0).center_y()
            .child(text(title).font(TextStyle { size: 14.0, line_height: 20.0, ..k.ty.body }).color(c.text_base).truncate().grow(1.0).min_w(0.0))
            .child(text(keys.to_owned()).font(help_style(k)).color(c.text_faint).pr(12.0)));
    }
    col().w_full().gap(12.0).child(head).child(card(k, rows))
}

