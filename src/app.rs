//! App state: threads, their transcripts, the composer, and applying harness events.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use rune::prefs::Prefs;
use rune::{Image, ImageCache, TextEdit, Tx, Waker};
use rune_kit::shell::PaneState;

use crate::harness::limits::{self, Limits};
use crate::harness::models::{self, ModelOpt};
use crate::harness::{self, Access, AskRequest, Decision, Event, Harness, Running, Turn};
use crate::history;
use crate::store;
use crate::worktree::{self, Worktree};
use crate::shortcuts::{Cmd, Keys};
use crate::ui::theme::Theme;
use crate::usage;

pub type ThreadId = u64;

/// The prefs store's app id (`~/Library/Application Support/dev.yoke/prefs.json`).
pub const APP_ID: &str = "dev.yoke";
/// The app's id before it was renamed Yoke (it was Rune Code).
const OLD_APP_ID: &str = "dev.rune-code";

/// The first launch after the rename copies the old settings and kept threads over, so the
/// projects, defaults and Codex threads are still there. The old folder is left alone.
pub fn migrate_from_rune_code() {
    let (old, new) = (Prefs::open(OLD_APP_ID), Prefs::open(APP_ID));
    let (Some(old), Some(new)) = (old.path(), new.path()) else { return };
    if new.exists() || !old.exists() { return; }
    let (Some(old_dir), Some(new_dir)) = (old.parent(), new.parent()) else { return };
    if std::fs::create_dir_all(new_dir).is_err() { return; }
    let _ = std::fs::copy(old, new);
    if let Ok(files) = std::fs::read_dir(old_dir.join("threads")) {
        let _ = std::fs::create_dir_all(new_dir.join("threads"));
        for f in files.flatten() {
            let _ = std::fs::copy(f.path(), new_dir.join("threads").join(f.file_name()));
        }
    }
}

/// A thread nobody has touched for this long settles on its own (T3 Code's default).
pub const AUTO_SETTLE: Duration = Duration::from_secs(7 * 24 * 3600);
/// How many settled threads the shelf lists before "Show more".
pub const SETTLED_PAGE: usize = 30;
const SETTLED_KEY: &str = "settled";
const SETTLED_OPEN_KEY: &str = "sidebar.settled-open";
const WORKING_OPEN_KEY: &str = "sidebar.working-open";
/// The folder last open, which a launch from the Dock or a launcher (no folder argument, no
/// useful working directory) opens again.
pub const LAST_PROJECT_KEY: &str = "project.last";
/// The sidebar's projects, as folder paths in the order added, and the ones shut.
pub const PROJECTS_KEY: &str = "projects";
/// The plan limits last read, so the gauge has numbers at launch.
const LIMITS_KEY: &str = "usage.limits";
/// Each Claude model's context window as Claude Code last reported it, by model id, so a
/// thread's gauge has the right size before its first reply.
const CONTEXT_WINDOWS_KEY: &str = "usage.context-windows";
/// LiteLLM's Claude prices (`usage::trim_prices`) and when they were fetched (epoch seconds).
const PRICES_KEY: &str = "usage.prices";
const PRICES_AT_KEY: &str = "usage.prices-fetched";
const USAGE_DAYS_KEY: &str = "usage.days";
/// How often plan limits are read again while the app is open (turns also read them).
pub const LIMITS_EVERY: Duration = Duration::from_secs(5 * 60);
/// The Usage page's periods, in days.
pub const USAGE_PERIODS: [(u32, &str); 4] = [(1, "Today"), (7, "7 days"), (30, "30 days"), (90, "90 days")];

/// One project's saved threads, listed off the UI thread.
pub struct Listed {
    pub project: PathBuf,
    pub claude: Vec<history::Session>,
    pub kept: Vec<store::Saved>,
}

/// Where a thread sits in the sidebar's inbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    /// Needs you: waiting on an approval, finished, failed, or not settled yet.
    Inbox,
    /// A turn is running and nothing is asked of you.
    Working,
    /// Done with, until it does something again.
    Settled,
}

/// Where pasted screenshots wait to be sent. The CLIs read them during the turn (Codex and
/// OpenCode by path), so they outlive it, and `purge_old_pastes` clears them at a later launch.
fn pastes_dir() -> PathBuf {
    std::env::temp_dir().join("yoke-pastes")
}

/// Deletes pastes over a day old, so the app leaves nothing growing behind it. Windows never
/// empties its temp folder by itself; macOS and Linux do, but on their own schedule.
pub fn purge_old_pastes() {
    let Ok(entries) = std::fs::read_dir(pastes_dir()) else { return };
    let day = Duration::from_secs(24 * 3600);
    for e in entries.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > day);
        if old { let _ = std::fs::remove_file(e.path()); }
    }
    let _ = std::fs::remove_dir(pastes_dir()); // only if now empty
}

pub struct App {
    pub sidebar: PaneState,
    /// Every project in the sidebar, in the order added. Threads keep the folder they run in.
    pub projects: Vec<Project>,
    /// The project new threads start in: the selected thread's, else the last one chosen.
    /// `None` only with no projects at all.
    pub current: Option<PathBuf>,
    /// Newest first.
    pub threads: Vec<Thread>,
    /// `None` is the new-thread screen.
    pub selected: Option<ThreadId>,
    pub draft: TextEdit,
    /// The composer's choices; a thread keeps its harness once it has started.
    pub harness: Harness,
    /// A `ModelOpt::value`; `models::DEFAULT` sends no model flag.
    pub model: String,
    /// `DEFAULT_EFFORT` sends no effort flag; harnesses without one hide the picker.
    pub effort: String,
    pub access: Access,
    pub open_menu: Option<&'static str>,
    /// The model picker's search, rail and favorites (`picker`).
    pub picker: crate::picker::Picker,
    /// The new-thread screen's project picker: its search and the row under the arrows.
    pub project_pick: crate::picker::ProjectPick,
    /// Updates from GitHub Releases (`updater`).
    pub update: crate::updater::Updater,
    /// Settings (⌘,) fills the main pane instead of the thread.
    pub settings: bool,
    /// The page Settings shows.
    pub settings_page: crate::settings::Page,
    /// What a new thread starts with, and how the prompt sends (`settings`).
    pub defaults: crate::settings::Prefs,
    /// The Usage page (⇧⌘U) fills it the same way.
    pub usage_open: bool,
    /// Its period, in days (`USAGE_PERIODS`).
    pub usage_days: u32,
    /// Claude's plan limits, as last read (`harness::limits`), and why the last read failed.
    pub limits: Option<Limits>,
    pub limits_error: Option<String>,
    /// Context windows by model id, as Claude Code reported them (`CONTEXT_WINDOWS_KEY`).
    context_windows: HashMap<String, u64>,
    limits_rx: Option<Receiver<Result<Limits, String>>>,
    /// When the last read started, so a failing one is retried on the same beat.
    limits_asked: Option<Instant>,
    /// What Claude Code's transcripts add up to (`usage`), and a scan running.
    pub usage: usage::Report,
    usage_rx: Option<Receiver<usage::Report>>,
    /// A rescan was asked for while one ran.
    usage_again: bool,
    usage_cache: Arc<Mutex<usage::Cache>>,
    pub prices: Arc<usage::Prices>,
    prices_rx: Option<Receiver<Result<serde_json::Value, String>>>,
    /// The last minute `poll` redrew the usage numbers for ("resets in 12m").
    pub usage_beat: u64,
    /// Images dropped or pasted onto the composer, sent with the next message.
    pub attachments: Vec<Attachment>,
    /// A file drag is over the window.
    pub drop_over: bool,
    /// Why the last drop (not an image) or folder switch (a turn running) was refused.
    pub composer_error: Option<String>,
    images: Option<ImageCache>,
    pub appearance: Appearance,
    /// The colour theme (OpenCode, Nord, Gruvbox), in either appearance.
    pub theme: Theme,
    pub installed: Vec<(Harness, Option<PathBuf>)>,
    /// Each harness's model picker: what its CLI reported (or the last report, saved), else
    /// the built-in list.
    pub models: HashMap<Harness, Vec<ModelOpt>>,
    /// Tool rows showing their output.
    pub expanded: HashSet<u64>,
    pub hud: bool,
    /// The last half-second (HUD) or second (working counter) `poll` redrew for.
    pub beat: u64,
    pub keys: Keys,
    /// The shortcut Settings is recording a chord for.
    pub recording: Option<Cmd>,
    /// Why the last recorded chord was refused, under that command's row.
    pub key_error: Option<(Cmd, String)>,
    pub prefs: Prefs,
    /// The Settled shelf is open, and how many of its threads it lists.
    pub settled_open: bool,
    pub settled_shown: usize,
    /// The Working shelf is open (shut by default, as in T3 Code).
    pub working_open: bool,
    /// What happened while the window wasn't looked at, for `attention` to notify about.
    pub alerts: Vec<(ThreadId, Alert)>,
    /// Notifications posted since the window last had focus: the Dock badge.
    pub unseen: usize,
    /// When `attention::sweep` next looks for threads to settle, on `poll`'s clock.
    pub next_sweep: f64,
    /// Settled (true) or kept in the inbox (false) by hand, by session id, so the choice
    /// outlives the app and beats `AUTO_SETTLE`.
    settled_by_hand: HashMap<String, bool>,
    /// The thread the delete dialog asks about (the row's ✕, or ⇧⌘⌫).
    pub confirm_delete: Option<ThreadId>,
    /// Claude Code's folder, where deleting a thread deletes its session (`history::remove`).
    /// `None` in tests unless one sets a scratch folder.
    pub(crate) claude_home: Option<PathBuf>,
    events: Receiver<(ThreadId, Event)>,
    sender: Sender<(ThreadId, Event)>,
    discovered: Receiver<(Harness, Vec<ModelOpt>)>,
    sessions: Receiver<Listed>,
    sessions_tx: Sender<Listed>,
    /// Where Codex and OpenCode threads are kept (`store`); `None` in tests.
    store: Option<PathBuf>,
    /// The new thread being composed starts in a worktree of their own, where the project is a
    /// git repository. Starts from `defaults.worktree`; the composer's chip changes it.
    pub worktree_mode: bool,
    worktrees: Receiver<(ThreadId, Result<Worktree, String>)>,
    worktrees_tx: Sender<(ThreadId, Result<Worktree, String>)>,
    discover_tx: Sender<(Harness, Vec<ModelOpt>)>,
    pub(crate) waker: Option<Waker>,
    running: HashMap<ThreadId, Running>,
    /// Threads whose turn just ended with queued messages to send next.
    ready: Vec<ThreadId>,
    next_id: u64,
}

/// Something a thread did that the person would want to hear about when away.
#[derive(Clone, Debug, PartialEq)]
pub enum Alert {
    /// The turn finished.
    Done,
    /// The turn ended in an error.
    Failed,
    /// A tool waits for approval (its name).
    Approval(String),
}

/// A message waiting for the running turn to end, with the composer's choices when it was
/// queued.
pub struct Queued {
    pub id: u64,
    pub text: String,
    pub model: String,
    pub effort: String,
    pub access: Access,
    pub images: Vec<PathBuf>,
}

/// An image waiting in the composer: its file, and the thumbnail decoding from it.
pub struct Attachment {
    pub id: u64,
    pub path: PathBuf,
    pub thumb: Option<Image>,
}

/// Image types the agents take.
pub const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Appearance {
    System,
    Light,
    Dark,
}

pub const APPEARANCES: [Appearance; 3] = [Appearance::System, Appearance::Light, Appearance::Dark];

impl Appearance {
    pub fn label(self) -> &'static str {
        match self {
            Appearance::System => "System",
            Appearance::Light => "Light",
            Appearance::Dark => "Dark",
        }
    }
    pub fn dark(self, system_dark: bool) -> bool {
        match self {
            Appearance::System => system_dark,
            Appearance::Light => false,
            Appearance::Dark => true,
        }
    }
    fn from_label(l: &str) -> Option<Appearance> {
        APPEARANCES.into_iter().find(|a| a.label() == l)
    }
}

pub struct Project {
    pub name: String,
    pub path: PathBuf,
    /// It's in a git repository, so its threads can run in worktrees.
    pub repo: bool,
}

impl Project {
    pub fn new(path: PathBuf) -> Project {
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        let repo = worktree::is_repo(&path);
        Project { name, path, repo }
    }
}

/// On the first launch with no saved projects, the folders Claude Code has sessions in
/// become the sidebar's projects, so earlier threads are there from the start.
pub fn seed_projects(prefs: &mut Prefs) {
    if prefs.get::<Vec<String>>(PROJECTS_KEY).is_some() { return; }
    let found: Vec<String> = history::projects().into_iter().map(|p| p.to_string_lossy().into_owned()).collect();
    let _ = prefs.set(PROJECTS_KEY, &found);
}

/// A thread's context window and how much of it is used, in tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Context {
    pub used: u64,
    pub window: u64,
    /// The window is the size Claude Code reported, not a guess.
    pub measured: bool,
}

impl Context {
    /// How full it is, 0 to 1.
    pub fn fraction(&self) -> f64 {
        if self.window == 0 { 0.0 } else { (self.used as f64 / self.window as f64).clamp(0.0, 1.0) }
    }
}

pub struct Thread {
    pub id: ThreadId,
    pub title: String,
    pub harness: Harness,
    pub model: String,
    /// The model the harness reported (`claude-opus-5-5`), once it has.
    pub resolved_model: Option<String>,
    pub session: Option<String>,
    /// The project it belongs to. Empty (a test's thread) counts as the current project.
    pub project: PathBuf,
    /// The worktree it runs in instead of the project's folder, if it has one.
    pub worktree: Option<Worktree>,
    /// Its worktree is being made; its first message waits in the queue.
    pub preparing: bool,
    pub items: Vec<Item>,
    pub busy: bool,
    pub unread: bool,
    /// In the Settled shelf. Any turn activity (a send, an approval, a finish) clears it.
    pub settled: bool,
    /// The last turn ended in an error.
    pub failed: bool,
    /// The last activity, for the sidebar's "5m".
    pub updated: SystemTime,
    /// A saved transcript not read yet (a Claude Code session file, or a `store` record):
    /// it loads when the thread opens.
    pub saved: Option<PathBuf>,
    /// When the running turn started.
    pub turn_started: Option<Instant>,
    /// Bumped by every change; the sidebar row and toolbar read it.
    pub rev: u64,
    /// Follow-ups sent while a turn ran, oldest first. The next goes when the turn ends.
    pub queue: Vec<Queued>,
    /// The running turn is being stopped to send the queue's first message now.
    pub steering: bool,
    /// The person stopped the turn, so the queue waits instead of sending.
    pub stop_requested: bool,
    /// Tools waiting for the person's approval, oldest first.
    pub asks: Vec<Pending>,
    /// Tokens in its context after the latest model call (Claude Code), once known.
    pub context: Option<u64>,
    /// Its model's context window, as its last turn reported it.
    pub context_window: Option<u64>,
}

/// A question from the CLI, shown on the row (`item`) of the tool it's about.
pub struct Pending {
    pub item: u64,
    pub ask: AskRequest,
}

impl Thread {
    /// The approval waiting on transcript row `item`.
    pub fn ask_on(&self, item: u64) -> Option<&Pending> {
        self.asks.iter().find(|p| p.item == item)
    }

    /// The tool row for call `call` in the current turn (since the last message sent).
    fn tool_row(&mut self, call: &str) -> Option<&mut Item> {
        self.items.iter_mut().rev()
            .take_while(|i| !matches!(i.kind, Kind::User(_)))
            .find(|i| matches!(&i.kind, Kind::Tool { call: c, .. } if c == call))
    }

    pub fn new(id: ThreadId, title: String, harness: Harness, model: String) -> Thread {
        Thread {
            id,
            title,
            harness,
            model,
            resolved_model: None,
            session: None,
            project: PathBuf::new(),
            worktree: None,
            preparing: false,
            items: Vec::new(),
            busy: false,
            unread: false,
            settled: false,
            failed: false,
            updated: SystemTime::now(),
            saved: None,
            turn_started: None,
            rev: 0,
            queue: Vec::new(),
            steering: false,
            stop_requested: false,
            asks: Vec::new(),
            context: None,
            context_window: None,
        }
    }
}

pub struct Item {
    pub id: u64,
    /// Bumped by every change to the item, so a finished row's `memo` keeps its subtree.
    pub rev: u64,
    pub kind: Kind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolState {
    Running,
    Ok,
    Failed,
}

pub enum Kind {
    User(String),
    Assistant { text: String, streaming: bool },
    Tool { call: String, name: String, detail: String, state: ToolState, output: String },
    Error(String),
    TurnEnd { secs: f64, cost_usd: Option<f64>, stopped: bool },
}

impl App {
    /// `prefs` holds the shortcuts, appearance and last model lists: `Prefs::open(APP_ID)` for
    /// the app, `Prefs::memory()` for tests.
    /// `open` is a folder to open as a project (added if it's new), else the last one used.
    pub fn new(open: Option<PathBuf>, prefs: Prefs) -> App {
        let (sender, events) = channel();
        let (discover_tx, discovered) = channel();
        let (sessions_tx, sessions) = channel();
        let (worktrees_tx, worktrees) = channel();
        let installed = harness::detect();
        let harness = installed.iter().find(|(_, p)| p.is_some()).map_or(Harness::Claude, |(h, _)| *h);
        let mut projects: Vec<Project> = prefs.get::<Vec<String>>(PROJECTS_KEY).unwrap_or_default().into_iter()
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .map(Project::new)
            .collect();
        let open = open.filter(|p| p.is_dir()).map(|p| p.canonicalize().unwrap_or(p));
        let added = open.as_ref().is_some_and(|p| !projects.iter().any(|x| &x.path == p));
        if added && let Some(p) = &open {
            projects.push(Project::new(p.clone()));
        }
        let last = prefs.get::<String>(LAST_PROJECT_KEY).map(PathBuf::from);
        let current = open
            .or(last.filter(|l| projects.iter().any(|p| &p.path == l)))
            .or_else(|| projects.first().map(|p| p.path.clone()));
        let mut draft = TextEdit::new("");
        draft.focused = true;
        let models = harness::ALL.into_iter()
            .map(|h| {
                let saved: Option<Vec<(String, String)>> = prefs.get(&models_key(h));
                let list = saved.filter(|l| !l.is_empty())
                    .map(|l| l.into_iter().map(|(value, label)| ModelOpt { value, label }).collect())
                    .unwrap_or_else(|| models::fallback(h));
                (h, list)
            })
            .collect();
        let settled_by_hand = prefs.get(SETTLED_KEY).unwrap_or_default();
        let settled_open = prefs.get(SETTLED_OPEN_KEY).unwrap_or(false);
        let working_open = prefs.get(WORKING_OPEN_KEY).unwrap_or(false);
        let theme = prefs.get::<String>("theme").and_then(|l| Theme::from_label(&l)).unwrap_or(Theme::OpenCode);
        let appearance = prefs.get::<String>("appearance").and_then(|l| Appearance::from_label(&l)).unwrap_or(Appearance::System);
        let limits = prefs.get::<serde_json::Value>(LIMITS_KEY).and_then(|v| Limits::from_json(&v));
        let fetched = prefs.get::<u64>(PRICES_AT_KEY).map(|s| SystemTime::UNIX_EPOCH + Duration::from_secs(s));
        let prices = match prefs.get::<serde_json::Value>(PRICES_KEY) {
            Some(doc) => usage::Prices::from_json(&doc, fetched),
            None => usage::Prices::built_in(),
        };
        let prices = if prices.len() == 0 { usage::Prices::built_in() } else { prices };
        let usage_days = prefs.get::<u32>(USAGE_DAYS_KEY).filter(|d| USAGE_PERIODS.iter().any(|(p, _)| p == d)).unwrap_or(7);
        let context_windows = prefs.get(CONTEXT_WINDOWS_KEY).unwrap_or_default();
        let mut app = App {
            sidebar: PaneState::new(272.0).range(200.0, 480.0),
            context_windows,
            projects,
            current,
            threads: Vec::new(),
            selected: None,
            draft,
            harness,
            model: models::DEFAULT.into(),
            effort: harness::DEFAULT_EFFORT.into(),
            access: Access::Edits,
            open_menu: None,
            settings: false,
            settings_page: crate::settings::Page::default(),
            defaults: crate::settings::Prefs::load(&prefs),
            usage_open: false,
            usage_days,
            limits,
            limits_error: None,
            limits_rx: None,
            limits_asked: None,
            usage: usage::Report::default(),
            usage_rx: None,
            usage_again: false,
            usage_cache: Arc::new(Mutex::new(usage::Cache::default())),
            prices: Arc::new(prices),
            prices_rx: None,
            usage_beat: 0,
            attachments: Vec::new(),
            drop_over: false,
            composer_error: None,
            images: None,
            appearance,
            theme,
            installed,
            models,
            expanded: HashSet::new(),
            hud: false,
            beat: 0,
            keys: Keys::load(&prefs),
            picker: crate::picker::Picker::load(&prefs),
            project_pick: crate::picker::ProjectPick::default(),
            update: crate::updater::Updater::default(),
            store: store::dir(&prefs),
            // `apply_defaults` sets it from Settings › New threads.
            worktree_mode: false,
            worktrees,
            worktrees_tx,
            recording: None,
            key_error: None,
            prefs,
            settled_open,
            settled_shown: SETTLED_PAGE,
            working_open,
            alerts: Vec::new(),
            unseen: 0,
            next_sweep: 0.0,
            settled_by_hand,
            confirm_delete: None,
            claude_home: if cfg!(test) { None } else { history::claude_home() },
            events,
            sender,
            discovered,
            sessions,
            sessions_tx,
            discover_tx,
            waker: None,
            running: HashMap::new(),
            ready: Vec::new(),
            next_id: 1,
        };
        app.apply_defaults();
        if added { app.save_projects(); }
        app
    }

    /// Called once the window exists: keeps the waker, asks each installed CLI for its models,
    /// and lists the project's saved Claude Code sessions.
    /// A window (not a headless run) is driving this state.
    pub fn has_waker(&self) -> bool {
        self.waker.is_some()
    }

    pub fn set_waker(&mut self, w: Waker) {
        self.images = Some(ImageCache::new(w.clone()));
        self.waker = Some(w);
        self.discover_models();
        self.load_sessions();
        self.refresh_limits();
        self.rescan_usage();
        self.fetch_prices_if_stale();
    }

    /// Reads Claude's plan limits again, off the UI thread, unless a read is running or
    /// Claude Code isn't installed; `drain` lands the answer.
    pub fn refresh_limits(&mut self) {
        // No waker: a test's state, not the app's, and nothing should spend time asking.
        if self.limits_rx.is_some() || self.waker.is_none() { return; }
        let Some(bin) = self.binary(Harness::Claude) else { return };
        let (tx, rx) = channel();
        self.limits_rx = Some(rx);
        self.limits_asked = Some(Instant::now());
        let (cwd, wake) = (self.work_dir(), self.waker.clone());
        std::thread::spawn(move || {
            let _ = tx.send(limits::probe(&bin, &cwd));
            if let Some(w) = wake { w(); }
        });
    }

    pub fn limits_busy(&self) -> bool {
        self.limits_rx.is_some()
    }

    /// Totals Claude Code's transcripts again (only files that changed are read).
    pub fn rescan_usage(&mut self) {
        if self.waker.is_none() { return; }
        if self.usage_rx.is_some() {
            self.usage_again = true;
            return;
        }
        let (tx, rx) = channel();
        self.usage_rx = Some(rx);
        let (cache, prices, wake) = (self.usage_cache.clone(), self.prices.clone(), self.waker.clone());
        std::thread::spawn(move || {
            let report = usage::scan(&mut cache.lock().unwrap(), &usage::roots(), &prices);
            let _ = tx.send(report);
            if let Some(w) = wake { w(); }
        });
    }

    pub fn usage_scanning(&self) -> bool {
        self.usage_rx.is_some()
    }

    /// Fetches LiteLLM's prices when the saved copy is a day old (or there's none).
    fn fetch_prices_if_stale(&mut self) {
        let fresh = self.prices.fetched.and_then(|t| t.elapsed().ok()).is_some_and(|age| age < usage::PRICES_MAX_AGE);
        if fresh || self.prices_rx.is_some() || self.waker.is_none() { return; }
        let (tx, rx) = channel();
        self.prices_rx = Some(rx);
        let wake = self.waker.clone();
        std::thread::spawn(move || {
            let _ = tx.send(usage::fetch_prices());
            if let Some(w) = wake { w(); }
        });
    }

    /// Lands whatever the limits read, the usage scan and the price fetch sent back.
    fn drain_usage(&mut self) -> bool {
        let mut changed = false;
        if let Some(r) = self.limits_rx.as_ref().and_then(|r| r.try_recv().ok()) {
            self.limits_rx = None;
            changed = true;
            match r {
                Ok(l) => {
                    let _ = self.prefs.set(LIMITS_KEY, &l.to_json());
                    self.limits = Some(l);
                    self.limits_error = None;
                }
                Err(e) => self.limits_error = Some(e),
            }
        }
        if let Some(report) = self.usage_rx.as_ref().and_then(|r| r.try_recv().ok()) {
            self.usage_rx = None;
            self.usage = report;
            changed = true;
            if std::mem::take(&mut self.usage_again) { self.rescan_usage(); }
        }
        if let Some(r) = self.prices_rx.as_ref().and_then(|r| r.try_recv().ok()) {
            self.prices_rx = None;
            // Offline: the saved or built-in prices stay, and the next launch tries again.
            if let Ok(doc) = r {
                let now = SystemTime::now();
                let _ = self.prefs.set(PRICES_KEY, &doc);
                let _ = self.prefs.set(PRICES_AT_KEY, &now.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs()));
                let prices = usage::Prices::from_json(&doc, Some(now));
                if prices.len() > 0 {
                    self.prices = Arc::new(prices);
                    changed = true;
                    self.rescan_usage();
                }
            }
        }
        changed
    }

    /// Once a minute from `poll`: every `LIMITS_EVERY` the plan's limits are read again and the
    /// transcripts rescanned (terminal sessions spend too), and the usage numbers on screen
    /// ("resets in 12m") redraw. True if the screen changed.
    pub fn usage_tick(&mut self, now: SystemTime) -> bool {
        let due = self.limits_asked.is_none_or(|t| t.elapsed() >= LIMITS_EVERY);
        if due {
            self.refresh_limits();
            self.rescan_usage();
        }
        let minute = now.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 60);
        let shown = self.usage_open || self.limits.is_some();
        if shown && minute != self.usage_beat {
            self.usage_beat = minute;
            return true;
        }
        false
    }

    pub fn open_usage(&mut self) {
        self.close_settings();
        self.open_menu = None;
        self.usage_open = true;
        // Fresh numbers for the page: limits more than a minute old, and any new transcript lines.
        let old = self.limits.as_ref().is_none_or(|l| l.checked.elapsed().map_or(true, |a| a > Duration::from_secs(60)));
        if old { self.limits_error = None; self.refresh_limits(); }
        self.rescan_usage();
    }

    pub fn close_usage(&mut self) {
        self.usage_open = false;
        self.draft.focused = true;
    }

    pub fn set_usage_days(&mut self, days: u32) {
        self.usage_days = days;
        let _ = self.prefs.set(USAGE_DAYS_KEY, &days);
    }

    /// Lists every project's Claude Code sessions off the UI thread; `drain` adds them as
    /// threads.
    pub fn load_sessions(&mut self) {
        let paths: Vec<PathBuf> = self.projects.iter().map(|p| p.path.clone()).collect();
        self.load_sessions_of(paths);
    }

    fn load_sessions_of(&self, paths: Vec<PathBuf>) {
        let (tx, wake, store) = (self.sessions_tx.clone(), self.waker.clone(), self.store.clone());
        std::thread::spawn(move || {
            for p in paths {
                let claude = history::list(&p);
                let kept = store.as_deref().map(|d| store::list(d, &p)).unwrap_or_default();
                let _ = tx.send(Listed { project: p, claude, kept });
                if let Some(w) = &wake { w(); }
            }
        });
    }

    /// Adds `project`'s kept Codex and OpenCode threads no thread has yet.
    pub(crate) fn land_kept(&mut self, project: &Path, list: Vec<store::Saved>) {
        if !self.projects.iter().any(|p| p.path == project) { return; }
        for x in list {
            if self.threads.iter().any(|t| t.harness == x.harness && t.session.as_deref() == Some(x.session.as_str())) { continue; }
            let id = self.id();
            let mut t = Thread::new(id, x.title, x.harness, x.model);
            let idle = x.modified.elapsed().is_ok_and(|d| d >= AUTO_SETTLE);
            t.settled = self.settled_by_hand.get(&x.session).copied().unwrap_or(idle);
            // A Claude thread's transcript is Claude Code's session file, kept under the
            // worktree's folder; others are in the record.
            t.saved = match (&x.worktree, x.harness) {
                (Some(w), Harness::Claude) => history::dir(&w.path).map(|d| d.join(format!("{}.jsonl", x.session))),
                _ => Some(x.path),
            };
            t.session = Some(x.session);
            t.project = project.to_owned();
            t.worktree = x.worktree;
            t.updated = x.modified;
            self.threads.push(t);
        }
        self.threads.sort_by(|a, b| b.updated.cmp(&a.updated));
    }

    /// A worktree was made (its queued message goes now) or couldn't be (it waits, to send
    /// in the project's folder instead).
    pub(crate) fn land_worktree(&mut self, id: ThreadId, made: Result<Worktree, String>) {
        let fresh = self.id();
        let Some(t) = self.thread_mut(id) else { return };
        t.preparing = false;
        t.busy = false;
        t.turn_started = None;
        t.rev += 1;
        match made {
            Ok(w) => {
                t.worktree = Some(w);
                self.ready.push(id);
            }
            Err(e) => {
                t.failed = true;
                t.items.push(Item { id: fresh, rev: 0, kind: Kind::Error(format!("Couldn't make a worktree: {e}. The message is still queued: send it to run in the project's folder instead.")) });
            }
        }
    }

    /// The composer's Local / New worktree chip: for this new thread only. Settings › New
    /// threads has the default each new thread starts from.
    pub fn toggle_worktree_mode(&mut self) {
        self.worktree_mode = !self.worktree_mode;
    }

    /// Writes thread `id`'s record if it's one the app keeps.
    fn keep(&self, id: ThreadId) {
        let (Some(dir), Some(t)) = (&self.store, self.threads.iter().find(|t| t.id == id)) else { return };
        if store::keeps(t) { store::save(dir, t); }
    }

    /// Adds `project`'s sessions no thread has yet, newest first among the threads.
    pub(crate) fn land_sessions(&mut self, project: &Path, list: Vec<history::Session>) {
        if !self.projects.iter().any(|p| p.path == project) { return; }
        for x in list {
            if self.threads.iter().any(|t| t.session.as_deref() == Some(x.id.as_str())) { continue; }
            let id = self.id();
            let mut t = Thread::new(id, x.title, Harness::Claude, models::DEFAULT.into());
            let idle = x.modified.elapsed().is_ok_and(|d| d >= AUTO_SETTLE);
            t.settled = self.settled_by_hand.get(&x.id).copied().unwrap_or(idle);
            t.session = Some(x.id);
            t.project = project.to_owned();
            t.saved = Some(x.path);
            t.updated = x.modified;
            self.threads.push(t);
        }
        self.threads.sort_by(|a, b| b.updated.cmp(&a.updated));
    }

    /// Images dropped on the window, or pasted: added to the composer for the next message.
    pub fn attach(&mut self, files: &[PathBuf]) {
        self.drop_over = false;
        self.composer_error = None;
        for path in files {
            if self.attachments.iter().any(|a| a.path == *path) { continue; }
            let id = self.id();
            let thumb = self.images.as_ref().map(|c| c.file(path, 56.0, 2.0));
            self.attachments.push(Attachment { id, path: path.clone(), thumb });
        }
        self.draft.focused = true;
    }

    /// A pasted image (a screenshot): saved to a temporary file and attached.
    pub fn attach_png(&mut self, png: &[u8]) {
        let dir = pastes_dir();
        let _ = std::fs::create_dir_all(&dir);
        let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let path = dir.join(format!("pasted-{stamp}.png"));
        if std::fs::write(&path, png).is_ok() { self.attach(&[path]); }
    }

    pub fn detach(&mut self, id: u64) {
        self.attachments.retain(|a| a.id != id);
    }

    /// Asks each installed CLI that can list its models, off the UI thread; `drain` lands the
    /// answers.
    pub fn discover_models(&self) {
        for (h, bin) in self.installed.iter().filter_map(|(h, p)| Some((*h, p.clone()?))) {
            let (tx, cwd, wake) = (self.discover_tx.clone(), self.work_dir(), self.waker.clone());
            std::thread::spawn(move || {
                if let Some(list) = models::discover(h, &bin, &cwd) {
                    let _ = tx.send((h, list));
                    if let Some(w) = wake { w(); }
                }
            });
        }
    }

    /// The model picker's entries for `h`.
    pub fn models(&self, h: Harness) -> &[ModelOpt] {
        self.models.get(&h).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The label the model picker shows for `value` (the value itself if the list lacks it).
    pub fn model_label<'a>(&'a self, h: Harness, value: &'a str) -> &'a str {
        self.models(h).iter().find(|m| m.value == value).map_or(value, |m| m.label.as_str())
    }

    pub fn set_appearance(&mut self, a: Appearance) {
        self.appearance = a;
        let _ = self.prefs.set("appearance", &a.label().to_owned());
    }

    pub fn set_theme(&mut self, t: Theme) {
        self.theme = t;
        let _ = self.prefs.set("theme", &t.label().to_owned());
    }

    fn land_models(&mut self, h: Harness, list: Vec<ModelOpt>) {
        let saved: Vec<(String, String)> = list.iter().map(|m| (m.value.clone(), m.label.clone())).collect();
        let _ = self.prefs.set(&models_key(h), &saved);
        // A composer choice the CLI no longer offers falls back to the default.
        if self.active_harness() == h && self.thread().is_none() && !list.iter().any(|m| m.value == self.model) {
            self.model = models::DEFAULT.into();
        }
        self.models.insert(h, list);
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn thread(&self) -> Option<&Thread> {
        self.selected.and_then(|id| self.threads.iter().find(|t| t.id == id))
    }

    fn thread_mut(&mut self, id: ThreadId) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|t| t.id == id)
    }

    pub fn binary(&self, h: Harness) -> Option<PathBuf> {
        self.installed.iter().find(|(x, _)| *x == h).and_then(|(_, p)| p.clone())
    }

    /// How full the composer's thread's context is, for its gauge: Claude Code threads only
    /// (the other CLIs don't say). A new thread's is empty.
    pub fn context(&self) -> Option<Context> {
        if self.active_harness() != Harness::Claude { return None; }
        let t = self.thread();
        let model = t.map_or(self.model.as_str(), |t| t.resolved_model.as_deref().unwrap_or(&t.model));
        let known = t.and_then(|t| t.context_window).or_else(|| self.context_windows.get(model).copied());
        // Before Claude Code has said: 1M for a `[1m]` model, else the 200K most have.
        let window = known.unwrap_or(if model.contains("[1m]") { 1_000_000 } else { 200_000 });
        Some(Context { used: t.and_then(|t| t.context).unwrap_or(0), window, measured: known.is_some() })
    }

    /// The harness the composer sends to: the thread's, else the picker's.
    pub fn active_harness(&self) -> Harness {
        self.thread().map_or(self.harness, |t| t.harness)
    }

    pub fn is_running(&self, id: ThreadId) -> bool {
        self.running.contains_key(&id)
    }

    /// Asks for a folder (⌘O) and adds it as a project.
    pub fn choose_folder(&mut self, tx: &mut Tx) {
        self.open_menu = None;
        let directory = self.current.as_ref().and_then(|p| p.parent()).map(PathBuf::from);
        let options = rune::FileDialogOptions { title: "Add Project".into(), directory, ..Default::default() };
        tx.open_folder(options, |s: &mut App, result, _: &mut Tx| match result {
            Ok(Some(path)) => s.add_project(path),
            Ok(None) => {}
            Err(e) => s.composer_error = Some(format!("Couldn't open a folder: {e}")),
        });
    }

    /// The project new threads start in.
    pub fn project(&self) -> Option<&Project> {
        let path = self.thread().map(|t| t.project.as_path()).filter(|p| !p.as_os_str().is_empty()).or(self.current.as_deref())?;
        self.projects.iter().find(|p| p.path == path)
    }

    /// Where background asks (models, plan limits) run: the current project, else home.
    fn work_dir(&self) -> PathBuf {
        self.current.clone().or_else(|| std::env::var_os("HOME").map(PathBuf::from)).unwrap_or_else(|| PathBuf::from("."))
    }

    /// The folder thread `t`'s turns run in: its worktree, else its project's.
    pub fn run_dir(&self, t: &Thread) -> Option<PathBuf> {
        if let Some(w) = &t.worktree { return Some(w.path.clone()); }
        Some(t.project.clone()).filter(|p| !p.as_os_str().is_empty()).or_else(|| self.current.clone())
    }

    /// Whether thread `t` is in the project at `path`.
    pub fn in_project(&self, t: &Thread, path: &Path) -> bool {
        if t.project.as_os_str().is_empty() { self.current.as_deref() == Some(path) } else { t.project == path }
    }

    /// Adds `path` as a project (if it's new) and starts a new thread in it. Its saved
    /// sessions come in as threads.
    pub fn add_project(&mut self, path: PathBuf) {
        let path = path.canonicalize().unwrap_or(path);
        if !self.projects.iter().any(|p| p.path == path) {
            self.projects.push(Project::new(path.clone()));
            self.load_sessions_of(vec![path.clone()]);
        }
        self.composer_error = None;
        self.save_projects();
        self.new_thread_in(path);
    }

    /// Takes the project out of Yoke, with its threads. Its folder and Claude Code's
    /// saved sessions stay; adding it again brings them back. Refused while one of its turns runs.
    pub fn remove_project(&mut self, path: &Path) -> bool {
        let threads: Vec<ThreadId> = self.threads.iter().filter(|t| self.in_project(t, path)).map(|t| t.id).collect();
        if threads.iter().any(|id| self.is_running(*id)) {
            self.composer_error = Some("Stop the project's running turns before removing it.".into());
            return false;
        }
        self.threads.retain(|t| !threads.contains(&t.id));
        if self.selected.is_some_and(|id| threads.contains(&id)) { self.selected = None; }
        self.projects.retain(|p| p.path != path);
        if self.current.as_deref() == Some(path) {
            self.current = self.projects.first().map(|p| p.path.clone());
        }
        self.save_projects();
        true
    }

    /// A new thread in the project at `path`.
    pub fn new_thread_in(&mut self, path: PathBuf) {
        self.set_current(path);
        self.new_thread();
    }

    pub(crate) fn set_current(&mut self, path: PathBuf) {
        if self.current.as_ref() == Some(&path) { return; }
        let _ = self.prefs.set(LAST_PROJECT_KEY, &path.to_string_lossy().into_owned());
        self.current = Some(path);
    }

    fn save_projects(&mut self) {
        let paths: Vec<String> = self.projects.iter().map(|p| p.path.to_string_lossy().into_owned()).collect();
        let _ = self.prefs.set(PROJECTS_KEY, &paths);
    }

    /// ⌘N: the new-thread screen, with the composer back on the new-thread defaults.
    pub fn new_thread(&mut self) {
        self.close_settings();
        self.usage_open = false;
        self.selected = None;
        self.apply_defaults();
        self.draft.focused = true;
        self.open_menu = None;
    }

    pub fn open_settings(&mut self) {
        self.open_menu = None;
        self.usage_open = false;
        self.settings = true;
    }

    pub fn close_settings(&mut self) {
        self.settings = false;
        self.recording = None;
        self.key_error = None;
        self.draft.focused = true;
    }

    pub fn toggle_menu(&mut self, menu: &'static str) {
        self.open_menu = if self.open_menu == Some(menu) { None } else { Some(menu) };
    }

    /// Selects the thread `by` rows below (positive) or above the selected one in the sidebar;
    /// from the new-thread screen, the first or last. False when there's nowhere to go.
    pub fn select_adjacent(&mut self, by: isize) -> bool {
        let order = self.sidebar_order();
        if order.is_empty() { return false; }
        let n = order.len() as isize;
        let at = match self.selected.and_then(|id| order.iter().position(|&o| o == id)) {
            Some(i) => i as isize + by,
            None if by > 0 => 0,
            None => n - 1,
        };
        if !(0..n).contains(&at) { return false; }
        self.select(order[at as usize]);
        true
    }

    pub fn lane(t: &Thread) -> Lane {
        if !t.asks.is_empty() { Lane::Inbox } else if t.busy { Lane::Working } else if t.settled { Lane::Settled } else { Lane::Inbox }
    }

    /// One lane's threads in the sidebar's order: the last to come back to you (finish, ask,
    /// get a message) first.
    pub fn lane_threads(&self, lane: Lane) -> Vec<&Thread> {
        let mut v: Vec<&Thread> = self.threads.iter().filter(|t| Self::lane(t) == lane).collect();
        v.sort_by(|a, b| b.updated.cmp(&a.updated));
        v
    }

    /// The inbox in the sidebar's order: every project's threads in one list, as T3 Code's
    /// Active list, the last to come back to you first.
    pub fn inbox_order(&self) -> Vec<ThreadId> {
        self.lane_threads(Lane::Inbox).iter().map(|t| t.id).collect()
    }

    /// The settled threads the shelf lists: up to `settled_shown` while it's open; while it's
    /// shut, only the selected one, so the thread you're reading keeps its row.
    pub fn settled_listed(&self) -> Vec<&Thread> {
        let all = self.lane_threads(Lane::Settled);
        if self.settled_open { all.into_iter().take(self.settled_shown).collect() } else { all.into_iter().filter(|t| self.selected == Some(t.id)).collect() }
    }

    /// The working threads the Working shelf lists: all while it's open; while it's shut, only
    /// the selected one.
    pub fn working_listed(&self) -> Vec<&Thread> {
        let all = self.lane_threads(Lane::Working);
        if self.working_open { all } else { all.into_iter().filter(|t| self.selected == Some(t.id)).collect() }
    }

    pub fn toggle_working_shelf(&mut self) {
        self.working_open = !self.working_open;
        let _ = self.prefs.set(WORKING_OPEN_KEY, &self.working_open);
    }

    /// Every thread row in the sidebar, top to bottom: the inbox, Working, then Settled.
    pub fn sidebar_order(&self) -> Vec<ThreadId> {
        let mut v = self.inbox_order();
        v.extend(self.working_listed().iter().map(|t| t.id));
        v.extend(self.settled_listed().iter().map(|t| t.id));
        v
    }

    /// Moves a thread to the Settled shelf (⇧⌘S, or ✓ on its row). Settling the open thread
    /// goes on to the next one in the inbox, as in T3 Code, or the new-thread screen at inbox
    /// zero. A running thread can't settle: it would come straight back when it finished.
    pub fn settle(&mut self, id: ThreadId) -> bool {
        let inbox = self.inbox_order();
        let Some(t) = self.thread_mut(id).filter(|t| !t.busy && t.asks.is_empty() && !t.settled) else { return false };
        t.settled = true;
        t.unread = false;
        t.rev += 1;
        let session = t.session.clone();
        self.remember_settled(session, true);
        if self.selected == Some(id) {
            let next = inbox.iter().position(|&i| i == id).and_then(|at| inbox.get(at + 1).or_else(|| at.checked_sub(1).and_then(|p| inbox.get(p))));
            match next {
                Some(&n) => self.select(n),
                None => self.new_thread(),
            }
        }
        true
    }

    /// Settles every thread nobody has touched for `AUTO_SETTLE`, as T3 Code does: not one
    /// running, asking, holding a queue, or kept in the inbox by hand. The automatic move
    /// isn't saved as a choice. Returns whether any moved, and when the next one is due.
    pub fn auto_settle(&mut self) -> (bool, Option<Duration>) {
        let kept = |t: &Thread, by_hand: &HashMap<String, bool>| t.session.as_ref().is_some_and(|s| by_hand.get(s) == Some(&false));
        let mut moved = false;
        let mut next: Option<Duration> = None;
        for t in &mut self.threads {
            if t.settled || t.busy || !t.asks.is_empty() || !t.queue.is_empty() || kept(t, &self.settled_by_hand) { continue; }
            let idle = t.updated.elapsed().unwrap_or_default();
            if idle >= AUTO_SETTLE {
                t.settled = true;
                t.unread = false;
                t.rev += 1;
                moved = true;
            } else {
                let due = AUTO_SETTLE - idle;
                next = Some(next.map_or(due, |n| n.min(due)));
            }
        }
        (moved, next)
    }

    /// Back to the inbox from the Settled shelf.
    pub fn unsettle(&mut self, id: ThreadId) {
        let Some(t) = self.thread_mut(id).filter(|t| t.settled) else { return };
        t.settled = false;
        t.updated = SystemTime::now();
        t.rev += 1;
        let session = t.session.clone();
        self.remember_settled(session, false);
    }

    pub fn toggle_settled_shelf(&mut self) {
        self.settled_open = !self.settled_open;
        self.settled_shown = SETTLED_PAGE;
        let _ = self.prefs.set(SETTLED_OPEN_KEY, &self.settled_open);
    }

    fn remember_settled(&mut self, session: Option<String>, settled: bool) {
        let Some(s) = session else { return };
        self.settled_by_hand.insert(s, settled);
        let _ = self.prefs.set(SETTLED_KEY, &self.settled_by_hand);
    }

    /// A thread doing something comes back to the inbox. The automatic return isn't saved as
    /// a choice: the activity itself keeps it fresh past `AUTO_SETTLE`.
    fn wake(&mut self, id: ThreadId) {
        let Some(t) = self.thread_mut(id).filter(|t| t.settled) else { return };
        t.settled = false;
        let session = t.session.clone();
        if let Some(s) = session && self.settled_by_hand.remove(&s).is_some() {
            let _ = self.prefs.set(SETTLED_KEY, &self.settled_by_hand);
        }
    }

    pub fn select(&mut self, id: ThreadId) {
        self.close_settings();
        self.usage_open = false;
        self.selected = Some(id);
        self.open_menu = None;
        let mut fresh = self.next_id;
        if let Some(t) = self.thread_mut(id) {
            t.unread = false;
            t.rev += 1;
            if let Some(path) = t.saved.take() {
                let kinds = if t.harness == Harness::Claude {
                    let loaded = history::load(&path);
                    if let Some((model, used)) = loaded.context {
                        t.context = Some(used);
                        t.resolved_model.get_or_insert(model);
                    }
                    loaded.rows
                } else {
                    store::load(&path)
                };
                for kind in kinds {
                    fresh += 1;
                    t.items.push(Item { id: fresh, rev: 0, kind });
                }
            }
            self.model = t.model.clone();
        }
        self.next_id = fresh;
        if let Some(p) = self.thread().map(|t| t.project.clone()).filter(|p| !p.as_os_str().is_empty()) {
            self.set_current(p);
        }
        self.draft.focused = true;
    }

    /// The row's ✕, or ⇧⌘⌫: asks before `delete`, since it can't be undone.
    pub fn ask_delete(&mut self, id: ThreadId) {
        if self.threads.iter().any(|t| t.id == id) {
            self.open_menu = None;
            self.confirm_delete = Some(id);
        }
    }

    /// The Claude Code session deleting thread `id` deletes from disk, if it has one.
    pub fn saved_session(&self, id: ThreadId) -> Option<&str> {
        self.threads.iter().find(|t| t.id == id).filter(|t| t.harness == Harness::Claude).and_then(|t| t.session.as_deref())
    }

    /// Deletes the thread, and with it its Claude Code session: the transcript, its subagent
    /// and tool output, and its file snapshots (`history::remove`), so it doesn't come back
    /// next launch or sit on disk. Other harnesses' threads leave the app only.
    pub fn delete(&mut self, id: ThreadId) {
        self.confirm_delete = None;
        let session = self.saved_session(id).map(str::to_owned);
        // The folder Claude Code filed the session under: the one the thread ran in.
        let folder = self.threads.iter().find(|t| t.id == id).map(|t| self.run_dir(t));
        if self.selected == Some(id) {
            // Like settling: go on to the thread below.
            let order = self.sidebar_order();
            if let Some(at) = order.iter().position(|&i| i == id) {
                let next = order.get(at + 1).or_else(|| at.checked_sub(1).and_then(|p| order.get(p))).copied();
                if let Some(n) = next { self.select(n); }
            }
        }
        let running = self.running.remove(&id);
        if let Some(r) = &running { r.stop(); }
        if let (Some(dir), Some(t)) = (&self.store, self.threads.iter().find(|t| t.id == id))
            && let (true, Some(s)) = (store::keeps(t), &t.session) {
                store::remove(dir, t.harness, s);
            }
        self.threads.retain(|t| t.id != id);
        if self.selected == Some(id) { self.selected = None; }
        let Some(session) = session else { return };
        if self.settled_by_hand.remove(&session).is_some() {
            let _ = self.prefs.set(SETTLED_KEY, &self.settled_by_hand);
        }
        let (Some(home), Some(Some(project))) = (self.claude_home.clone(), folder) else { return };
        std::thread::spawn(move || {
            // The killed CLI may not have exited yet; let it go before its files do.
            if running.is_some() { std::thread::sleep(Duration::from_millis(300)); }
            history::remove(&home, &project, &session);
        });
    }

    /// Switches the new-thread composer to `h`, with its default model and effort.
    pub fn choose_harness(&mut self, h: Harness) {
        self.harness = h;
        self.model = self.default_model(h);
        self.effort = self.default_effort(h);
    }

    /// The draft with the composer's current choices, emptying the composer. `None` if blank.
    fn take_draft(&mut self) -> Option<Queued> {
        let mut text = self.draft.text.trim().to_owned();
        let images: Vec<PathBuf> = self.attachments.drain(..).map(|a| a.path).collect();
        if text.is_empty() && images.is_empty() { return None; }
        if text.is_empty() { text = "See the attached image.".into(); }
        self.draft.clear();
        let id = self.id();
        Some(Queued { id, text, model: self.model.clone(), effort: self.effort.clone(), access: self.access, images })
    }

    /// Return: sends the draft to the selected thread (or starts a thread with it). While that
    /// thread's turn runs, queues it to go when the turn ends.
    pub fn send(&mut self, tx: &mut Tx) {
        let Some(q) = self.take_draft() else { return };
        match self.selected {
            Some(id) if self.is_running(id) || self.thread().is_some_and(|t| t.preparing) => {
                if let Some(t) = self.thread_mut(id) {
                    t.queue.push(q);
                    t.rev += 1;
                }
            }
            Some(id) => self.start_turn(id, q, tx),
            None => {
                let id = self.id();
                let Some(project) = self.current.clone() else {
                    self.composer_error = Some("Add a project first (⌘O).".into());
                    return;
                };
                let mut t = Thread::new(id, harness::first_line(&q.text, 60), self.harness, q.model.clone());
                t.project = project.clone();
                if self.worktree_mode && self.project().is_some_and(|p| p.repo) {
                    // The message waits in the queue until the worktree is made (`drain`).
                    let branch = worktree::branch_name(&q.text, id ^ SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos() as u64));
                    t.preparing = true;
                    t.busy = true;
                    t.turn_started = Some(Instant::now());
                    t.queue.push(q);
                    self.threads.insert(0, t);
                    self.selected = Some(id);
                    let (tx, wake) = (self.worktrees_tx.clone(), self.waker.clone());
                    std::thread::spawn(move || {
                        let _ = tx.send((id, worktree::create(&project, &branch)));
                        if let Some(w) = wake { w(); }
                    });
                    return;
                }
                self.threads.insert(0, t);
                self.selected = Some(id);
                self.start_turn(id, q, tx);
            }
        }
    }

    /// Steer (⌘↩): sends the draft into the running turn now. Print-mode CLIs can't take input
    /// mid-turn, so the turn stops and the message goes straight away on the same session; the
    /// agent sees the conversation so far and the new instruction. With nothing running it's
    /// an ordinary send.
    pub fn steer(&mut self, tx: &mut Tx) {
        match self.selected.filter(|id| self.is_running(*id)) {
            Some(id) => {
                let Some(q) = self.take_draft() else { return };
                if let Some(t) = self.thread_mut(id) {
                    t.queue.insert(0, q);
                }
                self.steer_queue(id);
            }
            None => self.send(tx),
        }
    }

    /// Sends queued message `qid` now: steering the running turn, or starting a turn if none runs.
    pub fn send_queued_now(&mut self, id: ThreadId, qid: u64, tx: &mut Tx) {
        let Some(t) = self.thread_mut(id) else { return };
        let Some(i) = t.queue.iter().position(|q| q.id == qid) else { return };
        let q = t.queue.remove(i);
        t.queue.insert(0, q);
        t.rev += 1;
        if self.is_running(id) {
            self.steer_queue(id);
        } else {
            let q = self.thread_mut(id).unwrap().queue.remove(0);
            self.start_turn(id, q, tx);
        }
    }

    pub fn unqueue(&mut self, id: ThreadId, qid: u64) {
        if let Some(t) = self.thread_mut(id) {
            t.queue.retain(|q| q.id != qid);
            t.rev += 1;
        }
    }

    /// Stops the running turn so the queue's first message goes as soon as it has exited.
    fn steer_queue(&mut self, id: ThreadId) {
        if let Some(t) = self.thread_mut(id) {
            t.steering = true;
            t.rev += 1;
        }
        if let Some(r) = self.running.get(&id) { r.stop(); }
    }

    fn start_turn(&mut self, id: ThreadId, q: Queued, tx: &mut Tx) {
        self.wake(id);
        let item = self.id();
        let Some(cwd) = self.threads.iter().find(|t| t.id == id).and_then(|t| self.run_dir(t)) else { return };
        let peer_messages = self.defaults.peer_messages;
        let Some(t) = self.thread_mut(id) else { return };
        let shown = if q.images.is_empty() { q.text.clone() } else {
            let names: Vec<String> = q.images.iter().map(|p| p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())).collect();
            format!("{}\n\nAttached: {}", q.text, names.join(", "))
        };
        t.items.push(Item { id: item, rev: 0, kind: Kind::User(shown) });
        t.model = q.model.clone();
        t.rev += 1;
        t.failed = false;
        t.updated = SystemTime::now();
        let turn = Turn { harness: t.harness, prompt: q.text, model: q.model, effort: q.effort, access: q.access, resume: t.session.clone(), cwd, images: q.images, peer_messages };
        let h = t.harness;
        let Some(bin) = self.binary(h) else {
            let e = self.id();
            let t = self.thread_mut(id).unwrap();
            t.items.push(Item { id: e, rev: 0, kind: Kind::Error(format!("`{}` isn't installed, so {} can't run. Install it and restart.", h.binary(), h.name())) });
            return;
        };
        let waker = self.waker.clone().unwrap_or_else(|| std::sync::Arc::new(|| {}));
        self.running.insert(id, harness::spawn(&bin, turn, id, self.sender.clone(), waker));
        if let Some(t) = self.thread_mut(id) {
            t.busy = true;
            t.turn_started = Some(Instant::now());
        }
        if self.selected == Some(id) { tx.scroll_to_bottom("transcript"); }
        tx.wake_after(Duration::from_secs(1));
    }

    /// The access picker. A Claude Code turn already running on the selected thread switches
    /// to the new level straight away, and approvals it now covers are allowed.
    pub fn set_access(&mut self, a: Access) {
        self.access = a;
        let Some(t) = self.thread().filter(|t| t.harness.asks()) else { return };
        let (id, a) = (t.id, a.for_harness(t.harness));
        let Some(r) = self.running.get(&id) else { return };
        // Read only can't be reached mid-turn: plan mode would refuse without the prompt tool.
        if a == Access::ReadOnly { return; }
        r.set_access(a);
        let covered: Vec<u64> = t.asks.iter().filter(|p| a.allows(&p.ask.tool)).map(|p| p.item).collect();
        for item in covered { self.answer(id, item, Decision::Allow); }
    }

    /// Answers the approval on row `item` of thread `id`, and sends the answer to the CLI.
    pub fn answer(&mut self, id: ThreadId, item: u64, d: Decision) {
        let Some(t) = self.thread_mut(id) else { return };
        let Some(i) = t.asks.iter().position(|p| p.item == item) else { return };
        let p = t.asks.remove(i);
        t.rev += 1;
        if let Some(row) = t.items.iter_mut().find(|x| x.id == item) { row.rev += 1; }
        if let Some(r) = self.running.get(&id) { r.answer(&p.ask, &d); }
        self.draft.focused = true;
    }

    /// Answers the selected thread's oldest approval (the Allow shortcuts). False if none waits.
    pub fn answer_first(&mut self, d: Decision) -> bool {
        let Some(t) = self.thread() else { return false };
        let Some(item) = t.asks.first().map(|p| p.item) else { return false };
        let id = t.id;
        self.answer(id, item, d);
        true
    }

    /// The person stops the running turn (Esc or the stop button). The queue waits.
    pub fn stop(&mut self, id: ThreadId) {
        if let Some(r) = self.running.get(&id) {
            r.stop();
            if let Some(t) = self.thread_mut(id) { t.stop_requested = true; }
        }
    }

    /// Lands every event that arrived since the last frame, and starts queued messages whose
    /// turn just ended; true if anything changed.
    pub fn drain(&mut self, tx: &mut Tx) -> bool {
        let mut changed = self.drain_usage();
        while let Ok((h, list)) = self.discovered.try_recv() {
            changed = true;
            self.land_models(h, list);
        }
        while let Ok((id, made)) = self.worktrees.try_recv() {
            changed = true;
            self.land_worktree(id, made);
        }
        while let Ok(l) = self.sessions.try_recv() {
            changed = true;
            self.land_sessions(&l.project, l.claude);
            self.land_kept(&l.project, l.kept);
        }
        while let Ok((id, e)) = self.events.try_recv() {
            changed = true;
            self.apply(id, e);
        }
        for id in std::mem::take(&mut self.ready) {
            if let Some(t) = self.thread_mut(id).filter(|t| !t.queue.is_empty()) {
                let q = t.queue.remove(0);
                self.start_turn(id, q, tx);
                changed = true;
            }
        }
        if !self.running.is_empty() {
            // The "Working… 12s" counter.
            tx.wake_after(Duration::from_secs(1));
        }
        changed
    }

    pub(crate) fn apply(&mut self, id: ThreadId, e: Event) {
        if matches!(e, Event::Ask(_) | Event::Exited { .. }) { self.wake(id); }
        if let Event::RateLimit(info) = &e {
            if let Some(l) = self.limits.as_mut() && l.apply_event(info) {
                let _ = self.prefs.set(LIMITS_KEY, &l.to_json());
            }
            return;
        }
        // Remembered for the model, so the next thread on it starts with the right size.
        if let Event::ContextWindow(w) = e
            && let Some(m) = self.threads.iter().find(|t| t.id == id).and_then(|t| t.resolved_model.clone())
            && self.context_windows.get(&m) != Some(&w) {
                self.context_windows.insert(m, w);
                let _ = self.prefs.set(CONTEXT_WINDOWS_KEY, &self.context_windows);
            }
        // A Claude turn spent from the plan and wrote to its transcript: read both again.
        if matches!(e, Event::Exited { .. }) && self.threads.iter().any(|t| t.id == id && t.harness == Harness::Claude) {
            self.refresh_limits();
            self.rescan_usage();
        }
        let selected = self.selected == Some(id);
        let fresh = self.id();
        let exited = matches!(e, Event::Exited { .. });
        self.apply_to(id, e, selected, fresh);
        if exited { self.keep(id); }
    }

    fn apply_to(&mut self, id: ThreadId, e: Event, selected: bool, fresh: u64) {
        let Some(t) = self.thread_mut(id) else { return };
        t.rev += 1;
        match e {
            Event::Session(s) => t.session = Some(s),
            Event::Model(m) => t.resolved_model = Some(m),
            Event::Thinking | Event::RateLimit(_) => {}
            Event::ContextUsed(n) => t.context = Some(n),
            Event::ContextWindow(w) => t.context_window = Some(w),
            Event::TextDelta(d) => match t.items.last_mut() {
                Some(Item { kind: Kind::Assistant { text, streaming: true }, rev, .. }) => {
                    text.push_str(&d);
                    *rev += 1;
                }
                _ => t.items.push(Item { id: fresh, rev: 0, kind: Kind::Assistant { text: d, streaming: true } }),
            },
            Event::Text(s) => {
                end_stream(t);
                if !s.trim().is_empty() { t.items.push(Item { id: fresh, rev: 0, kind: Kind::Assistant { text: s, streaming: false } }); }
            }
            Event::ToolStart { id: call, name, detail } => {
                end_stream(t);
                // An approval can arrive first and make the row itself.
                if t.tool_row(&call).is_none() {
                    t.items.push(Item { id: fresh, rev: 0, kind: Kind::Tool { call, name, detail, state: ToolState::Running, output: String::new() } });
                }
            }
            Event::Ask(ask) => {
                end_stream(t);
                let call = ask.tool_use_id.clone().unwrap_or_else(|| format!("ask-{}", ask.request_id));
                let item = match t.tool_row(&call) {
                    Some(row) => {
                        row.rev += 1;
                        row.id
                    }
                    None => {
                        let detail = harness::tool_detail(&ask.input);
                        t.items.push(Item { id: fresh, rev: 0, kind: Kind::Tool { call, name: ask.tool.clone(), detail, state: ToolState::Running, output: String::new() } });
                        fresh
                    }
                };
                let tool = ask.tool.clone();
                t.asks.push(Pending { item, ask });
                t.updated = SystemTime::now();
                if !selected { t.unread = true; }
                self.alerts.push((id, Alert::Approval(tool)));
            }
            Event::AskCancelled { request_id } => {
                if let Some(i) = t.asks.iter().position(|p| p.ask.request_id == request_id) {
                    let item = t.asks.remove(i).item;
                    if let Some(row) = t.items.iter_mut().find(|x| x.id == item) { row.rev += 1; }
                }
            }
            Event::ToolEnd { id: done, ok, output: out } => {
                if let Some(item) = t.items.iter_mut().rev().find(|i| matches!(&i.kind, Kind::Tool { call, .. } if *call == done)) {
                    if let Kind::Tool { state, output, .. } = &mut item.kind {
                        *state = if ok { ToolState::Ok } else { ToolState::Failed };
                        *output = out;
                    }
                    item.rev += 1;
                }
            }
            Event::Error(m) => {
                end_stream(t);
                t.failed = true;
                t.items.push(Item { id: fresh, rev: 0, kind: Kind::Error(m) });
            }
            Event::Done { cost_usd, error } => {
                end_stream(t);
                if let Some(m) = error {
                    t.failed = true;
                    t.items.push(Item { id: fresh, rev: 0, kind: Kind::Error(m) });
                }
                let secs = t.turn_started.map_or(0.0, |s| s.elapsed().as_secs_f64());
                let fresh2 = fresh + 1_000_000_000;
                t.items.push(Item { id: fresh2, rev: 0, kind: Kind::TurnEnd { secs, cost_usd, stopped: false } });
            }
            Event::Exited { code, stderr } => {
                end_stream(t);
                let ended = matches!(t.items.last(), Some(Item { kind: Kind::TurnEnd { .. }, .. }));
                if !ended {
                    let stopped = code.is_none();
                    if !stopped && code != Some(0) {
                        let msg = harness::first_line(stderr.trim().lines().last().unwrap_or(""), 400);
                        let msg = if msg.is_empty() { format!("{} exited with status {}", t.harness.binary(), code.unwrap_or(-1)) } else { msg };
                        t.failed = true;
                        t.items.push(Item { id: fresh, rev: 0, kind: Kind::Error(msg) });
                    }
                    let secs = t.turn_started.map_or(0.0, |s| s.elapsed().as_secs_f64());
                    t.items.push(Item { id: fresh + 1_000_000_000, rev: 0, kind: Kind::TurnEnd { secs, cost_usd: None, stopped } });
                }
                // Nothing is listening for answers any more; the rows below are bumped as
                // they fail.
                t.asks.clear();
                for i in &mut t.items {
                    if let Kind::Tool { state: s @ ToolState::Running, .. } = &mut i.kind {
                        *s = ToolState::Failed;
                        i.rev += 1;
                    }
                }
                t.busy = false;
                t.unread = !selected;
                t.updated = SystemTime::now();
                // Steering always sends next; otherwise the queue goes on after a turn that
                // finished cleanly, and waits after a stop or a failure.
                let steering = std::mem::take(&mut t.steering);
                let stopped_by_user = std::mem::take(&mut t.stop_requested);
                let advance = !t.queue.is_empty() && (steering || (!stopped_by_user && code == Some(0)));
                // A turn you stopped isn't news, and one with a queue behind it isn't over.
                let alert = (!stopped_by_user && !advance).then(|| if t.failed { Alert::Failed } else { Alert::Done });
                self.running.remove(&id);
                if advance { self.ready.push(id); }
                if let Some(a) = alert { self.alerts.push((id, a)); }
            }
        }
    }
}

fn models_key(h: Harness) -> String {
    format!("models.{}", h.id())
}

fn end_stream(t: &mut Thread) {
    if let Some(Item { kind: Kind::Assistant { streaming, .. }, rev, .. }) = t.items.last_mut()
        && *streaming {
            *streaming = false;
            *rev += 1;
        }
}

/// Headless scenes: a finished conversation and one still working, for snapshots.
pub mod scenes {
    use super::*;

    pub fn demo(s: &mut App) {
        let push = |s: &mut App, t: &mut Thread, kind: Kind| {
            let id = s.id();
            t.items.push(Item { id, rev: 0, kind });
        };
        let mut t = Thread::new(100, "Add a retry with backoff to the fetch client".into(), Harness::Claude, "opus".into());
        t.resolved_model = Some("claude-opus-5-5".into());
        t.session = Some("demo".into());
        (t.context, t.context_window) = (Some(64_300), Some(200_000));
        push(s, &mut t, Kind::User("Add a retry with exponential backoff to the fetch client, and cap it at 5 attempts.".into()));
        push(s, &mut t, Kind::Assistant { text: "I'll look at how the client is structured first.".into(), streaming: false });
        push(s, &mut t, Kind::Tool { call: "1".into(), name: "Grep".into(), detail: "fn fetch".into(), state: ToolState::Ok, output: "src/client.rs:42: pub async fn fetch(&self, req: Request)".into() });
        push(s, &mut t, Kind::Tool { call: "2".into(), name: "Read".into(), detail: "src/client.rs".into(), state: ToolState::Ok, output: String::new() });
        push(s, &mut t, Kind::Tool { call: "3".into(), name: "Edit".into(), detail: "src/client.rs".into(), state: ToolState::Ok, output: String::new() });
        push(s, &mut t, Kind::Tool { call: "4".into(), name: "Bash".into(), detail: "cargo test -p client retry".into(), state: ToolState::Ok, output: "running 3 tests\ntest retry::gives_up_after_five ... ok\ntest retry::backs_off ... ok\ntest retry::succeeds_first_try ... ok\n\ntest result: ok. 3 passed".into() });
        push(s, &mut t, Kind::Assistant { text: DEMO_ANSWER.into(), streaming: false });
        push(s, &mut t, Kind::TurnEnd { secs: 48.3, cost_usd: Some(0.1842), stopped: false });
        let mut w = Thread::new(101, "Why is the settings window slow to open?".into(), Harness::Claude, models::DEFAULT.into());
        w.busy = true;
        (w.context, w.context_window) = (Some(131_500), Some(200_000));
        w.turn_started = Instant::now().checked_sub(Duration::from_secs(34));
        push(s, &mut w, Kind::User("Why is the settings window slow to open?".into()));
        push(s, &mut w, Kind::Tool { call: "1".into(), name: "Bash".into(), detail: "cargo build --profile perf -p settings".into(), state: ToolState::Running, output: String::new() });
        let mut c = Thread::new(102, "Write a changelog entry for the release".into(), Harness::Codex, models::DEFAULT.into());
        c.unread = true;
        push(s, &mut c, Kind::User("Write a changelog entry for the release".into()));
        let mut f = Thread::new(103, "Fix the flaky file-watcher test".into(), Harness::Claude, models::DEFAULT.into());
        f.failed = true;
        f.unread = true;
        push(s, &mut f, Kind::User("Fix the flaky file-watcher test".into()));
        push(s, &mut f, Kind::Error("claude exited with status 1".into()));
        let mut settled = Vec::new();
        for (i, title) in ["Bump the Rust toolchain to 1.92", "Rename Workspace to Project everywhere", "Why does the tab strip flicker on resize?"].into_iter().enumerate() {
            let mut x = Thread::new(110 + i as u64, title.into(), Harness::Claude, models::DEFAULT.into());
            x.settled = true;
            x.updated = SystemTime::now() - Duration::from_secs(3600 * (5 + 20 * i as u64));
            settled.push(x);
        }
        let ago = |m: u64| SystemTime::now() - Duration::from_secs(60 * m);
        (w.updated, t.updated, c.updated, f.updated) = (ago(1), ago(4), ago(2), ago(12));
        // Two stand-in projects, so the snapshots show none of yours.
        let (api, desktop) = (PathBuf::from("/demo/api-client"), PathBuf::from("/demo/desktop"));
        s.projects = vec![Project::new(api.clone()), Project::new(desktop.clone())];
        s.current = Some(api.clone());
        (t.project, c.project, w.project, f.project) = (api.clone(), api.clone(), desktop.clone(), desktop);
        for x in &mut settled { x.project = api.clone(); }
        s.threads = vec![w, t, c, f];
        s.threads.extend(settled);
        s.settled_open = true;
        s.selected = Some(100);
        s.draft.focused = false;
        stand_in_usage(s);
    }

    /// One thread of `turns` finished turns (six rows each), the demo's turn repeated.
    pub fn long_with(s: &mut App, turns: usize) {
        demo(s);
        let i = s.threads.iter().position(|t| t.id == 100).unwrap();
        let mut t = s.threads.remove(i);
        let one: Vec<Kind> = t.items.drain(..).map(|i| i.kind).collect();
        for n in 0..turns {
            for k in &one {
                let kind = match k {
                    Kind::User(m) => Kind::User(format!("{m} (#{n})")),
                    Kind::Assistant { text, .. } => Kind::Assistant { text: text.clone(), streaming: false },
                    Kind::Tool { call, name, detail, state, output } => Kind::Tool { call: format!("{n}-{call}"), name: name.clone(), detail: detail.clone(), state: *state, output: output.clone() },
                    Kind::Error(m) => Kind::Error(m.clone()),
                    Kind::TurnEnd { secs, cost_usd, stopped } => Kind::TurnEnd { secs: *secs, cost_usd: *cost_usd, stopped: *stopped },
                };
                let id = s.id();
                t.items.push(Item { id, rev: 0, kind });
            }
        }
        t.title = format!("A long thread ({turns} turns)");
        s.threads.insert(i, t);
    }

    pub fn long(s: &mut App) {
        long_with(s, 500);
    }

    pub fn picker(s: &mut App) {
        empty(s);
        s.model = "opus".into();
        s.effort = "high".into();
        s.open_picker();
    }

    pub fn effort(s: &mut App) {
        empty(s);
        s.effort = "high".into();
        s.open_picker();
    }

    pub fn settings(s: &mut App) {
        demo(s);
        s.settings = true;
    }

    /// Settings › New threads, with Claude Code set to start on Opus at high effort.
    pub fn settings_new_threads(s: &mut App) {
        settings(s);
        s.settings_page = crate::settings::Page::NewThreads;
        s.defaults.models.insert(Harness::Claude, "opus".into());
        s.defaults.efforts.insert(Harness::Claude, "high".into());
    }

    pub fn settings_chat(s: &mut App) {
        settings(s);
        s.settings_page = crate::settings::Page::Chat;
    }

    /// A Max plan part-way through its session, and 90 days of made-up spend, held still: the
    /// live reads `setup` starts never land (their channels stand in for running ones).
    pub fn stand_in_usage(s: &mut App) {
        use crate::harness::limits::Window;
        let now = SystemTime::now();
        let h = |n: u64| Duration::from_secs(n * 3600);
        s.limits = Some(Limits {
            plan: Some("Max".into()),
            available: true,
            windows: vec![
                Window { id: "session".into(), label: "Session".into(), used: 78.0, resets: Some(now + h(1) + Duration::from_secs(58 * 60)), length: Some(h(5)), severity: "normal".into() },
                Window { id: "weekly_all".into(), label: "Weekly".into(), used: 41.0, resets: Some(now + h(3 * 24 + 4)), length: Some(h(7 * 24)), severity: "normal".into() },
                Window { id: "weekly_scoped:Fable".into(), label: "Weekly · Fable".into(), used: 12.0, resets: Some(now + h(3 * 24 + 4)), length: Some(h(7 * 24)), severity: "normal".into() },
            ],
            spend: None,
            checked: now - Duration::from_secs(90),
        });
        s.limits_rx = Some(channel().1);
        s.usage_rx = Some(channel().1);
        s.prices_rx = Some(channel().1);
        s.prices = Arc::new(usage::Prices::built_in());
        let today = usage::today();
        let mut rows = Vec::new();
        for back in 0..90u64 {
            let day = today - chrono::Days::new(back);
            // Weekdays busier than weekends, with some wobble.
            let busy = if matches!(chrono::Datelike::weekday(&day), chrono::Weekday::Sat | chrono::Weekday::Sun) { 0.3 } else { 1.0 };
            let wobble = ((back * 37 + 11) % 17) as f64 / 17.0;
            for (model, project, share) in [("claude-opus-5-5", "rune", 1.0), ("claude-opus-5-5", "api-server", 0.45), ("claude-sonnet-5-5", "docs-site", 0.2), ("claude-haiku-5-5", "rune", 0.05)] {
                let scale = busy * share * (0.4 + wobble) * if back < 30 { 1.0 } else { 0.6 };
                let n = |base: f64| (base * scale) as u64;
                let tokens = usage::Tokens { input: n(40_000.0), output: n(180_000.0), cache_read: n(9_000_000.0), cache_write: n(150_000.0), cache_write_1h: n(250_000.0) };
                let rate = |input: f64, output: f64| tokens.input as f64 * input + tokens.output as f64 * output + tokens.cache_read as f64 * input / 20.0 + (tokens.cache_write as f64 * 1.25 + tokens.cache_write_1h as f64 * 2.0) * input;
                let cost = match model { "claude-opus-5-5" => rate(4e-6, 2e-5), "claude-sonnet-5-5" => rate(2e-6, 1e-5), _ => rate(1e-7, 5e-7) };
                rows.push(usage::Row { day, model: model.into(), project: project.into(), tokens, cost, priced: true, messages: (220.0 * scale) as u32 });
            }
        }
        s.usage = usage::Report { rows, files: 214, scanned: Some(now), took: Duration::from_millis(38) };
    }

    pub fn usage(s: &mut App) {
        demo(s);
        stand_in_usage(s);
        s.usage_days = 30;
        s.usage_open = true;
    }

    /// The working thread with two follow-ups queued and a third being typed.
    pub fn queue(s: &mut App) {
        demo(s);
        s.selected = Some(101);
        for text in ["Also check whether the font atlas is rebuilt on open.", "Then write up what you find in NOTES.md."] {
            let id = s.id();
            let q = Queued { id, text: text.into(), model: models::DEFAULT.into(), effort: harness::DEFAULT_EFFORT.into(), access: Access::Edits, images: Vec::new() };
            s.thread_mut(101).unwrap().queue.push(q);
        }
        s.draft = TextEdit::new("Skip the release build, debug is fine");
    }

    /// The working thread's Bash call waiting for approval.
    pub fn approval(s: &mut App) {
        demo(s);
        s.selected = Some(101);
        let t = s.thread_mut(101).unwrap();
        let item = t.items.iter().find(|i| matches!(&i.kind, Kind::Tool { call, .. } if call == "1")).unwrap().id;
        let ask = AskRequest {
            request_id: "r1".into(),
            tool_use_id: Some("1".into()),
            tool: "Bash".into(),
            input: serde_json::json!({"command": "cargo build --profile perf -p settings", "description": "Build the settings crate with the perf profile"}),
            suggestions: vec![serde_json::json!({"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "cargo build:*"}], "behavior": "allow", "destination": "localSettings"})],
            reason: None,
        };
        t.asks.push(Pending { item, ask });
    }

    /// A screenshot attached to the composer of the demo thread.
    pub fn attach(s: &mut App) {
        demo(s);
        let shot = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shots/demo-light.png");
        s.attach(&[shot]);
        s.draft = TextEdit::new("Why is the sidebar wider in this one?");
    }

    /// The demo with the sidebar shut: the toolbar starts past the traffic lights.
    pub fn collapsed(s: &mut App) {
        demo(s);
        s.sidebar.set_open(false);
    }

    pub fn empty(s: &mut App) {
        demo(s);
        s.threads.retain(|t| t.settled);
        s.selected = None;
        s.draft.focused = true;
    }

    /// The new-thread screen with the headline's project picker open.
    pub fn projects(s: &mut App) {
        demo(s);
        s.new_thread();
        s.open_project_picker();
    }

    const DEMO_ANSWER: &str = "Done. `fetch` now retries transient failures with exponential backoff:\n\n\
```rust\npub async fn fetch(&self, req: Request) -> Result<Response> {\n    let mut delay = Duration::from_millis(100);\n    for attempt in 1..=MAX_ATTEMPTS {\n        match self.send(req.clone()).await {\n            Err(e) if e.is_transient() && attempt < MAX_ATTEMPTS => {\n                sleep(delay).await;\n                delay *= 2;\n            }\n            other => return other,\n        }\n    }\n    unreachable!()\n}\n```\n\n\
| Attempt | Delay before |\n| --- | ---: |\n| 1 | 0 ms |\n| 2 | 100 ms |\n| 3 | 200 ms |\n| 4 | 400 ms |\n| 5 | 800 ms |\n\n\
- Only **transient** errors retry (timeouts, 5xx, connection resets); 4xx fail at once.\n\
- The three new tests in `retry.rs` pass.\n";
}
