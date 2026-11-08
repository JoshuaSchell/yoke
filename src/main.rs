//! Rune Code: a small T3 Code-style app for prompting coding agents, to see how Rune does.
//!
//! `cargo run -- [project folder]` opens the window on that folder (default: the current one).
//! Headless: `-- --snap out.png --scene demo --size 1200x800` renders a scene without a window.

mod app;
mod attention;
#[cfg(test)]
mod bench;
mod harness;
mod history;
mod picker;
mod settings;
mod shortcuts;
mod ui;
mod usage;

use std::path::PathBuf;
use std::time::Duration;

use rune::prefs::Prefs;
use rune::{AppSpec, Chrome, MenuItem, Tx};

use app::App;

/// The folder to open: one named on the command line, else the working directory when run from
/// a shell, else the folder open last time, else home. A launch from the Dock, Finder or a
/// desktop launcher starts in `/` or home, which is never the project you meant.
fn init() -> App {
    let mut prefs = Prefs::open(app::APP_ID);
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    let arg = rune::extra_args().iter().find(|a| !a.starts_with('-')).map(PathBuf::from);
    let cwd = std::env::current_dir().ok().filter(|p| p.parent().is_some() && Some(p) != home.as_ref());
    let last = prefs.get::<String>(app::LAST_PROJECT_KEY).map(PathBuf::from).filter(|p| p.is_dir());
    let project = arg.or(cwd).or(last).or(home).unwrap_or_else(|| PathBuf::from("."));
    let project = project.canonicalize().unwrap_or(project);
    let _ = prefs.set(app::LAST_PROJECT_KEY, &project.to_string_lossy().into_owned());
    std::thread::spawn(app::purge_old_pastes);
    App::new(project, prefs)
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
    tx.wake_after(Duration::from_secs(60));
    changed
}

/// Named starting states for headless snapshots (`--scene`).
type Scene = (&'static str, fn(&mut App));

const SCENES: &[Scene] = &[
    ("demo", app::scenes::demo),
    ("long", app::scenes::long),
    ("empty", app::scenes::empty),
    ("picker", app::scenes::picker),
    ("effort", app::scenes::effort),
    ("settings", app::scenes::settings),
    ("settings-new-threads", app::scenes::settings_new_threads),
    ("settings-chat", app::scenes::settings_chat),
    ("usage", app::scenes::usage),
    ("usage-popover", app::scenes::usage_popover),
    ("queue", app::scenes::queue),
    ("approval", app::scenes::approval),
    ("attach", app::scenes::attach),
    ("collapsed", app::scenes::collapsed),
];

fn main() {
    attention::install();
    rune::prewarm_gpu();
    // Inside a bundle macOS already shows Rune Code.app's icon.
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
        ..AppSpec::new("Rune Code", (1180.0, 780.0), init, ui::view)
    };
    // Traffic lights centred 24 points down, as in other macOS apps; the toolbar's contents
    // and the sidebar toggle line up with them (`ui::LIGHTS_MID`).
    let chrome = Chrome { full_size_content: true, traffic_lights: Some((16.0, 17.0)), drag_regions: true };
    // Rune Code › Settings…, on the Settings shortcut as saved at launch (a rebinding reaches the
    // menu at the next launch; the shortcut table answers it straight away). `ui::mac` moves
    // it below About.
    let keys = shortcuts::Keys::load(&Prefs::open(app::APP_ID));
    let mut settings = MenuItem::new("Settings…", |s: &mut App, _: &mut Tx| s.open_settings()).id("settings");
    if let Ok(k) = keys.spec(shortcuts::Cmd::Settings).parse() {
        settings = settings.key_equivalent(k);
    }
    let mut open = MenuItem::new("Open Folder…", |s: &mut App, tx: &mut Tx| s.choose_folder(tx)).id("open-folder").menu("File");
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
        App::new(std::env::current_dir().unwrap(), rune::prefs::Prefs::memory())
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
        assert!(c.contains("rune-code"), "answer: {c:?}");
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
        let turn = crate::harness::Turn { harness: h, prompt: "hi".into(), model: "Default".into(), effort: effort.into(), access: Access::Edits, resume: None, cwd: ".".into(), images: Vec::new() };
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

    /// Saved sessions untouched for a week land settled, unless settled or kept by hand.
    #[test]
    fn old_sessions_land_settled() {
        use std::time::SystemTime;
        use crate::app::Lane;
        let mut app = new_app();
        let _ = app.prefs.set("settled", &std::collections::HashMap::from([("kept".to_owned(), false), ("done".to_owned(), true)]));
        let app = App::new(std::env::current_dir().unwrap(), app.prefs);
        let mut app = app;
        let week = Duration::from_secs(8 * 24 * 3600);
        let s = |id: &str, ago: Duration| crate::history::Session { id: id.into(), title: id.into(), modified: SystemTime::now() - ago, path: "/nonexistent".into() };
        app.land_sessions(vec![s("old", week), s("new", Duration::from_secs(60)), s("kept", week), s("done", Duration::ZERO)]);
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

    /// The composer's gauge opens the plan's meters; "Open usage" opens the page, which
    /// ⇧⌘U and Esc close, as does picking a thread.
    #[test]
    fn usage_gauge_popover_and_page() {
        let mut app = new_app();
        crate::app::scenes::demo(&mut app);
        let mut rt = headless(app);
        let (x, y) = rt.find("usage-chip").expect("the gauge shows for a Claude thread");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.2);
        assert_eq!(rt.state.open_menu, Some("usage"));
        assert!(rt.find("meter-session").is_some() && rt.find("meter-weekly_scoped:Fable").is_some());
        // The popover settles beside its trigger over the next frames.
        for i in 0..20 { rt.frame(0.2 + i as f64 / 60.0); }
        let (bx, by) = rt.find("usage-open-page").unwrap();
        rt.pointer_down(bx, by);
        rt.pointer_up(bx, by);
        rt.frame(0.4);
        assert!(rt.state.usage_open && rt.state.open_menu.is_none());
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
        // Codex has no Claude plan to meter.
        rt.update(|s, _| s.select(102));
        rt.frame(0.8);
        assert!(rt.find("usage-chip").is_none());
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
        assert!(rt.find("usage-chip").is_some(), "the gauge shows on the new-thread screen");
        rt.key(shift_cmd('u'));
        rt.frame(0.1);
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("Cost by day")));
    }

    /// ⌘O asks for a folder; picking one makes it the project, empties the sidebar of the old
    /// folder's threads and saves it for the next launch. Cancelling changes nothing.
    #[test]
    fn command_o_opens_another_folder() {
        use rune::FileDialogKind;
        let mut rt = headless(new_app());
        rt.state.threads.push(crate::app::Thread::new(99, "old".into(), Harness::Claude, "Default".into()));
        rt.key(cmd('o'));
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.file_dialog().map(|(kind, o)| (kind, o.title.clone())), Some((FileDialogKind::Folder, "Open Folder".into())));
        rt.resolve_file_dialog(None).unwrap();
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.threads.len(), 1, "cancelling keeps the project");

        let dir = std::env::temp_dir().canonicalize().unwrap();
        rt.key(cmd('o'));
        rt.frame(1.0 / 60.0);
        rt.resolve_file_dialog(Some(dir.clone())).unwrap();
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.project.path, dir);
        assert!(rt.state.threads.iter().all(|t| t.id != 99), "the old folder's threads go");
        assert_eq!(rt.state.prefs.get::<String>(crate::app::LAST_PROJECT_KEY), Some(dir.to_string_lossy().into_owned()));
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
        let started = Instant::now();
        rt.update(|s, tx| s.send_queued_now(id, qid, tx));
        // "two" starts once the slow turn is killed, then "three" after it.
        pump_until(&mut rt, 8, |s| s.thread().unwrap().queue.is_empty() && !s.is_running(id));
        assert!(started.elapsed() < Duration::from_secs(8), "steering didn't wait out the slow turn");
        assert_eq!(user_messages(&rt.state), ["slow one", "two", "three"]);

        // Esc stops a slow turn; what was queued behind it waits.
        typed(&mut rt, "slow again", key(Key::Enter));
        typed(&mut rt, "after", key(Key::Enter));
        rt.key(key(Key::Escape));
        rt.frame(1.0 / 60.0);
        pump_until(&mut rt, 8, |s| !s.is_running(id));
        rt.update(|s, tx| { s.drain(tx); });
        assert!(!rt.state.is_running(id));
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
        assert_eq!((app.model.as_str(), app.effort.as_str(), app.access), (opus.value.as_str(), "high", Access::ReadOnly), "the new-thread screen takes them at once");

        app.model = crate::harness::models::DEFAULT.into();
        app.effort = "low".into();
        app.access = Access::Full;
        app.new_thread();
        assert_eq!((app.model.as_str(), app.effort.as_str(), app.access), (opus.value.as_str(), "high", Access::ReadOnly), "⌘N starts from the defaults");

        app.set_default_harness(Harness::Codex);
        assert_eq!(app.harness, Harness::Codex);
        assert_eq!(app.model, crate::harness::models::DEFAULT, "Codex has no default model of its own");
        app.choose_harness(Harness::Claude);
        assert_eq!((app.model.as_str(), app.effort.as_str()), (opus.value.as_str(), "high"), "switching back takes Claude's");

        let prefs = std::mem::replace(&mut app.prefs, rune::prefs::Prefs::memory());
        let mut again = App::new(std::env::current_dir().unwrap(), prefs);
        assert_eq!(again.defaults.harness, Some(Harness::Codex));
        assert_eq!(again.default_model(Harness::Claude), opus.value);
        assert_eq!(again.default_effort(Harness::Claude), "high");
        assert_eq!(again.defaults.access, Access::ReadOnly);

        assert!(again.defaults_changed());
        again.reset_defaults();
        assert!(!again.defaults_changed());
        assert_eq!(again.default_model(Harness::Claude), crate::harness::models::DEFAULT);
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
        let started = Instant::now();
        typed(&mut rt, "now", enter(true, false));
        pump_until(&mut rt, 8, |s| s.thread().unwrap().queue.is_empty() && !s.is_running(id));
        assert!(started.elapsed() < Duration::from_secs(8), "steering didn't wait out the slow turn");
        assert_eq!(user_messages(&rt.state), ["first\nslow", "now"]);

        // ⇧⌘↩ does the other: it queues.
        typed(&mut rt, "slow again", enter(true, false));
        assert!(rt.state.is_running(id));
        typed(&mut rt, "later", enter(true, true));
        assert_eq!(rt.state.thread().unwrap().queue.iter().map(|q| q.text.as_str()).collect::<Vec<_>>(), ["later"]);
        rt.key(key(Key::Escape));
        rt.frame(1.0 / 60.0);
        pump_until(&mut rt, 8, |s| !s.is_running(id));
    }

    /// An image dropped on the window shows in the composer and goes to the CLI as an image
    /// block; a file that isn't an image is refused with a note.
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
        rt.drop_files(vec![dir.join("notes.txt")], Some((500.0, 300.0)));
        rt.frame(1.0 / 60.0);
        assert!(rt.state.attachments.is_empty() && rt.state.composer_error.is_some());
        rt.drop_files(vec![png.clone()], Some((500.0, 300.0)));
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.attachments.len(), 1);
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("shot.png")), "the attachment shows");
        // Return with no text still sends the image.
        rt.key(key(Key::Enter));
        rt.frame(1.0 / 60.0);
        assert!(rt.state.attachments.is_empty());
        let t = rt.state.thread().expect("a thread started");
        assert!(matches!(&t.items[0].kind, Kind::User(m) if m.contains("Attached: shot.png")));
        let turn = crate::harness::Turn { harness: Harness::Claude, prompt: "look".into(), model: "Default".into(), effort: crate::harness::DEFAULT_EFFORT.into(), access: Access::Edits, resume: None, cwd: ".".into(), images: vec![png] };
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
        pump_until(&mut rt, 8, |s| !s.thread().unwrap().asks.is_empty());
        rt.update(|s, _| s.set_access(Access::Full));
        assert!(rt.state.thread().unwrap().asks.is_empty(), "the waiting tool was allowed");
        pump_until(&mut rt, 8, |s| !s.is_running(id));
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
        pump_until(&mut rt, 8, |s| !s.thread().unwrap().asks.is_empty());
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("Allow")), "the approval shows");
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("echo hi")), "with the command");
        assert!(rt.state.is_running(id), "the CLI waits for the answer");
        rt.key(cmd('y'));
        rt.frame(1.0 / 60.0);
        assert!(rt.state.thread().unwrap().asks.is_empty());
        pump_until(&mut rt, 8, |s| !s.is_running(id));
        assert_eq!(tool_outputs(&rt.state), [(ToolState::Ok, "allowed".to_owned())], "one row for the call, and the CLI was allowed");

        send(&mut rt, "approve this one too");
        pump_until(&mut rt, 8, |s| !s.thread().unwrap().asks.is_empty());
        let item = rt.state.thread().unwrap().asks[0].item;
        rt.update(|s, _| s.answer(id, item, crate::harness::Decision::Deny));
        pump_until(&mut rt, 8, |s| !s.is_running(id));
        assert_eq!(tool_outputs(&rt.state)[1].1, "denied");
    }

    #[test]
    fn claude_takes_the_prompt_on_stdin_and_asks_over_stdio() {
        let turn = |access| crate::harness::Turn { harness: Harness::Claude, prompt: "hi there".into(), model: "Default".into(), effort: crate::harness::DEFAULT_EFFORT.into(), access, resume: None, cwd: ".".into(), images: Vec::new() };
        let claude_args = |access| -> Vec<String> {
            Harness::Claude.command(std::path::Path::new("x"), &turn(access)).get_args().map(|a| a.to_string_lossy().into_owned()).collect()
        };
        let a = claude_args(Access::Ask);
        assert!(a.windows(2).any(|w| w == ["--permission-prompt-tool", "stdio"]), "{a:?}");
        assert!(a.windows(2).any(|w| w == ["--input-format", "stream-json"]), "{a:?}");
        assert!(a.windows(2).any(|w| w == ["--permission-mode", "default"]), "{a:?}");
        assert!(!a.iter().any(|x| x.contains("hi there")), "the prompt isn't an argument");
        assert!(claude_args(Access::Edits).windows(2).any(|w| w == ["--permission-mode", "acceptEdits"]));
        let lines = Harness::Claude.stdin_lines(&turn(Access::Ask));
        let msg: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
        assert_eq!((msg["type"].as_str(), msg["message"]["content"].as_str()), (Some("user"), Some("hi there")));
        // Codex can't ask, so "Ask first" is read only there.
        let codex = crate::harness::Turn { harness: Harness::Codex, ..turn(Access::Ask) };
        let c: Vec<String> = Harness::Codex.command(std::path::Path::new("x"), &codex).get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(c.windows(2).any(|w| w == ["--sandbox", "read-only"]), "{c:?}");
    }
}
