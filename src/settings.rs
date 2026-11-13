//! What Settings stores beyond the look: the defaults a new thread starts from (harness, and
//! each harness's model and effort, the access level, and whether it runs in a new worktree),
//! how the prompt sends, and which of Settings' pages is showing. `ui::settings` draws the pages.
//!
//! A new thread (⌘N, and the first screen at launch) starts from these defaults; switching
//! harness on the new-thread screen takes that harness's model and effort. Changing a
//! default while the new-thread screen is showing applies it there at once.

use std::collections::HashMap;

use rune::{Icon, Tx};
use rune_theme_opencode::icons;

use crate::app::App;
use crate::harness::{self, Access, Harness, models};

const HARNESS_KEY: &str = "new-thread.harness";
const MODELS_KEY: &str = "new-thread.models";
const EFFORTS_KEY: &str = "new-thread.efforts";
const ACCESS_KEY: &str = "new-thread.access";
const WORKTREE_KEY: &str = "new-thread.worktree";
/// Before it was a setting, the composer's Local / New worktree chip was remembered here.
const OLD_WORKTREE_KEY: &str = "composer.worktree";
const SEND_KEY: &str = "chat.send";
const FOLLOW_UP_KEY: &str = "chat.follow-up";
const PEER_MESSAGES_KEY: &str = "agents.peer-messages";
const SETTLE_AFTER_KEY: &str = "inbox.settle-after-days";
const SETTLE_MERGED_KEY: &str = "inbox.settle-merged";

/// How many days a thread sits untouched before it settles on its own, as T3 Code's default.
pub const SETTLE_AFTER_DAYS: u32 = 7;
/// The choices Settings › Chat offers; 0 is never.
pub const SETTLE_CHOICES: [u32; 6] = [1, 3, 7, 14, 30, 0];

/// "7 days", "1 day", "Never".
pub fn settle_label(days: u32) -> String {
    match days {
        0 => "Never".into(),
        1 => "1 day".into(),
        n => format!("{n} days"),
    }
}

/// Settings' pages, in the order the rail lists them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Appearance,
    NewThreads,
    Chat,
    Agents,
    Skills,
    Shortcuts,
    Advanced,
}

pub const PAGES: [Page; 7] = [Page::Appearance, Page::NewThreads, Page::Chat, Page::Agents, Page::Skills, Page::Shortcuts, Page::Advanced];

impl Page {
    pub fn id(self) -> &'static str {
        match self {
            Page::Appearance => "appearance",
            Page::NewThreads => "new-threads",
            Page::Chat => "chat",
            Page::Agents => "agents",
            Page::Skills => "skills",
            Page::Shortcuts => "shortcuts",
            Page::Advanced => "advanced",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Page::Appearance => "Appearance",
            Page::NewThreads => "New threads",
            Page::Chat => "Chat",
            Page::Agents => "Agents",
            Page::Skills => "Skills",
            Page::Shortcuts => "Keyboard shortcuts",
            Page::Advanced => "Advanced",
        }
    }

    pub fn icon(self) -> &'static Icon {
        match self {
            Page::Appearance => &icons::APPEARANCE,
            Page::NewThreads => &icons::NEW_SESSION,
            Page::Chat => &icons::SPEECH_BUBBLE,
            Page::Agents => &icons::PROVIDERS,
            Page::Skills => &icons::POST_SKILL,
            Page::Shortcuts => &icons::KEYBOARD,
            Page::Advanced => &icons::SLIDERS,
        }
    }
}

/// Which key sends the prompt; the other inserts a newline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SendKey {
    /// Return sends, Shift-Return is a newline.
    #[default]
    Return,
    /// ⌘Return sends, Return is a newline: for writing long prompts.
    ModReturn,
}

impl SendKey {
    fn id(self) -> &'static str {
        match self { SendKey::Return => "return", SendKey::ModReturn => "mod-return" }
    }
}

/// What sending does while the thread's turn is still running. The Steer shortcut (⌘↩, or
/// ⇧⌘↩ when ⌘↩ sends) does the other one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FollowUp {
    /// Wait for the turn to end, then send.
    #[default]
    Queue,
    /// Stop the turn and send now, on the same session.
    Steer,
}

impl FollowUp {
    fn id(self) -> &'static str {
        match self { FollowUp::Queue => "queue", FollowUp::Steer => "steer" }
    }
}

/// What a new thread could already do before this was a setting.
pub const DEFAULT_ACCESS: Access = Access::Edits;

/// The new-thread defaults and the chat behaviour, as saved.
#[derive(Clone, Debug)]
pub struct Prefs {
    /// `None`: the first harness that's installed.
    pub harness: Option<Harness>,
    /// A `ModelOpt::value` per harness; missing is the CLI's own default.
    pub models: HashMap<Harness, String>,
    /// An effort per harness; missing is `harness::DEFAULT_EFFORT`.
    pub efforts: HashMap<Harness, String>,
    pub access: Access,
    /// In a git repository, a new thread runs in a new worktree rather than the project's folder.
    pub worktree: bool,
    pub send: SendKey,
    pub follow_up: FollowUp,
    /// Claude Code threads take messages from your other Claude Code sessions (`Turn::peer_messages`).
    pub peer_messages: bool,
    /// Days untouched before a thread settles on its own; 0 never.
    pub settle_after: u32,
    /// A worktree thread settles once its branch is merged, or its pull request is merged or
    /// closed, as T3 Code's settle on merge.
    pub settle_merged: bool,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs { harness: None, models: HashMap::new(), efforts: HashMap::new(), access: DEFAULT_ACCESS, worktree: false, send: SendKey::Return, follow_up: FollowUp::Queue, peer_messages: true, settle_after: SETTLE_AFTER_DAYS, settle_merged: true }
    }
}

fn by_id(id: &str) -> Option<Harness> {
    harness::ALL.into_iter().find(|h| h.id() == id)
}

fn per_harness(saved: Option<HashMap<String, String>>) -> HashMap<Harness, String> {
    saved.unwrap_or_default().into_iter().filter_map(|(id, v)| Some((by_id(&id)?, v))).collect()
}

fn by_harness_id(map: &HashMap<Harness, String>) -> HashMap<String, String> {
    map.iter().map(|(h, v)| (h.id().to_owned(), v.clone())).collect()
}

impl Prefs {
    pub fn load(prefs: &rune::prefs::Prefs) -> Prefs {
        Prefs {
            harness: prefs.get::<String>(HARNESS_KEY).and_then(|id| by_id(&id)),
            models: per_harness(prefs.get(MODELS_KEY)),
            efforts: per_harness(prefs.get(EFFORTS_KEY)),
            access: prefs.get::<String>(ACCESS_KEY).and_then(|l| Access::from_label(&l)).unwrap_or(DEFAULT_ACCESS),
            worktree: prefs.get::<bool>(WORKTREE_KEY).or_else(|| prefs.get::<bool>(OLD_WORKTREE_KEY)).unwrap_or(false),
            send: match prefs.get::<String>(SEND_KEY).as_deref() { Some("mod-return") => SendKey::ModReturn, _ => SendKey::Return },
            follow_up: match prefs.get::<String>(FOLLOW_UP_KEY).as_deref() { Some("steer") => FollowUp::Steer, _ => FollowUp::Queue },
            peer_messages: prefs.get::<bool>(PEER_MESSAGES_KEY).unwrap_or(true),
            settle_after: prefs.get::<u32>(SETTLE_AFTER_KEY).filter(|d| *d <= 90).unwrap_or(SETTLE_AFTER_DAYS),
            settle_merged: prefs.get::<bool>(SETTLE_MERGED_KEY).unwrap_or(true),
        }
    }

    fn save(&self, prefs: &mut rune::prefs::Prefs) {
        match self.harness {
            Some(h) => { let _ = prefs.set(HARNESS_KEY, h.id()); }
            None => { let _ = prefs.remove(HARNESS_KEY); }
        }
        let _ = prefs.set(MODELS_KEY, &by_harness_id(&self.models));
        let _ = prefs.set(EFFORTS_KEY, &by_harness_id(&self.efforts));
        let _ = prefs.set(ACCESS_KEY, self.access.label());
        let _ = prefs.set(WORKTREE_KEY, &self.worktree);
        let _ = prefs.remove(OLD_WORKTREE_KEY);
        let _ = prefs.set(SEND_KEY, self.send.id());
        let _ = prefs.set(FOLLOW_UP_KEY, self.follow_up.id());
        let _ = prefs.set(PEER_MESSAGES_KEY, &self.peer_messages);
        let _ = prefs.set(SETTLE_AFTER_KEY, &self.settle_after);
        let _ = prefs.set(SETTLE_MERGED_KEY, &self.settle_merged);
    }
}

impl App {
    /// The harness a new thread starts with: the chosen one if it's installed, else the first
    /// that is.
    pub fn default_harness(&self) -> Harness {
        self.defaults.harness.filter(|h| self.binary(*h).is_some())
            .or_else(|| harness::ALL.into_iter().find(|h| self.binary(*h).is_some()))
            .unwrap_or(Harness::Claude)
    }

    pub fn default_model(&self, h: Harness) -> String {
        self.defaults.models.get(&h).cloned().unwrap_or_else(|| models::DEFAULT.into())
    }

    /// The saved effort, if `h` still has that level.
    pub fn default_effort(&self, h: Harness) -> String {
        self.defaults.efforts.get(&h).filter(|e| h.efforts().contains(&e.as_str())).cloned().unwrap_or_else(|| harness::DEFAULT_EFFORT.into())
    }

    /// Puts the composer back to the new-thread defaults.
    pub fn apply_defaults(&mut self) {
        self.harness = self.default_harness();
        self.model = self.default_model(self.harness);
        self.effort = self.default_effort(self.harness);
        self.access = self.defaults.access;
        self.worktree_mode = self.defaults.worktree;
    }

    /// Whether the composer is starting a new thread, so a changed default shows there now.
    fn composing_new(&self) -> bool {
        self.selected.is_none()
    }

    fn save_defaults(&mut self) {
        self.defaults.save(&mut self.prefs);
    }

    pub fn set_default_harness(&mut self, h: Harness) {
        self.defaults.harness = Some(h);
        self.save_defaults();
        if self.composing_new() && self.binary(h).is_some() && self.harness != h { self.choose_harness(h); }
    }

    /// `label` as the model picker lists it ("Default" clears the choice).
    pub fn set_default_model(&mut self, h: Harness, label: &str) {
        let value = self.models(h).iter().find(|m| m.label == label).map_or_else(|| label.to_owned(), |m| m.value.clone());
        if value == models::DEFAULT { self.defaults.models.remove(&h); } else { self.defaults.models.insert(h, value.clone()); }
        self.save_defaults();
        if self.composing_new() && self.harness == h { self.model = value; }
    }

    pub fn set_default_effort(&mut self, h: Harness, effort: &str) {
        if effort == harness::DEFAULT_EFFORT { self.defaults.efforts.remove(&h); } else { self.defaults.efforts.insert(h, effort.to_owned()); }
        self.save_defaults();
        if self.composing_new() && self.harness == h { self.effort = effort.to_owned(); }
    }

    pub fn set_default_access(&mut self, a: Access) {
        self.defaults.access = a;
        self.save_defaults();
        if self.composing_new() { self.access = a; }
    }

    /// Where a new thread in a git repository runs: a new worktree (`true`) or the project's folder.
    pub fn set_default_worktree(&mut self, on: bool) {
        self.defaults.worktree = on;
        self.save_defaults();
        if self.composing_new() { self.worktree_mode = on; }
    }

    /// Clears every new-thread default back to the CLI's own choices.
    pub fn reset_defaults(&mut self) {
        let d = &self.defaults;
        let (send, follow_up, settle_after, settle_merged) = (d.send, d.follow_up, d.settle_after, d.settle_merged);
        self.defaults = Prefs { send, follow_up, settle_after, settle_merged, ..Prefs::default() };
        self.save_defaults();
        if self.composing_new() { self.apply_defaults(); }
    }

    pub fn defaults_changed(&self) -> bool {
        let d = &self.defaults;
        d.harness.is_some() || !d.models.is_empty() || !d.efforts.is_empty() || d.access != DEFAULT_ACCESS || d.worktree
    }

    /// Threads untouched this long settle on their own; 0 turns it off.
    pub fn set_settle_after(&mut self, days: u32) {
        self.defaults.settle_after = days;
        self.save_defaults();
        self.next_sweep = 0.0;
    }

    pub fn set_settle_merged(&mut self, on: bool) {
        self.defaults.settle_merged = on;
        self.save_defaults();
        self.next_merge_check = 0.0;
    }

    pub fn set_send_key(&mut self, k: SendKey) {
        self.defaults.send = k;
        self.save_defaults();
    }

    pub fn set_follow_up(&mut self, f: FollowUp) {
        self.defaults.follow_up = f;
        self.save_defaults();
    }

    /// Settings › Agents: whether Claude Code threads take your other sessions' messages, from
    /// each thread's next turn.
    pub fn set_peer_messages(&mut self, on: bool) {
        self.defaults.peer_messages = on;
        self.save_defaults();
    }

    /// The send key (Return, or ⌘Return when that sends): a new turn, or while one runs, what
    /// Settings › Chat says follow-ups do.
    pub fn submit(&mut self, tx: &mut Tx) {
        match self.defaults.follow_up {
            FollowUp::Queue => self.send(tx),
            FollowUp::Steer => self.steer(tx),
        }
    }

    /// The Steer shortcut: the other of queue and steer.
    pub fn submit_other(&mut self, tx: &mut Tx) {
        match self.defaults.follow_up {
            FollowUp::Queue => self.steer(tx),
            FollowUp::Steer => self.send(tx),
        }
    }

    /// The prompt field's own keys for the send setting, before the shortcut table: with ⌘↩
    /// sending, plain ↩ is a newline and ⇧⌘↩ is the other of queue and steer.
    pub fn prompt_key(&mut self, ev: &rune::KeyEvent, tx: &mut Tx) -> bool {
        if self.defaults.send != SendKey::ModReturn || ev.key != rune::Key::Enter || self.draft.is_composing() { return false; }
        let m = ev.mods;
        if m.cmd && !m.alt && !m.ctrl {
            if tx.key_repeat() { return true; }
            if m.shift { self.submit_other(tx) } else { self.submit(tx) }
            return true;
        }
        if m.shift || m.alt || m.cmd || m.ctrl { return false; }
        // Option-Return is the field's own newline, whatever block the caret is in.
        self.draft.key(&rune::KeyEvent { key: rune::Key::Enter, mods: rune::Mods { alt: true, ..rune::Mods::default() } }, None);
        true
    }

    /// How the send and the other key read: ("↩", "⌘↩") or ("⌘↩", "⇧⌘↩").
    pub fn send_labels(&self) -> (String, String) {
        let steer = self.keys.label(crate::shortcuts::Cmd::Steer);
        match self.defaults.send {
            SendKey::Return => ("↩".into(), steer),
            SendKey::ModReturn => (mod_label("↩"), if cfg!(target_os = "macos") { "⇧⌘↩".into() } else { "Ctrl+Shift+↩".into() }),
        }
    }

    pub fn show_settings_page(&mut self, p: Page) {
        self.settings_page = p;
        if p == Page::Skills { self.scan_skills(); }
        self.open_menu = None;
        self.recording = None;
        self.key_error = None;
    }

    /// Shows `prefs.json` in Finder (its folder elsewhere).
    pub fn reveal_prefs(&self) {
        let Some(path) = self.prefs.path() else { return };
        #[cfg(target_os = "macos")]
        let spawned = std::process::Command::new("open").arg("-R").arg(path).spawn();
        #[cfg(target_os = "windows")]
        let spawned = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let spawned = std::process::Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn();
        let _ = spawned;
    }

    /// Looks for the agent CLIs again (one was installed since launch, say), and asks any
    /// found for their models.
    pub fn rescan_clis(&mut self) {
        self.installed = harness::detect();
        self.discover_models();
        if self.composing_new() && self.binary(self.harness).is_none() { self.apply_defaults(); }
    }
}

/// `key` with the platform's primary modifier: "⌘↩" on a Mac, "Ctrl+↩" elsewhere.
fn mod_label(key: &str) -> String {
    if cfg!(target_os = "macos") { format!("⌘{key}") } else { format!("Ctrl+{key}") }
}
