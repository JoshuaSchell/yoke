//! Yoke: a small T3 Code-style app for prompting coding agents, to see how Rune does.
//!
//! `cargo run -- [project folder]` opens the window on that folder (default: the current one).
//! Headless: `-- --snap out.png --scene demo --size 1200x800` renders a scene without a window.

mod app;
mod attention;
#[cfg(test)]
mod bench;
mod editors;
mod checkpoint;
mod diff;
mod harness;
mod history;
mod icon_editor;
mod picker;
mod project_icon;
mod settings;
mod store;
mod updater;
mod worktree;
mod shortcuts;
mod ui;
mod usage;

use std::path::PathBuf;
use std::time::Duration;

use rune::prefs::Prefs;
use rune::{AppSpec, Chrome, MenuItem, Tx};

use app::App;

/// The projects are the ones saved in prefs (on the first launch, the folders Claude Code has
/// sessions in). A folder named on the command line is added and opened; otherwise the one
/// used last is. Where the app was started from doesn't matter, as with any desktop app.
fn init() -> App {
    let mut prefs = Prefs::open(app::APP_ID);
    app::seed_projects(&mut prefs);
    let arg = rune::extra_args().iter().find(|a| !a.starts_with('-')).map(PathBuf::from);
    std::thread::spawn(app::purge_old_pastes);
    App::new(arg, prefs)
}

/// The app icon, small enough to decode at launch. A packaged app's own icon (the bundle's on
/// macOS) is what the Dock and launchers show; this covers `cargo run` and Windows' title bars.
const ICON: &[u8] = include_bytes!("../assets/icon-256.png");

fn poll(s: &mut App, clock: f64, tx: &mut Tx) -> bool {
    #[cfg(target_os = "macos")]
    ui::mac::fix_menus();
    let mut changed = s.drain(tx);
    changed |= attention::poll(s, clock, tx);
    // The "Working… 12s" counter and the frame-stats HUD redraw on their own beat, not every
    // frame: `poll` says true only when something on screen changed.
    let beat = if s.hud { (clock * 2.0) as u64 } else { clock as u64 };
    if (s.hud || s.threads.iter().any(|t| t.busy)) && beat != s.beat {
        s.beat = beat;
        changed = true;
    }
    if s.hud {
        tx.wake_after(Duration::from_millis(500));
    }
    // Plan limits are read again every few minutes, and "resets in 12m" moves on; an idle
    // window still draws nothing in between.
    if s.usage_tick(std::time::SystemTime::now()) {
        changed = true;
    }
    // An installed app looks for an update at launch and every few hours.
    if s.update_tick() {
        changed = true;
    }
    tx.wake_after(Duration::from_secs(60));
    changed
}

/// Named starting states for headless snapshots (`--scene`).
type Scene = (&'static str, fn(&mut App));

const SCENES: &[Scene] = &[
    ("demo", app::scenes::demo),
    ("long", app::scenes::long),
    ("empty", app::scenes::empty),
    ("projects", app::scenes::projects),
    ("picker", app::scenes::picker),
    ("effort", app::scenes::effort),
    ("settings", app::scenes::settings),
    ("settings-new-threads", app::scenes::settings_new_threads),
    ("settings-chat", app::scenes::settings_chat),
    ("usage", app::scenes::usage),
    ("queue", app::scenes::queue),
    ("approval", app::scenes::approval),
    ("attach", app::scenes::attach),
    ("collapsed", app::scenes::collapsed),
    ("delete", app::scenes::delete),
    ("context-hover", app::scenes::context_hover),
    ("plan-hover", app::scenes::plan_hover),
    ("update-ready", app::scenes::update_ready),
    ("open-in", app::scenes::open_in),
    ("tools", app::scenes::tools),
    ("trust", app::scenes::trust),
    ("icon", app::scenes::icon),
    ("diff", app::scenes::diff),
];

fn main() {
    app::migrate_from_rune_code();
    attention::install();
    rune::prewarm_gpu();
    // Inside a bundle macOS already shows Yoke.app's icon.
    let bundled = std::env::current_exe().is_ok_and(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"));
    if !bundled { let _ = rune::set_app_icon(ICON); }
    let spec = AppSpec {
        min_size: Some((720.0, 480.0)),
        scenes: SCENES,
        fonts: rune_theme_opencode::FONTS,
        families: rune_theme_opencode::FAMILIES,
        setup: Some(|s: &mut App, w| { attention::set_waker(&w); s.set_waker(w) }),
        poll: Some(poll),
        on_key: Some(ui::on_key),
        ..AppSpec::new("Yoke", (1180.0, 780.0), init, ui::view)
    };
    // Traffic lights centred 24 points down, as in other macOS apps; the toolbar's contents
    // and the sidebar toggle line up with them (`ui::LIGHTS_MID`).
    let chrome = Chrome { full_size_content: true, traffic_lights: Some((16.0, 17.0)), drag_regions: true };
    // Yoke › Settings…, on the Settings shortcut as saved at launch (a rebinding reaches the
    // menu at the next launch; the shortcut table answers it straight away). `ui::mac` moves
    // it below About.
    let keys = shortcuts::Keys::load(&Prefs::open(app::APP_ID));
    let mut settings = MenuItem::new("Settings…", |s: &mut App, _: &mut Tx| s.open_settings()).id("settings");
    if let Ok(k) = keys.spec(shortcuts::Cmd::Settings).parse() {
        settings = settings.key_equivalent(k);
    }
    let mut open = MenuItem::new("Add Project…", |s: &mut App, tx: &mut Tx| s.choose_folder(tx)).id("open-folder").menu("File");
    if let Ok(k) = keys.spec(shortcuts::Cmd::OpenFolder).parse() {
        open = open.key_equivalent(k);
    }
    rune::App::new(spec).chrome(chrome).standard_menus().menu_item(settings).menu_item(open).on_frame_encoded(ui::hud::record).on_focus(attention::on_focus).run();
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use rune::{Env, Key, KeyEvent, Mods, Runtime};

    use crate::app::{App, Kind, ToolState};
    use crate::harness::{Access, Harness};

    fn key(k: Key) -> KeyEvent {
        KeyEvent { key: k, mods: Mods::default() }
    }

    fn new_app() -> App {
        let mut app = App::new(Some(std::env::current_dir().unwrap()), rune::prefs::Prefs::memory());
        app.ask_trust = false;
        app
    }

    fn runtime() -> Runtime<App> {
        let mut app = new_app();
        assert!(app.binary(Harness::Claude).is_some(), "claude must be installed");
        app.choose_harness(Harness::Claude);
        app.model = "haiku".into();
        app.access = Access::ReadOnly;
        let mut rt = Runtime::new(app, crate::ui::view, (1100.0, 760.0), Env { system_dark: true, scale: 2.0 });
        rt.frame(0.0);
        rt
    }

    /// Types `prompt` into the composer, presses Return, and pumps frames until the turn ends.
    /// Returns the turn's assistant text.
    fn turn(rt: &mut Runtime<App>, prompt: &str) -> String {
        let before = rt.state.thread().map_or(0, |t| t.items.len());
        rt.key(key(Key::Text(prompt.into())));
        rt.frame(1.0 / 60.0);
        rt.key(key(Key::Enter));
        rt.frame(1.0 / 60.0);
        let id = rt.state.selected.expect("Return started a thread");
        assert!(rt.state.is_running(id));
        let start = Instant::now();
        let (mut first, mut frames) = (None, 0);
        while rt.state.is_running(id) && start.elapsed() < Duration::from_secs(120) {
            rt.update(|s, tx| { s.drain(tx); });
            rt.frame(1.0 / 60.0);
            frames += 1;
            let t = rt.state.thread().unwrap();
            if first.is_none() && t.items[before..].iter().any(|i| !matches!(i.kind, Kind::User(_))) {
                first = Some(start.elapsed());
            }
            std::thread::sleep(Duration::from_millis(16));
        }
        rt.update(|s, tx| { s.drain(tx); });
        rt.frame(1.0 / 60.0);
        let t = rt.state.thread().unwrap();
        assert!(!t.busy && matches!(t.items.last().unwrap().kind, Kind::TurnEnd { .. }));
        eprintln!("first event after {first:?}, done after {:?}, {frames} frames", start.elapsed());
        t.items[before..].iter().filter_map(|i| match &i.kind { Kind::Assistant { text, .. } => Some(text.as_str()), _ => None }).collect()
    }

    /// Drives the real `claude` CLI through the UI. Costs a few tokens, so it's opt-in:
    /// `cargo test -- --ignored live_claude --nocapture`.
    #[test]
    #[ignore]
    fn live_claude_streams_resumes_and_calls_tools() {
        let mut rt = runtime();
        let a = turn(&mut rt, "Reply with exactly: rune works");
        assert!(a.to_lowercase().contains("rune works"), "answer: {a:?}");
        assert!(rt.elements().iter().any(|e| e.text.as_deref().is_some_and(|x| x.to_lowercase().contains("rune works"))), "the answer is on screen");
        let session = rt.state.thread().unwrap().session.clone();
        assert!(session.is_some());

        // The second turn resumes the same session, so it remembers the first.
        let b = turn(&mut rt, "What exact phrase did I just ask you to reply with? Answer with only the phrase.");
        assert!(b.to_lowercase().contains("rune works"), "answer: {b:?}");

        // A tool call shows as a row and finishes.
        let c = turn(&mut rt, "Use the Read tool to read Cargo.toml in the current directory, then tell me only the package name.");
        let t = rt.state.thread().unwrap();
        let tools: Vec<_> = t.items.iter().filter_map(|i| match &i.kind { Kind::Tool { name, state, .. } => Some((name.clone(), *state)), _ => None }).collect();
        eprintln!("tools {tools:?}, answer {c:?}");
        assert!(tools.iter().any(|(n, s)| n == "Read" && *s == ToolState::Ok), "tools: {tools:?}");
        assert!(c.contains("yoke"), "answer: {c:?}");
    }

    /// "Ask first": the real CLI asks before running a command, ⌘Y allows it, and the command's
    /// output comes back. `cargo test -- --ignored live_claude --nocapture`.
    #[test]
    #[ignore]
    fn live_claude_asks_before_running_a_command() {
        let mut rt = runtime();
        rt.state.access = Access::Ask;
        rt.key(key(Key::Text("Run the shell command `echo rune-approval-ok` with the Bash tool, then reply with only its output.".into())));
        rt.frame(1.0 / 60.0);
        rt.key(key(Key::Enter));
        rt.frame(1.0 / 60.0);
        let id = rt.state.selected.unwrap();
        pump_until(&mut rt, 90, |s| !s.thread().unwrap().asks.is_empty() || !s.is_running(id));
        let t = rt.state.thread().unwrap();
        let ask = &t.asks.first().expect("the CLI asked before running the command").ask;
        eprintln!("asked: {} {} suggestions={}", ask.tool, ask.input, ask.suggestions.len());
        assert_eq!(ask.tool, "Bash");
        rt.key(cmd('y'));
        rt.frame(1.0 / 60.0);
        pump_until(&mut rt, 120, |s| !s.is_running(id));
        let t = rt.state.thread().unwrap();
        let answer: String = t.items.iter().filter_map(|i| match &i.kind { Kind::Assistant { text, .. } => Some(text.as_str()), _ => None }).collect();
        assert!(answer.contains("rune-approval-ok"), "answer: {answer:?}");
        assert!(tool_outputs(&rt.state).iter().any(|(s, o)| *s == ToolState::Ok && o.contains("rune-approval-ok")));
    }

    /// Esc stops a running turn: the process is killed and the turn ends as stopped.
    #[test]
    #[ignore]
    fn live_claude_escape_stops_the_turn() {
        let mut rt = runtime();
        rt.key(key(Key::Text("Count from 1 to 300, one number per line, nothing else.".into())));
        rt.frame(1.0 / 60.0);
        rt.key(key(Key::Enter));
        rt.frame(1.0 / 60.0);
        let id = rt.state.selected.unwrap();
        let start = Instant::now();
        // Wait for the answer to start streaming, then press Esc.
        while start.elapsed() < Duration::from_secs(60) {
            rt.update(|s, tx| { s.drain(tx); });
            rt.frame(1.0 / 60.0);
            if rt.state.thread().unwrap().items.iter().any(|i| matches!(&i.kind, Kind::Assistant { text, .. } if text.len() > 20)) { break; }
            std::thread::sleep(Duration::from_millis(16));
        }
        rt.key(key(Key::Escape));
        rt.frame(1.0 / 60.0);
        let pressed = Instant::now();
        while rt.state.is_running(id) && pressed.elapsed() < Duration::from_secs(10) {
            rt.update(|s, tx| { s.drain(tx); });
            rt.frame(1.0 / 60.0);
            std::thread::sleep(Duration::from_millis(16));
        }
        eprintln!("stopped {:?} after Esc", pressed.elapsed());
        let t = rt.state.thread().unwrap();
        for i in t.items.iter().rev().take(3) {
            match &i.kind {
                Kind::Assistant { text, .. } => eprintln!("assistant: {} chars", text.len()),
                Kind::TurnEnd { secs, stopped, .. } => eprintln!("turn end: {secs:.1}s stopped={stopped}"),
                Kind::Error(m) => eprintln!("error: {m}"),
                _ => eprintln!("other"),
            }
        }
        assert!(!rt.state.is_running(id) && !t.busy);
        assert!(matches!(t.items.last().unwrap().kind, Kind::TurnEnd { stopped: true, .. }));
    }

    fn args(h: Harness, effort: &str) -> Vec<String> {
        let turn = crate::harness::Turn { harness: h, prompt: "hi".into(), model: "Default".into(), effort: effort.into(), access: Access::Edits, resume: None, cwd: ".".into(), images: Vec::new(), peer_messages: true };
        h.command(std::path::Path::new("x"), &turn).get_args().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn effort_maps_to_each_cli_flag() {
        let claude = args(Harness::Claude, "xhigh");
        assert!(claude.windows(2).any(|w| w == ["--effort", "xhigh"]), "{claude:?}");
        let codex = args(Harness::Codex, "high");
        assert!(codex.windows(2).any(|w| w == ["-c", "model_reasoning_effort=\"high\""]), "{codex:?}");
        // The default, and harnesses without an effort flag, send none.
        assert!(!args(Harness::Claude, crate::harness::DEFAULT_EFFORT).iter().any(|a| a.contains("effort")));
        assert!(!args(Harness::OpenCode, "high").iter().any(|a| a.contains("effort")));
    }

    /// Scratch files for tests, under `target/` so `cargo clean` removes them and the system
    /// temp folder collects nothing.
    fn test_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-tmp")
    }

    fn cmd(c: char) -> KeyEvent {
        KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ..Mods::default() } }
    }

    fn headless(app: App) -> Runtime<App> {
        let mut rt = Runtime::new(app, crate::ui::view, (1100.0, 760.0), Env { system_dark: true, scale: 2.0 });
        rt.on_key = Some(crate::ui::on_key);
        rt.frame(0.0);
        rt
    }

    fn shift_cmd(c: char) -> KeyEvent {
        KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, shift: true, ..Mods::default() } }
    }

    fn lane_ids(s: &App, lane: crate::app::Lane) -> Vec<u64> {
        s.lane_threads(lane).iter().map(|t| t.id).collect()
    }

    /// The inbox: ⇧⌘S settles the open thread and goes on to the next one in the inbox; a
    /// working thread can't settle; the choice is saved by session; activity wakes it.
    #[test]
    fn settling_moves_threads_out_of_the_inbox() {
        use crate::app::Lane;
        let mut app = new_app();
        crate::app::scenes::demo(&mut app);
        let mut rt = headless(app);
        // Last back first: the Codex thread (2m), the open one (4m), the failed one (12m).
        assert_eq!(lane_ids(&rt.state, Lane::Inbox), [102, 100, 103]);
        assert_eq!(lane_ids(&rt.state, Lane::Working), [101]);
        // The Working shelf starts shut, so arrow keys skip its rows until it's opened.
        assert_eq!(rt.state.sidebar_order(), [102, 100, 103, 110, 111, 112]);
        rt.update(|s, _| s.toggle_working_shelf());
        assert_eq!(rt.state.sidebar_order(), [102, 100, 103, 101, 110, 111, 112]);
        rt.update(|s, _| s.toggle_working_shelf());

        rt.key(shift_cmd('s'));
        rt.frame(1.0 / 60.0);
        assert_eq!(lane_ids(&rt.state, Lane::Inbox), [102, 103]);
        assert_eq!(rt.state.selected, Some(103), "goes on to the thread below");
        let saved: std::collections::HashMap<String, bool> = rt.state.prefs.get("settled").unwrap();
        assert_eq!(saved.get("demo"), Some(&true));

        // A running thread stays where it is.
        rt.update(|s, _| s.select(102));
        rt.key(shift_cmd('s'));
        rt.frame(1.0 / 60.0);
        assert_eq!(lane_ids(&rt.state, Lane::Working), [101]);

        // The turn finishing brings a settled thread back, and forgets the hand-made choice.
        rt.update(|s, _| s.apply(100, crate::harness::Event::Exited { code: Some(0), stderr: String::new() }));
        assert_eq!(crate::app::App::lane(rt.state.threads.iter().find(|t| t.id == 100).unwrap()), Lane::Inbox);
        let saved: std::collections::HashMap<String, bool> = rt.state.prefs.get("settled").unwrap();
        assert_eq!(saved.get("demo"), None);

        // Settling the last inbox thread lands on the new-thread screen.
        for id in [100, 102, 103] {
            rt.update(|s, _| { s.select(id); s.settle(id); });
        }
        assert!(lane_ids(&rt.state, Lane::Inbox).is_empty());
        assert_eq!(rt.state.selected, None);
        rt.frame(1.0 / 60.0);
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("All caught up")));
    }

    /// The row's ✓ shows under the pointer and settles it; ↩ on a settled row brings it back.
    #[test]
    fn row_actions_settle_and_unsettle() {
        use crate::app::Lane;
        let mut app = new_app();
        crate::app::scenes::demo(&mut app);
        let mut rt = headless(app);
        let (x, y) = rt.find("thread-102").unwrap();
        assert!(rt.presented("settle").unwrap().1 < 0.01, "hidden until hovered");
        rt.pointer_move(x, y);
        for i in 1..30 { rt.frame(i as f64 / 60.0); }
        assert!(rt.presented("settle").unwrap().1 > 0.99, "shown under the pointer");
        let (bx, by) = rt.find("thread-102/trailing/actions/settle").unwrap();
        rt.pointer_down(bx, by);
        rt.pointer_up(bx, by);
        rt.frame(0.6);
        assert!(lane_ids(&rt.state, Lane::Settled).contains(&102));
        assert_eq!(rt.state.selected, Some(100), "pressing ✓ doesn't open the row");
        // Let the row spring into the shelf: a row on the move doesn't take the hover.
        rt.frame(1.0);
        rt.frame(1.5);

        let (x, y) = rt.find("thread-102").unwrap();
        rt.pointer_move(x, y);
        for i in 1..30 { rt.frame(2.0 + i as f64 / 60.0); }
        let (bx, by) = rt.find("thread-102/trailing/actions/unsettle").unwrap();
        rt.pointer_down(bx, by);
        rt.pointer_up(bx, by);
        rt.frame(2.6);
        assert!(lane_ids(&rt.state, Lane::Inbox).contains(&102));
    }

    /// The row's ✕ asks first: Escape keeps the thread, Return deletes it and its Claude Code
    /// session's files.
    #[test]
    fn delete_asks_then_takes_the_session_files() {
        let home = std::env::temp_dir().join(format!("yoke-delete-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let sid = "5f5aefca-fd04-4499-9f53-1b734ef86e56";
        let mut app = new_app();
        crate::app::scenes::demo(&mut app);
        app.claude_home = Some(home.clone());
        let t = app.threads.iter_mut().find(|t| t.id == 102).unwrap();
        (t.harness, t.session) = (Harness::Claude, Some(sid.into()));
        let files = crate::history::session_files(&home, app.current.as_ref().unwrap(), sid);
        std::fs::create_dir_all(&files[1]).unwrap();
        std::fs::write(&files[0], "{}").unwrap();
        let mut rt = headless(app);

        let press_x = |rt: &mut Runtime<App>, at: f64| {
            let (x, y) = rt.find("thread-102").unwrap();
            rt.pointer_move(x, y);
            for i in 1..30 { rt.frame(at + i as f64 / 60.0); }
            let (bx, by) = rt.find("thread-102/trailing/actions/delete").unwrap();
            rt.pointer_down(bx, by);
            rt.pointer_up(bx, by);
            rt.frame(at + 0.6);
        };
        press_x(&mut rt, 0.0);
        assert_eq!(rt.state.confirm_delete, Some(102));
        assert!(rt.find("delete-dialog").is_some(), "the dialog shows");
        rt.key(key(Key::Escape));
        rt.frame(1.0);
        assert_eq!(rt.state.confirm_delete, None);
        assert!(rt.state.threads.iter().any(|t| t.id == 102), "Escape keeps it");

        press_x(&mut rt, 2.0);
        rt.key(key(Key::Enter));
        rt.frame(3.0);
        assert!(!rt.state.threads.iter().any(|t| t.id == 102), "Return deletes it");
        let start = Instant::now();
        while files.iter().any(|f| f.exists()) && start.elapsed() < Duration::from_secs(3) { std::thread::sleep(Duration::from_millis(20)); }
        assert!(files.iter().all(|f| !f.exists()), "its session's files are gone");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Saved sessions untouched for a week land settled, unless settled or kept by hand.
    #[test]
    fn old_sessions_land_settled() {
        use std::time::SystemTime;
        use crate::app::Lane;
        let mut app = new_app();
        let _ = app.prefs.set("settled", &std::collections::HashMap::from([("kept".to_owned(), false), ("done".to_owned(), true)]));
        let app = App::new(Some(std::env::current_dir().unwrap()), app.prefs);
        let mut app = app;
        let week = Duration::from_secs(8 * 24 * 3600);
        let s = |id: &str, ago: Duration| crate::history::Session { id: id.into(), title: id.into(), modified: SystemTime::now() - ago, path: "/nonexistent".into() };
        let here = app.current.clone().unwrap();
        app.land_sessions(&here, vec![s("old", week), s("new", Duration::from_secs(60)), s("kept", week), s("done", Duration::ZERO)]);
        let title = |l| app.lane_threads(l).iter().map(|t| t.title.clone()).collect::<Vec<_>>();
        assert_eq!(title(Lane::Inbox), ["new", "kept"]);
        assert_eq!(title(Lane::Settled), ["done", "old"]);
    }

    #[test]
    fn command_comma_opens_settings_and_escape_closes_it() {
        let mut rt = headless(new_app());
        rt.key(cmd(','));
        rt.frame(1.0 / 60.0);
        assert!(rt.state.settings);
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("Appearance")));
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("Keyboard shortcuts")));
        rt.key(key(Key::Escape));
        rt.frame(1.0 / 60.0);
        assert!(!rt.state.settings);
    }

    /// The model chip opens the picker on the current harness: a star pins a model to
    /// Favorites (without choosing it), the search filters, ⌘2 picks the second row, the
    /// arrows and Return pick too, and the effort column sets the effort.
    #[test]
    fn model_picker() {
        let mut app = new_app();
        // As if Claude Code were installed, so this runs where it isn't (CI).
        app.installed.retain(|(h, _)| *h != Harness::Claude);
        app.installed.push((Harness::Claude, Some("/usr/bin/true".into())));
        app.choose_harness(Harness::Claude);
        app.models.insert(Harness::Claude, crate::harness::models::fallback(Harness::Claude));
        let mut rt = headless(app);
        let click = |rt: &mut Runtime<App>, key: &str, t: f64| {
            let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} on screen"));
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            rt.frame(t);
        };
        click(&mut rt, "model-chip", 0.1);
        assert!(rt.state.picker_open());
        for i in 0..20 { rt.frame(0.1 + i as f64 / 60.0); }
        assert!(rt.find("pick-claude-opus").is_some() && rt.find("effort-high").is_some());

        click(&mut rt, "pick-claude-sonnet/star", 0.5);
        assert!(rt.state.is_favorite(Harness::Claude, "sonnet"));
        assert_eq!(rt.state.model, crate::harness::models::DEFAULT, "a star doesn't choose");
        assert!(rt.state.picker_open());
        click(&mut rt, "rail-favorites", 0.6);
        assert_eq!(rt.state.picker_rows().0.iter().map(|r| r.value.as_str()).collect::<Vec<_>>(), ["sonnet"]);
        click(&mut rt, "rail-claude", 0.7);

        click(&mut rt, "effort-max", 0.8);
        assert_eq!(rt.state.effort, "max");
        rt.key(key(Key::Text("hai".into())));
        rt.frame(0.9);
        assert_eq!(rt.state.picker_rows().0.iter().map(|r| r.value.as_str()).collect::<Vec<_>>(), ["haiku"]);
        rt.key(key(Key::Enter));
        rt.frame(1.0);
        assert_eq!(rt.state.model, "haiku");
        assert!(!rt.state.picker_open() && rt.state.draft.focused);

        rt.key(shift_cmd('m'));
        rt.frame(1.1);
        assert!(rt.state.picker_open());
        rt.key(cmd('2'));
        rt.frame(1.2);
        assert_eq!(rt.state.model, rt.state.models(Harness::Claude)[1].value);
        rt.key(shift_cmd('m'));
        rt.frame(1.3);
        rt.key(key(Key::Down));
        rt.key(key(Key::Enter));
        rt.frame(1.4);
        assert_eq!(rt.state.model, rt.state.models(Harness::Claude)[2].value);
        rt.key(shift_cmd('m'));
        rt.frame(1.5);
        rt.key(key(Key::Escape));
        rt.frame(1.6);
        assert!(!rt.state.picker_open());
        assert_eq!(rt.state.model, rt.state.models(Harness::Claude)[2].value);
        assert_eq!(rt.state.picker_summary().1, Some("Max"), "the chip shows the effort");
    }

    /// A file row's "Open in …" goes to the file at the line: a read's first, an edit's
    /// replacement where the file has it now, a write's top. Other rows and gone files have none.
    #[test]
    fn tool_rows_open_their_file_at_the_line() {
        use crate::app::{Item, Kind, ToolState};
        use crate::harness::ToolBody;
        let dir = std::env::temp_dir().join(format!("yoke-open-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("lib.rs");
        std::fs::write(&file, "fn a() {}\n\nfn b() {\n    retry();\n}\n").unwrap();
        let path = file.display().to_string();
        let mut app = new_app();
        crate::app::scenes::demo(&mut app);
        let id = app.selected.unwrap();
        let rows = [
            ("Read", ToolBody::None, "    40\tfn b() {".to_owned()),
            ("Edit", ToolBody::Edits(vec![("x".into(), "    retry();".into())]), String::new()),
            ("Write", ToolBody::Code("fn a() {}".into()), String::new()),
            ("Bash", ToolBody::None, String::new()),
        ];
        let t = app.threads.iter_mut().find(|t| t.id == id).unwrap();
        for (i, (name, body, output)) in rows.into_iter().enumerate() {
            let detail = if name == "Bash" { "ls".to_owned() } else { path.clone() };
            t.items.push(Item { id: 9000 + i as u64, rev: 0, kind: Kind::Tool { call: format!("f{i}"), name: name.into(), detail, state: ToolState::Ok, output, body } });
        }
        assert_eq!(app.tool_file(9000), Some((file.clone(), Some(40))));
        assert_eq!(app.tool_file(9001), Some((file.clone(), Some(4))), "the edit is on line 4 now");
        assert_eq!(app.tool_file(9002), Some((file.clone(), None)));
        assert_eq!(app.tool_file(9003), None, "a command isn't a file");
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(app.tool_file(9000), None, "nor is a deleted one");
        let mut rt = headless(app);
        rt.frame(0.1);
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("open-file")), "file rows have the button");
    }

    /// A project neither Yoke nor Claude Code trusts asks before its first turn, keeping the
    /// draft; Escape sends nothing, Trust sends and remembers, and a folder inside a trusted one
    /// (or one Claude Code trusts) doesn't ask.
    #[test]
    fn a_new_project_asks_to_be_trusted_before_its_first_turn() {
        let mut app = new_app();
        app.installed = vec![(Harness::Claude, Some(fake_claude("claude-trust")))];
        app.choose_harness(Harness::Claude);
        app.ask_trust = true;
        app.claude_trusted = Some(Vec::new());
        let project = app.current.clone().unwrap();
        let mut rt = headless(app);
        rt.key(key(Key::Text("hello".into())));
        rt.key(key(Key::Enter));
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.confirm_trust.as_ref(), Some(&project));
        assert!(rt.state.threads.iter().all(|t| t.items.is_empty()) && rt.state.draft.text == "hello", "nothing sent, the draft kept");
        assert!(rt.find("trust-confirm").is_some());
        rt.key(key(Key::Escape));
        rt.frame(1.0 / 60.0);
        assert!(rt.state.confirm_trust.is_none() && rt.state.draft.text == "hello");
        rt.key(key(Key::Enter));
        rt.frame(1.0 / 60.0);
        let (x, y) = rt.find("trust-confirm").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.confirm_trust.is_none() && rt.state.draft.text.is_empty(), "sent");
        assert!(rt.state.trusted.contains(&project));
        assert!(rt.state.is_trusted(&project.join("sub")), "a folder inside it is trusted too");
        let saved: Vec<String> = rt.state.prefs.get("trusted.projects").unwrap();
        assert_eq!(saved, [project.display().to_string()]);
        rt.update(|s, _| s.claude_trusted = Some(vec![std::path::PathBuf::from("/claude/ok")]));
        assert!(rt.state.is_trusted(std::path::Path::new("/claude/ok/app")) && !rt.state.is_trusted(std::path::Path::new("/elsewhere")));
    }

    /// A long thread title wraps inside the delete dialog instead of running past it.
    #[test]
    fn delete_dialog_wraps_a_long_title() {
        let mut app = new_app();
        crate::app::scenes::delete(&mut app);
        let mut rt = headless(app);
        rt.frame(1.0);
        let rect = |key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).map(|e| e.target).unwrap();
        let (panel, what) = (rect("delete-dialog"), rt.elements().into_iter().find(|e| e.text.as_deref().is_some_and(|t| t.ends_with("can't be undone."))).map(|e| e.target).unwrap());
        assert!(what[0] >= panel[0] && what[0] + what[2] <= panel[0] + panel[2], "the text {what:?} stays in the panel {panel:?}");
        assert!(what[3] > 30.0, "and wraps onto more lines: {what:?}");
    }

    /// The toolbar's update button: a check while idle, a turning arrow while it checks and
    /// downloads, then the same button restarts, held back while a turn runs.
    #[test]
    fn update_button_follows_the_update() {
        use crate::updater::Status;
        let mut app = new_app();
        crate::app::scenes::demo(&mut app);
        app.update.status = Status::Installing("9.9.9".into());
        let mut rt = headless(app);
        let label = |rt: &Runtime<App>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("update")).and_then(|e| e.label.clone());
        assert_eq!(label(&rt).as_deref(), Some("Downloading update"));
        assert!(rt.find("update-glyph").is_some());
        rt.update(|s, _| s.update.status = Status::Ready("9.9.9".into()));
        rt.frame(0.1);
        assert_eq!(label(&rt).as_deref(), Some("Restart to update"));
        let (x, y) = rt.find("update").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.2);
        assert!(rt.state.update_ready().is_some(), "a running turn holds the restart back");
    }

    /// The composer's gauge reads the thread's context window and the toolbar's the plan, each
    /// with the numbers in a hover card; the toolbar's opens the page, which ⇧⌘U and Esc close.
    #[test]
    fn usage_gauges_and_page() {
        use crate::ui::usage::{Good, tone};
        let mut app = new_app();
        crate::app::scenes::demo(&mut app);
        let mut rt = headless(app);
        let label = |rt: &Runtime<App>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).and_then(|e| e.label.clone()).unwrap_or_default();
        let shows = |rt: &Runtime<App>, t: &str| rt.elements().iter().any(|e| e.text.as_deref() == Some(t));
        let ctx = label(&rt, "context-gauge");
        assert!(ctx.contains("64.3K of 200K tokens (32%)"), "{ctx}");
        assert!(rt.find("context-needle").is_some() && rt.find("plan-needle").is_some());
        assert!(rt.find("context-card").is_none() && rt.find("plan-card").is_none(), "no cards until hovered");
        rt.update(|s, _| s.hover = Some("context"));
        rt.frame(0.05);
        assert!(rt.find("context-card").is_some() && shows(&rt, "64.3K of 200K tokens") && shows(&rt, "135.7K left"));
        rt.update(|s, _| s.hover = Some("plan"));
        rt.frame(0.08);
        assert!(rt.find("plan-card").is_some() && rt.find("context-card").is_none());
        assert!(shows(&rt, "Claude Max") && shows(&rt, "Weekly · Fable") && shows(&rt, "12% used"));
        rt.update(|s, _| s.hover = None);
        rt.frame(0.09);
        // The pointer opens and closes them.
        let (x, y) = rt.find("context-gauge").unwrap();
        rt.pointer_move(x, y);
        rt.frame(0.1);
        assert_eq!(rt.state.hover, Some("context"));
        let (x, y) = rt.find("usage-toggle").unwrap();
        rt.pointer_move(x, y);
        rt.frame(0.11);
        assert_eq!(rt.state.hover, Some("plan"));
        rt.pointer_move(x - 300.0, y + 300.0);
        rt.frame(0.12);
        assert_eq!(rt.state.hover, None);

        // Context is good low, the plan good high.
        let k = crate::ui::theme::kit(rt.state.theme, true);
        assert_eq!(tone(0.2, Good::Low, k), k.c.success.icon);
        assert_eq!(tone(0.6, Good::Low, k), k.c.warning.icon);
        assert_eq!(tone(0.95, Good::Low, k), k.c.danger.icon);
        assert_eq!(tone(0.2, Good::High, k), k.c.danger.icon);
        assert_eq!(tone(0.95, Good::High, k), k.c.success.icon);

        // A new thread's context is empty.
        rt.update(|s, _| s.new_thread());
        rt.frame(0.1);
        assert!(label(&rt, "context-gauge").contains("0 of "), "{}", label(&rt, "context-gauge"));

        let (x, y) = rt.find("usage-toggle").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.2);
        assert!(rt.state.usage_open);
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("Spend at API prices")));
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("Opus 5.5")));
        rt.key(key(Key::Escape));
        rt.frame(0.5);
        assert!(!rt.state.usage_open);
        rt.key(shift_cmd('u'));
        rt.frame(0.6);
        assert!(rt.state.usage_open);
        rt.key(shift_cmd('u'));
        rt.frame(0.7);
        assert!(!rt.state.usage_open);
        // Codex doesn't say how full its context is.
        rt.update(|s, _| s.select(102));
        rt.frame(0.8);
        assert!(rt.find("context-gauge").is_none());
        assert!(rt.find("usage-toggle").is_some(), "the plan's gauge stays");
    }

    /// The real plan and transcripts land in the app and on screen (no tokens spent):
    /// `cargo test -- --ignored live_usage --nocapture`.
    #[test]
    #[ignore]
    fn live_usage_lands_in_the_app() {
        let mut app = new_app();
        app.set_waker(std::sync::Arc::new(|| {}));
        let mut rt = headless(app);
        let start = Instant::now();
        while (rt.state.limits_busy() || rt.state.usage_scanning()) && start.elapsed() < Duration::from_secs(30) {
            rt.update(|s, tx| { s.drain(tx); });
            std::thread::sleep(Duration::from_millis(20));
        }
        rt.frame(1.0 / 60.0);
        let l = rt.state.limits.clone().expect("the plan was read");
        eprintln!("after {:?}: {:?} {:?}; {} transcripts, 7 days {}", start.elapsed(), l.plan, l.windows.iter().map(|w| format!("{} {:.0}%", w.label, w.used)).collect::<Vec<_>>(),
            rt.state.usage.files, crate::usage::dollars(rt.state.usage.summary(crate::usage::today(), 7).cost));
        assert!(rt.find("context-gauge").is_some(), "the gauge shows on the new-thread screen");
        rt.key(shift_cmd('u'));
        rt.frame(0.1);
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("Cost by day")));
    }

    /// ⌘O asks for a folder and adds it as a project, starting a new thread there; the
    /// other projects' threads stay. Cancelling changes nothing.
    #[test]
    fn command_o_adds_a_project() {
        use rune::FileDialogKind;
        let mut rt = headless(new_app());
        let here = rt.state.current.clone().unwrap();
        rt.state.threads.push(crate::app::Thread::new(99, "old".into(), Harness::Claude, "Default".into()));
        rt.key(cmd('o'));
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.file_dialog().map(|(kind, o)| (kind, o.title.clone())), Some((FileDialogKind::Folder, "Add Project".into())));
        rt.resolve_file_dialog(None).unwrap();
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.projects.len(), 1, "cancelling adds nothing");

        let dir = std::env::temp_dir().canonicalize().unwrap();
        rt.key(cmd('o'));
        rt.frame(1.0 / 60.0);
        rt.resolve_file_dialog(Some(dir.clone())).unwrap();
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.projects.iter().map(|p| p.path.clone()).collect::<Vec<_>>(), [here.clone(), dir.clone()]);
        assert_eq!(rt.state.current.as_ref(), Some(&dir));
        assert!(rt.state.selected.is_none(), "a new thread in the new project");
        assert!(rt.state.threads.iter().any(|t| t.id == 99), "the first project's threads stay");
        assert_eq!(rt.state.prefs.get::<String>(crate::app::LAST_PROJECT_KEY), Some(dir.to_string_lossy().into_owned()));
        assert_eq!(rt.state.prefs.get::<Vec<String>>(crate::app::PROJECTS_KEY).unwrap().len(), 2);
    }

    /// A Codex thread is saved when its turn ends, and the next launch lists it in its
    /// project with its transcript; deleting it deletes the record.
    #[test]
    fn codex_threads_outlive_the_app() {
        use crate::app::{Item, Thread};
        use crate::harness::Event;
        let root = std::env::temp_dir().join(format!("yoke-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let prefs = || rune::prefs::Prefs::at(root.join("prefs.json"));
        let mut app = App::new(Some(std::env::current_dir().unwrap()), prefs());
        let here = app.current.clone().unwrap();
        let mut t = Thread::new(7, "Write the changelog".into(), Harness::Codex, "Default".into());
        t.project = here.clone();
        t.session = Some("codex-session-1".into());
        t.busy = true;
        t.items.push(Item { id: 1, rev: 0, kind: Kind::User("Write it".into()) });
        t.items.push(Item { id: 2, rev: 0, kind: Kind::Assistant { text: "Done.".into(), streaming: false } });
        app.threads.push(t);
        app.apply(7, Event::Exited { code: Some(0), stderr: String::new() });
        let record = root.join("threads/codex-codex-session-1.json");
        let start = Instant::now();
        while !record.exists() && start.elapsed() < Duration::from_secs(5) { std::thread::sleep(Duration::from_millis(10)); }
        assert!(record.exists(), "the turn's end saved the thread");

        let mut rt = headless(App::new(None, prefs()));
        rt.update(|s, _| s.load_sessions());
        let start = Instant::now();
        while !rt.state.threads.iter().any(|t| t.harness == Harness::Codex) && start.elapsed() < Duration::from_secs(10) {
            rt.update(|s, tx| { s.drain(tx); });
            std::thread::sleep(Duration::from_millis(10));
        }
        let id = rt.state.threads.iter().find(|t| t.harness == Harness::Codex).map(|t| t.id).expect("listed at the next launch");
        rt.update(|s, _| s.select(id));
        let t = rt.state.thread().unwrap();
        assert_eq!((t.title.as_str(), t.session.as_deref(), &t.project), ("Write the changelog", Some("codex-session-1"), &here));
        assert!(matches!(&t.items[1].kind, Kind::Assistant { text, .. } if text == "Done."));
        assert!(t.items.iter().any(|i| matches!(i.kind, Kind::TurnEnd { .. })));

        rt.update(|s, _| s.delete(id));
        assert!(!record.exists(), "deleting the thread deletes its record");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// With "New worktree" on, a new thread's message waits while a worktree is made on a
    /// new branch, then its turn runs there (here Codex, not installed, so it only fails to
    /// start, spending nothing).
    #[test]
    fn a_new_thread_in_a_worktree() {
        use std::process::Command;
        let repo = std::env::temp_dir().join(format!("yoke-wt-app-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        let git = |dir: &std::path::Path, args: &[&str]| Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("a.txt"), "hello").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "first"]);

        let mut app = App::new(Some(repo.clone()), rune::prefs::Prefs::memory());
        app.ask_trust = false;
        if app.binary(Harness::Codex).is_some() { return; }
        app.choose_harness(Harness::Codex);
        let mut rt = headless(app);
        assert!(rt.find("worktree-chip").is_some(), "offered in a git repository");
        rt.update(|s, _| s.toggle_worktree_mode());
        rt.key(key(Key::Text("Fix the build".into())));
        rt.key(key(Key::Enter));
        rt.frame(0.1);
        let t = rt.state.thread().unwrap();
        assert!(t.preparing && t.queue.len() == 1 && t.items.is_empty(), "the message waits for the worktree");

        let start = Instant::now();
        while rt.state.thread().unwrap().preparing && start.elapsed() < Duration::from_secs(10) {
            rt.update(|s, tx| { s.drain(tx); });
            std::thread::sleep(Duration::from_millis(10));
        }
        rt.update(|s, tx| { s.drain(tx); });
        let t = rt.state.thread().unwrap();
        let w = t.worktree.clone().expect("the worktree was made");
        assert!(w.branch.starts_with("yoke/fix-the-build-"), "{}", w.branch);
        assert_eq!(std::fs::read_to_string(w.path.join("a.txt")).unwrap(), "hello");
        assert!(t.queue.is_empty() && matches!(&t.items[0].kind, Kind::User(p) if p == "Fix the build"), "then the message went");
        let head = git(&w.path, &["branch", "--show-current"]);
        assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), w.branch);

        git(&repo, &["worktree", "remove", "--force", w.path.to_str().unwrap()]);
        let _ = std::fs::remove_dir(w.path.parent().unwrap());
        let _ = std::fs::remove_dir_all(&repo);
    }

    /// The folders your Claude Code sessions ran in become projects listing their threads
    /// (read only; prefs stay in memory): `cargo test -- --ignored live_projects --nocapture`.
    #[test]
    #[ignore]
    fn live_projects_list_their_threads() {
        let mut prefs = rune::prefs::Prefs::memory();
        crate::app::seed_projects(&mut prefs);
        let mut app = App::new(None, prefs);
        app.set_waker(std::sync::Arc::new(|| {}));
        let mut rt = headless(app);
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            rt.update(|s, tx| { s.drain(tx); });
            std::thread::sleep(Duration::from_millis(50));
        }
        for p in &rt.state.projects {
            let n = rt.state.threads.iter().filter(|t| rt.state.in_project(t, &p.path)).count();
            eprintln!("{}: {n} threads", p.path.display());
        }
        assert!(!rt.state.threads.is_empty());
    }

    /// Every project's threads share one inbox, the last to come back first, each row naming
    /// its project; picking a thread makes its project the current one; the new-thread
    /// headline's picker chooses, searches and removes projects, and removing one takes its
    /// threads out of the sidebar.
    #[test]
    fn projects_share_one_inbox_and_are_picked_in_the_headline() {
        use crate::app::Thread;
        use rune::{Key, KeyEvent, Mods};
        let mut rt = headless(new_app());
        let a = rt.state.current.clone().unwrap();
        let b = std::env::temp_dir().canonicalize().unwrap();
        rt.update(|s, _| s.add_project(b.clone()));
        for (id, project, ago) in [(1, &a, 10), (2, &b, 5), (3, &b, 20)] {
            let mut t = Thread::new(id, format!("t{id}"), Harness::Claude, "Default".into());
            t.project = project.clone();
            t.updated = std::time::SystemTime::now() - Duration::from_secs(ago);
            rt.state.threads.push(t);
        }
        rt.frame(0.1);
        assert_eq!(rt.state.inbox_order(), [2, 1, 3], "one list across projects, newest first");
        let b_name = b.file_name().unwrap().to_string_lossy().into_owned();
        assert!(rt.find("thread-2").is_some(), "thread 2 is in the inbox");
        let named = format!("{b_name} · ");
        assert!(rt.elements().iter().any(|e| e.path.contains("thread-2") && e.text.as_deref().is_some_and(|x| x.starts_with(&named))), "the row names its project");

        rt.update(|s, _| s.select(2));
        assert_eq!(rt.state.current.as_ref(), Some(&b));
        rt.update(|s, _| s.select(1));
        assert_eq!(rt.state.current.as_ref(), Some(&a));

        // The new-thread screen: "What should we build in <a>?", and its picker.
        rt.update(|s, _| s.new_thread());
        rt.frame(0.2);
        assert!(rt.find("project-chip").is_some());
        rt.update(|s, _| s.open_project_picker());
        rt.frame(0.3);
        assert_eq!(rt.state.project_rows(), [a.clone(), b.clone()]);
        assert_eq!(rt.state.project_pick.active, 0, "the current project is under the arrows");
        rt.update(|s, tx| { s.project_picker_key(&KeyEvent { key: Key::Down, mods: Mods::default() }, tx); });
        rt.update(|s, tx| { s.project_picker_key(&KeyEvent { key: Key::Enter, mods: Mods::default() }, tx); });
        assert_eq!(rt.state.current.as_ref(), Some(&b), "↓ ↩ chose the second project");
        assert!(!rt.state.project_picker_open());
        assert!(rt.state.selected.is_none(), "still on the new-thread screen");
        rt.update(|s, _| { s.open_project_picker(); s.project_pick.search = rune::TextEdit::new(b.display().to_string()); s.project_picker_searched(); });
        assert_eq!(rt.state.project_rows(), std::slice::from_ref(&b), "the search narrows the list");

        rt.update(|s, _| { s.remove_project(&b); });
        rt.frame(0.4);
        assert!(rt.state.threads.iter().all(|t| t.project != b));
        assert_eq!(rt.state.current.as_ref(), Some(&a));
        assert_eq!(rt.state.prefs.get::<Vec<String>>(crate::app::PROJECTS_KEY), Some(vec![a.to_string_lossy().into_owned()]));
    }

    #[test]
    fn every_default_shortcut_parses_and_none_collide() {
        use crate::shortcuts::{ALL, Keys};
        let keys = Keys::defaults();
        for c in ALL {
            assert!(!keys.label(c).is_empty(), "{c:?} ({}) didn't parse", c.default_spec());
            for o in ALL.into_iter().filter(|o| *o != c) {
                assert_ne!(c.default_spec(), o.default_spec(), "{c:?} and {o:?}");
            }
        }
        // The recorder reads a key back as the same canonical chord a binding is written in.
        assert_eq!(crate::shortcuts::chord_of(&cmd('n')).map(|(s, _)| s), Some("mod+n".to_owned()));
    }

    /// Settings › Keyboard shortcuts: record ⌘K for New thread, then ⌘K starts one and ⌘N no
    /// longer does. A taken chord is refused; Reset puts the default back.
    #[test]
    fn recording_a_shortcut_rebinds_it() {
        use crate::shortcuts::{self, Cmd};
        let mut app = new_app();
        crate::app::scenes::demo(&mut app);
        let mut rt = headless(app);
        rt.update(|s, _| {
            s.open_settings();
            s.show_settings_page(crate::settings::Page::Shortcuts);
            shortcuts::begin_recording(s, Cmd::NewThread);
        });
        rt.frame(1.0 / 60.0);
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("Press keys…")));
        // ⌘, belongs to Settings: refused, still recording.
        rt.key(cmd(','));
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.recording, Some(Cmd::NewThread));
        assert!(rt.state.key_error.is_some());
        rt.key(cmd('k'));
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.recording, None);
        assert_eq!(rt.state.keys.spec(Cmd::NewThread), "mod+k");
        rt.update(|s, _| s.close_settings());
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.selected, Some(100));
        rt.key(cmd('n'));
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.selected, Some(100), "⌘N is unbound now");
        rt.key(cmd('k'));
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.selected, None, "⌘K starts a new thread");
        rt.update(|s, _| shortcuts::reset(s, Cmd::NewThread));
        assert!(rt.state.keys.is_default(Cmd::NewThread));
    }

    /// A stand-in `claude` speaking stream-json on stdin: it reads our `initialize` and the
    /// prompt, answers at once, or after 10 s when the prompt says "slow". When it says
    /// "approve" it asks to run a Bash command, waits for the answer, and reports it as the
    /// tool's output ("allowed" or "denied"). Each test gets its own copy (`name`), so tests
    /// running in parallel don't rewrite a script another is running.
    fn fake_claude(name: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = test_dir().join(format!("fake-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join(name);
        std::fs::write(&bin, concat!(
            "#!/bin/sh\n",
            "read -r init\n",
            "read -r msg\n",
            "case \"$msg\" in *slow*) sleep 10 ;; esac\n",
            "echo '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s1\",\"model\":\"fake\"}'\n",
            "case \"$msg\" in *approve*)\n",
            "  echo '{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"Bash\",\"input\":{\"command\":\"echo hi\"}}]}}'\n",
            "  echo '{\"type\":\"control_request\",\"request_id\":\"r1\",\"request\":{\"subtype\":\"can_use_tool\",\"tool_name\":\"Bash\",\"input\":{\"command\":\"echo hi\"},\"tool_use_id\":\"t1\",\"permission_suggestions\":[]}}'\n",
            "  mode=\n",
            "  while read -r answer; do\n",
            "    case \"$answer\" in *set_permission_mode*bypassPermissions*) mode='mode-bypassPermissions ' ;; *'\"request_id\":\"r1\"'*) break ;; esac\n",
            "  done\n",
            "  out=denied\n",
            "  case \"$answer\" in *'\"behavior\":\"allow\"'*) out=\"${mode}allowed\" ;; esac\n",
            "  echo '{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"t1\",\"content\":\"'\"$out\"'\",\"is_error\":false}]}}'\n",
            "  ;;\n",
            "esac\n",
            "echo '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"session_id\":\"s1\"}'\n",
        )).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        bin
    }

    #[track_caller]
    fn pump_until(rt: &mut Runtime<App>, secs: u64, done: impl Fn(&App) -> bool) {
        let start = Instant::now();
        while !done(&rt.state) && start.elapsed() < Duration::from_secs(secs) {
            rt.update(|s, tx| { s.drain(tx); });
            rt.frame(1.0 / 60.0);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(done(&rt.state), "timed out");
    }

    fn user_messages(s: &App) -> Vec<String> {
        s.thread().unwrap().items.iter().filter_map(|i| match &i.kind { Kind::User(m) => Some(m.clone()), _ => None }).collect()
    }

    /// Return while a turn runs queues; the queue goes in order when the turn ends. Steer stops
    /// the slow turn and sends at once; Esc stops and the queue waits.
    #[test]
    fn follow_ups_queue_and_steer() {
        let mut app = new_app();
        app.installed = vec![(Harness::Claude, Some(fake_claude("claude-queue")))];
        app.choose_harness(Harness::Claude);
        let mut rt = headless(app);
        let typed = |rt: &mut Runtime<App>, text: &str, k: KeyEvent| {
            rt.key(key(Key::Text(text.into())));
            rt.frame(1.0 / 60.0);
            rt.key(k);
            rt.frame(1.0 / 60.0);
        };

        // Queue two behind a slow turn, then steer the first in.
        typed(&mut rt, "slow one", key(Key::Enter));
        let id = rt.state.selected.unwrap();
        assert!(rt.state.is_running(id));
        typed(&mut rt, "two", key(Key::Enter));
        typed(&mut rt, "three", key(Key::Enter));
        assert_eq!(rt.state.thread().unwrap().queue.iter().map(|q| q.text.as_str()).collect::<Vec<_>>(), ["two", "three"]);
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("two")), "the queue shows above the prompt");
        let qid = rt.state.thread().unwrap().queue[0].id;
        rt.update(|s, tx| s.send_queued_now(id, qid, tx));
        // "two" starts once the slow turn is killed, then "three" after it.
        pump_until(&mut rt, 30, |s| s.thread().unwrap().queue.is_empty() && !s.is_running(id));
        let slow = rt.state.thread().unwrap().items.iter().find_map(|i| match i.kind { Kind::TurnEnd { stopped, .. } => Some(stopped), _ => None });
        assert_eq!(slow, Some(true), "steering killed the slow turn rather than waiting it out");
        assert_eq!(user_messages(&rt.state), ["slow one", "two", "three"]);

        // Esc stops a slow turn; what was queued behind it waits.
        typed(&mut rt, "slow again", key(Key::Enter));
        typed(&mut rt, "after", key(Key::Enter));
        rt.key(key(Key::Escape));
        rt.frame(1.0 / 60.0);
        // Generous for a slow CI machine; what matters is that the turn was killed, not that
        // its 10-second sleep ran out.
        pump_until(&mut rt, 30, |s| !s.is_running(id));
        rt.update(|s, tx| { s.drain(tx); });
        assert!(!rt.state.is_running(id));
        let stopped = rt.state.thread().unwrap().items.iter().rev().find_map(|i| match i.kind { Kind::TurnEnd { stopped, .. } => Some(stopped), _ => None });
        assert_eq!(stopped, Some(true), "Esc killed the turn");
        assert_eq!(rt.state.thread().unwrap().queue.len(), 1, "the queue waits after a stop");
    }

    /// Settings › New threads: the defaults show on the new-thread screen at once, ⌘N goes
    /// back to them after the composer changed, switching agent takes that agent's, and they
    /// survive a relaunch. Reset clears them.
    #[test]
    fn new_threads_start_from_the_defaults() {
        let mut app = new_app();
        let bin = fake_claude("claude-defaults");
        app.installed = vec![(Harness::Claude, Some(bin.clone())), (Harness::Codex, Some(bin))];
        app.choose_harness(Harness::Claude);
        let opus = app.models(Harness::Claude).iter().find(|m| m.value != crate::harness::models::DEFAULT).unwrap().clone();
        app.set_default_model(Harness::Claude, &opus.label);
        app.set_default_effort(Harness::Claude, "high");
        app.set_default_access(Access::ReadOnly);
        app.set_default_worktree(true);
        assert_eq!((app.model.as_str(), app.effort.as_str(), app.access), (opus.value.as_str(), "high", Access::ReadOnly), "the new-thread screen takes them at once");
        assert!(app.worktree_mode);
        app.toggle_worktree_mode();
        assert!(!app.worktree_mode && app.defaults.worktree, "the composer's chip is for this thread only");

        app.model = crate::harness::models::DEFAULT.into();
        app.effort = "low".into();
        app.access = Access::Full;
        app.new_thread();
        assert_eq!((app.model.as_str(), app.effort.as_str(), app.access), (opus.value.as_str(), "high", Access::ReadOnly), "⌘N starts from the defaults");
        assert!(app.worktree_mode, "in a new worktree again");

        app.set_default_harness(Harness::Codex);
        assert_eq!(app.harness, Harness::Codex);
        assert_eq!(app.model, crate::harness::models::DEFAULT, "Codex has no default model of its own");
        app.choose_harness(Harness::Claude);
        assert_eq!((app.model.as_str(), app.effort.as_str()), (opus.value.as_str(), "high"), "switching back takes Claude's");

        let prefs = std::mem::replace(&mut app.prefs, rune::prefs::Prefs::memory());
        let mut again = App::new(Some(std::env::current_dir().unwrap()), prefs);
        assert_eq!(again.defaults.harness, Some(Harness::Codex));
        assert_eq!(again.default_model(Harness::Claude), opus.value);
        assert_eq!(again.default_effort(Harness::Claude), "high");
        assert_eq!(again.defaults.access, Access::ReadOnly);
        assert!(again.defaults.worktree && again.worktree_mode, "the launch screen starts in a worktree too");

        assert!(again.defaults_changed());
        again.reset_defaults();
        assert!(!again.defaults_changed());
        assert_eq!(again.default_model(Harness::Claude), crate::harness::models::DEFAULT);
        assert!(!again.worktree_mode);

        // The chip's old remembered choice becomes the default.
        let mut old = rune::prefs::Prefs::memory();
        let _ = old.set("composer.worktree", &true);
        assert!(App::new(Some(std::env::current_dir().unwrap()), old).worktree_mode);
    }

    /// Settings › Agents: messages from your other sessions are on by default, and the switch
    /// turns them off and saves that.
    #[test]
    fn peer_messages_switch() {
        let mut app = new_app();
        assert!(app.defaults.peer_messages, "on by default");
        app.open_settings();
        app.show_settings_page(crate::settings::Page::Agents);
        let mut rt = headless(app);
        rt.frame(0.5);
        let (x, y) = rt.find("peer-messages").expect("the switch shows on Agents");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0);
        assert!(!rt.state.defaults.peer_messages);
        assert!(!crate::settings::Prefs::load(&rt.state.prefs).peer_messages, "saved");
    }

    /// Settings › Chat: with ⌘↩ sending, ↩ is a newline; with follow-ups steering, sending into
    /// a running turn steers and ⇧⌘↩ queues instead.
    #[test]
    fn send_key_and_follow_up_settings() {
        use crate::settings::{FollowUp, SendKey};
        let mut app = new_app();
        app.installed = vec![(Harness::Claude, Some(fake_claude("claude-chat")))];
        app.choose_harness(Harness::Claude);
        app.set_send_key(SendKey::ModReturn);
        app.set_follow_up(FollowUp::Steer);
        let mut rt = headless(app);
        let enter = |cmd: bool, shift: bool| KeyEvent { key: Key::Enter, mods: Mods { cmd, shift, ..Mods::default() } };
        let typed = |rt: &mut Runtime<App>, text: &str, k: KeyEvent| {
            rt.key(key(Key::Text(text.into())));
            rt.frame(1.0 / 60.0);
            rt.key(k);
            rt.frame(1.0 / 60.0);
        };

        typed(&mut rt, "first", enter(false, false));
        assert_eq!(rt.state.selected, None, "Return didn't send");
        assert_eq!(rt.state.draft.text, "first\n");
        typed(&mut rt, "slow", enter(true, false));
        let id = rt.state.selected.expect("⌘↩ sent");
        assert!(rt.state.is_running(id));

        // Sending into the running turn steers: it stops, and "now" goes straight away.
        typed(&mut rt, "now", enter(true, false));
        pump_until(&mut rt, 30, |s| s.thread().unwrap().queue.is_empty() && !s.is_running(id));
        let slow = rt.state.thread().unwrap().items.iter().find_map(|i| match i.kind { Kind::TurnEnd { stopped, .. } => Some(stopped), _ => None });
        assert_eq!(slow, Some(true), "steering killed the slow turn rather than waiting it out");
        assert_eq!(user_messages(&rt.state), ["first\nslow", "now"]);

        // ⇧⌘↩ does the other: it queues.
        typed(&mut rt, "slow again", enter(true, false));
        assert!(rt.state.is_running(id));
        typed(&mut rt, "later", enter(true, true));
        assert_eq!(rt.state.thread().unwrap().queue.iter().map(|q| q.text.as_str()).collect::<Vec<_>>(), ["later"]);
        rt.key(key(Key::Escape));
        rt.frame(1.0 / 60.0);
        pump_until(&mut rt, 30, |s| !s.is_running(id));
    }

    /// An image dropped on the window shows in the composer and goes to the CLI as an image
    /// block; another file goes by its path, named in the prompt for the agent to read.
    #[test]
    fn dropped_images_attach_and_send() {
        let mut app = new_app();
        app.installed = vec![(Harness::Claude, Some(fake_claude("claude-drop")))];
        app.choose_harness(Harness::Claude);
        let mut rt = headless(app);
        let dir = test_dir().join(format!("drop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("shot.png");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\nnot really").unwrap();
        let notes = dir.join("notes.txt");
        std::fs::write(&notes, "todo").unwrap();
        rt.drop_files(vec![notes.clone(), png.clone()], Some((500.0, 300.0)));
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.attachments.len(), 2);
        assert!(rt.state.composer_error.is_none());
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("shot.png")), "the attachment shows");
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("notes.txt")));
        // Return with no text still sends the image.
        rt.key(key(Key::Enter));
        rt.frame(1.0 / 60.0);
        assert!(rt.state.attachments.is_empty());
        let t = rt.state.thread().expect("a thread started");
        assert!(matches!(&t.items[0].kind, Kind::User(m) if m.contains("Attached: shot.png, notes.txt")));
        assert_eq!(crate::app::with_files("look", std::slice::from_ref(&notes)), format!("look\n\nAttached files:\n- {}", notes.display()));
        let turn = crate::harness::Turn { harness: Harness::Claude, prompt: "look".into(), model: "Default".into(), effort: crate::harness::DEFAULT_EFFORT.into(), access: Access::Edits, resume: None, cwd: ".".into(), images: vec![png], peer_messages: true };
        let msg: serde_json::Value = serde_json::from_str(Harness::Claude.stdin_lines(&turn).last().unwrap()).unwrap();
        assert_eq!(msg["message"]["content"][0]["text"], "look");
        assert_eq!(msg["message"]["content"][1]["source"]["media_type"], "image/png");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Switching the access picker to Full access while Claude Code waits on an approval
    /// tells the running CLI (`set_permission_mode`) and allows the waiting tool.
    #[test]
    fn access_change_reaches_the_running_turn() {
        let mut app = new_app();
        app.installed = vec![(Harness::Claude, Some(fake_claude("claude-mode")))];
        app.choose_harness(Harness::Claude);
        app.access = Access::Ask;
        let mut rt = headless(app);
        rt.key(key(Key::Text("please approve".into())));
        rt.frame(1.0 / 60.0);
        rt.key(key(Key::Enter));
        rt.frame(1.0 / 60.0);
        let id = rt.state.selected.unwrap();
        pump_until(&mut rt, 30, |s| !s.thread().unwrap().asks.is_empty());
        rt.update(|s, _| s.set_access(Access::Full));
        assert!(rt.state.thread().unwrap().asks.is_empty(), "the waiting tool was allowed");
        pump_until(&mut rt, 30, |s| !s.is_running(id));
        assert_eq!(tool_outputs(&rt.state), [(ToolState::Ok, "mode-bypassPermissions allowed".to_owned())]);
    }

    /// Settings fills the main pane in place of the thread and composer; picking a thread
    /// closes it.
    #[test]
    fn settings_takes_the_main_pane() {
        let mut app = new_app();
        crate::app::scenes::demo(&mut app);
        let mut rt = headless(app);
        rt.key(cmd(','));
        rt.frame(1.0 / 60.0);
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("Keyboard shortcuts")));
        // "Default" (let the CLI choose) isn't counted as a model, so a CLI that isn't
        // installed doesn't claim one.
        assert!(!rt.elements().iter().any(|e| e.text.as_deref() == Some("1 model")));
        // Picking a theme repaints with it and saves it.
        rt.update(|s, _| s.set_theme(crate::ui::theme::Theme::Gruvbox));
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.prefs.get::<String>("theme").as_deref(), Some("Gruvbox"));
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        assert!(!rt.elements().iter().any(|e| e.key.as_deref() == Some("composer")), "the composer is hidden");
        // The rail switches pages; New threads' access menu sets the default.
        let click = |rt: &mut Runtime<App>, key: &str| {
            let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} on screen"));
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            for _ in 0..20 { rt.frame(1.0 / 60.0); }
        };
        click(&mut rt, "settings-page-new-threads");
        assert_eq!(rt.state.settings_page, crate::settings::Page::NewThreads);
        click(&mut rt, "default-access-trigger");
        assert_eq!(rt.state.open_menu, Some("default-access"));
        // Read only, Ask first, Edit files, Full access.
        click(&mut rt, "option3");
        assert_eq!(rt.state.defaults.access, Access::Full);
        assert_eq!(rt.state.prefs.get::<String>("new-thread.access").as_deref(), Some("Full access"));
        rt.update(|s, _| s.select(102));
        rt.frame(1.0 / 60.0);
        assert!(!rt.state.settings);
    }

    fn tool_outputs(s: &App) -> Vec<(ToolState, String)> {
        s.thread().unwrap().items.iter().filter_map(|i| match &i.kind { Kind::Tool { state, output, .. } => Some((*state, output.clone())), _ => None }).collect()
    }

    /// Claude Code asks before a tool runs: the tool's row shows Allow and Deny while the CLI
    /// waits, ⌘Y allows, Deny refuses, and each answer reaches the CLI.
    #[test]
    fn approvals_reach_the_cli() {
        let mut app = new_app();
        app.installed = vec![(Harness::Claude, Some(fake_claude("claude-approve")))];
        app.choose_harness(Harness::Claude);
        let mut rt = headless(app);
        let send = |rt: &mut Runtime<App>, text: &str| {
            rt.key(key(Key::Text(text.into())));
            rt.frame(1.0 / 60.0);
            rt.key(key(Key::Enter));
            rt.frame(1.0 / 60.0);
        };

        send(&mut rt, "please approve");
        let id = rt.state.selected.unwrap();
        pump_until(&mut rt, 30, |s| !s.thread().unwrap().asks.is_empty());
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("Allow")), "the approval shows");
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("echo hi")), "with the command");
        assert!(rt.state.is_running(id), "the CLI waits for the answer");
        rt.key(cmd('y'));
        rt.frame(1.0 / 60.0);
        assert!(rt.state.thread().unwrap().asks.is_empty());
        pump_until(&mut rt, 30, |s| !s.is_running(id));
        assert_eq!(tool_outputs(&rt.state), [(ToolState::Ok, "allowed".to_owned())], "one row for the call, and the CLI was allowed");

        send(&mut rt, "approve this one too");
        pump_until(&mut rt, 30, |s| !s.thread().unwrap().asks.is_empty());
        let item = rt.state.thread().unwrap().asks[0].item;
        rt.update(|s, _| s.answer(id, item, crate::harness::Decision::Deny));
        pump_until(&mut rt, 30, |s| !s.is_running(id));
        assert_eq!(tool_outputs(&rt.state)[1].1, "denied");
    }

    #[test]
    fn claude_takes_the_prompt_on_stdin_and_asks_over_stdio() {
        let turn = |access| crate::harness::Turn { harness: Harness::Claude, prompt: "hi there".into(), model: "Default".into(), effort: crate::harness::DEFAULT_EFFORT.into(), access, resume: None, cwd: ".".into(), images: Vec::new(), peer_messages: true };
        let claude_args = |access| -> Vec<String> {
            Harness::Claude.command(std::path::Path::new("x"), &turn(access)).get_args().map(|a| a.to_string_lossy().into_owned()).collect()
        };
        let a = claude_args(Access::Ask);
        assert!(a.windows(2).any(|w| w == ["--permission-prompt-tool", "stdio"]), "{a:?}");
        assert!(a.windows(2).any(|w| w == ["--input-format", "stream-json"]), "{a:?}");
        assert!(a.windows(2).any(|w| w == ["--permission-mode", "default"]), "{a:?}");
        assert!(!a.iter().any(|x| x.contains("hi there")), "the prompt isn't an argument");
        assert!(claude_args(Access::Edits).windows(2).any(|w| w == ["--permission-mode", "acceptEdits"]));
        assert!(a.windows(2).any(|w| w == ["--settings", r#"{"crossSessionInbound":"accept"}"#]), "{a:?}");
        let off = crate::harness::Turn { peer_messages: false, ..turn(Access::Ask) };
        let a: Vec<String> = Harness::Claude.command(std::path::Path::new("x"), &off).get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(a.windows(2).any(|w| w == ["--settings", r#"{"crossSessionInbound":"refuse"}"#]), "{a:?}");
        let lines = Harness::Claude.stdin_lines(&turn(Access::Ask));
        let msg: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
        assert_eq!((msg["type"].as_str(), msg["message"]["content"].as_str()), (Some("user"), Some("hi there")));
        // Codex can't ask, so "Ask first" is read only there.
        let codex = crate::harness::Turn { harness: Harness::Codex, ..turn(Access::Ask) };
        let c: Vec<String> = Harness::Codex.command(std::path::Path::new("x"), &codex).get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(c.windows(2).any(|w| w == ["--sandbox", "read-only"]), "{c:?}");
    }
}
