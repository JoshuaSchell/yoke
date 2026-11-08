//! Keyboard shortcuts: every command the app has, its default chord, the user's overrides
//! (saved in prefs), and recording a new chord from Settings.
//!
//! Key handling, tooltips and the Settings list all read this one table, so a rebinding shows
//! everywhere at once. Chords are Rune's (`rune::Keybind`, written like "mod+shift+b"), so they
//! follow the platform's primary modifier and the user's keyboard layout.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use rune::{Chord, KeyEvent, Keybind, Tx};

use crate::app::App;
use crate::harness::Decision;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cmd {
    NewThread,
    OpenFolder,
    ToggleSidebar,
    Settings,
    Usage,
    FrameStats,
    FocusPrompt,
    PrevThread,
    NextThread,
    SettleThread,
    DeleteThread,
    Stop,
    Steer,
    Allow,
    AlwaysAllow,
    HarnessMenu,
    ModelMenu,
    EffortMenu,
    AccessMenu,
}

pub const ALL: [Cmd; 19] = [
    Cmd::NewThread,
    Cmd::OpenFolder,
    Cmd::ToggleSidebar,
    Cmd::Settings,
    Cmd::Usage,
    Cmd::FrameStats,
    Cmd::FocusPrompt,
    Cmd::PrevThread,
    Cmd::NextThread,
    Cmd::SettleThread,
    Cmd::DeleteThread,
    Cmd::Stop,
    Cmd::Steer,
    Cmd::Allow,
    Cmd::AlwaysAllow,
    Cmd::HarnessMenu,
    Cmd::ModelMenu,
    Cmd::EffortMenu,
    Cmd::AccessMenu,
];

/// Keys the prompt field owns, listed in Settings but not rebindable.
pub const FIXED: [(&str, &str); 2] = [("Send, or queue while a turn runs", "↩"), ("New line", "⇧↩")];

impl Cmd {
    pub fn id(self) -> &'static str {
        match self {
            Cmd::NewThread => "new-thread",
            Cmd::OpenFolder => "open-folder",
            Cmd::ToggleSidebar => "toggle-sidebar",
            Cmd::Settings => "settings",
            Cmd::Usage => "usage",
            Cmd::FrameStats => "frame-stats",
            Cmd::FocusPrompt => "focus-prompt",
            Cmd::PrevThread => "prev-thread",
            Cmd::NextThread => "next-thread",
            Cmd::SettleThread => "settle-thread",
            Cmd::DeleteThread => "delete-thread",
            Cmd::Stop => "stop",
            Cmd::Steer => "steer",
            Cmd::Allow => "allow",
            Cmd::AlwaysAllow => "always-allow",
            Cmd::HarnessMenu => "harness-menu",
            Cmd::ModelMenu => "model-menu",
            Cmd::EffortMenu => "effort-menu",
            Cmd::AccessMenu => "access-menu",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Cmd::NewThread => "New thread",
            Cmd::OpenFolder => "Open a folder",
            Cmd::ToggleSidebar => "Show or hide the sidebar",
            Cmd::Settings => "Open Settings",
            Cmd::Usage => "Show usage and plan limits",
            Cmd::FrameStats => "Show or hide frame stats",
            Cmd::FocusPrompt => "Focus the prompt",
            Cmd::PrevThread => "Previous thread",
            Cmd::NextThread => "Next thread",
            Cmd::SettleThread => "Settle thread and go to the next",
            Cmd::DeleteThread => "Delete thread",
            Cmd::Stop => "Stop the running turn",
            Cmd::Steer => "Steer: send into the running turn now",
            Cmd::Allow => "Allow the waiting tool",
            Cmd::AlwaysAllow => "Always allow the waiting tool",
            Cmd::HarnessMenu => "Choose harness",
            Cmd::ModelMenu => "Choose model",
            Cmd::EffortMenu => "Choose effort",
            Cmd::AccessMenu => "Choose access",
        }
    }

    pub fn default_spec(self) -> &'static str {
        match self {
            Cmd::NewThread => "mod+n",
            Cmd::OpenFolder => "mod+o",
            Cmd::ToggleSidebar => "mod+shift+b",
            Cmd::Settings => "mod+comma",
            Cmd::Usage => "mod+shift+u",
            Cmd::FrameStats => "mod+shift+p",
            Cmd::FocusPrompt => "mod+l",
            Cmd::PrevThread => "mod+alt+up",
            Cmd::NextThread => "mod+alt+down",
            Cmd::SettleThread => "mod+shift+s",
            Cmd::DeleteThread => "mod+shift+backspace",
            Cmd::Stop => "escape",
            Cmd::Steer => "mod+enter",
            Cmd::Allow => "mod+y",
            Cmd::AlwaysAllow => "mod+shift+y",
            Cmd::HarnessMenu => "mod+shift+h",
            Cmd::ModelMenu => "mod+shift+m",
            Cmd::EffortMenu => "mod+shift+e",
            Cmd::AccessMenu => "mod+shift+a",
        }
    }

    fn from_id(id: &str) -> Option<Cmd> {
        ALL.into_iter().find(|c| c.id() == id)
    }
}

const PREFS_KEY: &str = "shortcuts";

struct Binding {
    spec: String,
    keybind: Option<Keybind>,
    label: String,
}

impl Binding {
    fn new(spec: &str) -> Binding {
        let keybind = spec.parse::<Keybind>().ok();
        let label = keybind.as_ref().map(|k| k.label().to_string()).unwrap_or_default();
        Binding { spec: spec.to_owned(), keybind, label }
    }
}

/// The table: each command's chord, parsed once when it changes.
pub struct Keys {
    map: HashMap<Cmd, Binding>,
}

impl Keys {
    pub fn defaults() -> Keys {
        Keys { map: ALL.into_iter().map(|c| (c, Binding::new(c.default_spec()))).collect() }
    }

    /// The defaults with the saved overrides on top; overrides that don't parse are dropped.
    pub fn load(prefs: &rune::prefs::Prefs) -> Keys {
        let mut keys = Keys::defaults();
        let saved: HashMap<String, String> = prefs.get(PREFS_KEY).unwrap_or_default();
        for (id, spec) in saved {
            if let Some(c) = Cmd::from_id(&id) {
                let b = Binding::new(&spec);
                if b.keybind.is_some() { keys.map.insert(c, b); }
            }
        }
        keys
    }

    pub fn save(&self, prefs: &mut rune::prefs::Prefs) {
        let overrides: HashMap<String, String> = ALL.into_iter()
            .filter(|c| !self.is_default(*c))
            .map(|c| (c.id().to_owned(), self.spec(c).to_owned()))
            .collect();
        let _ = prefs.set(PREFS_KEY, &overrides);
    }

    pub fn spec(&self, c: Cmd) -> &str {
        &self.map[&c].spec
    }

    /// How the chord reads here ("⇧⌘B"), or "" when unbound.
    pub fn label(&self, c: Cmd) -> String {
        self.map[&c].label.clone()
    }

    /// `label` as a `&'static str`, for kit APIs that keep the hint in a `Copy` struct
    /// (`SidebarToggle::shortcut`). Each distinct label is leaked once and reused after that, so
    /// rebinding doesn't grow memory per frame, only per new chord.
    pub fn label_static(&self, c: Cmd) -> &'static str {
        static LABELS: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
        let label = &self.map[&c].label;
        let mut set = LABELS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&s) = set.get(label.as_str()) {
            return s;
        }
        let s: &'static str = Box::leak(label.clone().into_boxed_str());
        set.insert(s);
        s
    }

    pub fn matches(&self, c: Cmd, ev: &KeyEvent) -> bool {
        self.map[&c].keybind.as_ref().is_some_and(|k| k.matches(ev))
    }

    pub fn is_default(&self, c: Cmd) -> bool {
        self.spec(c) == c.default_spec()
    }

    pub fn set(&mut self, c: Cmd, spec: &str) {
        self.map.insert(c, Binding::new(spec));
    }

    /// The command other than `c` already bound to `spec`.
    fn taken(&self, c: Cmd, spec: &str) -> Option<Cmd> {
        ALL.into_iter().find(|o| *o != c && self.spec(*o).split(',').any(|s| s.trim() == spec))
    }
}

/// Runs whatever command `ev` is bound to. False lets the key through (unbound, or the
/// command doesn't apply right now, as Stop with nothing running).
pub fn dispatch(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.recording.is_some() {
        record(s, ev);
        return true;
    }
    for c in ALL {
        if s.keys.matches(c, ev) && run(s, c, tx) {
            return true;
        }
    }
    false
}

pub fn run(s: &mut App, c: Cmd, tx: &mut Tx) -> bool {
    // Settings fills the main pane: Esc (the Stop key) closes it, and a command that goes
    // somewhere else (a thread, the prompt) closes it on the way.
    if s.settings {
        match c {
            Cmd::Stop => { s.close_settings(); return true; }
            Cmd::Settings | Cmd::Usage | Cmd::FrameStats | Cmd::ToggleSidebar => {}
            _ => s.close_settings(),
        }
    }
    // The Usage page likewise.
    if s.usage_open {
        match c {
            Cmd::Stop => { s.close_usage(); return true; }
            Cmd::Settings | Cmd::Usage | Cmd::FrameStats | Cmd::ToggleSidebar => {}
            _ => s.close_usage(),
        }
    }
    match c {
        Cmd::NewThread => { s.new_thread(); }
        Cmd::OpenFolder => s.choose_folder(tx),
        Cmd::ToggleSidebar => { s.sidebar.toggle(); }
        Cmd::Settings => { s.open_settings(); }
        Cmd::Usage => { if s.usage_open { s.close_usage() } else { s.open_usage() } }
        Cmd::FrameStats => {
            s.hud = !s.hud;
            if s.hud { tx.wake_after(Duration::from_millis(500)); }
        }
        Cmd::FocusPrompt => {
            s.open_menu = None;
            s.draft.focused = true;
        }
        Cmd::PrevThread => return s.select_adjacent(-1),
        Cmd::NextThread => return s.select_adjacent(1),
        Cmd::SettleThread => {
            let Some(id) = s.selected else { return false };
            return s.settle(id);
        }
        Cmd::DeleteThread => {
            let Some(id) = s.selected else { return false };
            s.delete(id);
        }
        Cmd::Stop => {
            let Some(id) = s.selected.filter(|id| s.is_running(*id)) else { return false };
            s.stop(id);
        }
        Cmd::Steer => {
            if s.draft.text.trim().is_empty() { return false; }
            s.steer(tx);
        }
        Cmd::Allow => return s.answer_first(Decision::Allow),
        Cmd::AlwaysAllow => return s.answer_first(Decision::AlwaysAllow),
        Cmd::HarnessMenu => {
            if s.thread().is_some() { return false; }
            s.toggle_menu("harness");
        }
        Cmd::ModelMenu => s.toggle_menu("model"),
        Cmd::EffortMenu => {
            if s.active_harness().efforts().is_empty() { return false; }
            s.toggle_menu("effort");
        }
        Cmd::AccessMenu => s.toggle_menu("access"),
    }
    true
}

// Recording a chord in Settings.

/// Keys a chord can end in, as `Chord` parses them.
fn key_names() -> Vec<String> {
    let mut keys: Vec<String> = ('a'..='z').chain('0'..='9').map(String::from).collect();
    for k in [
        "enter", "tab", "space", "backspace", "delete", "escape", "home", "end", "pageup", "pagedown", "left", "right", "up",
        "down", "comma", "period", "slash", "backslash", "semicolon", "quote", "bracketleft", "bracketright", "minus", "equal",
        "backquote",
    ] {
        keys.push(k.into());
    }
    keys.extend((1..=24).map(|n| format!("f{n}")));
    keys
}

const MODS: [&str; 4] = ["mod", "ctrl", "alt", "shift"];

/// The chord `ev` is, in canonical form, and whether it holds a modifier other than Shift.
/// Found by asking `Chord::matches` of each candidate, so it agrees with how bindings match.
pub fn chord_of(ev: &KeyEvent) -> Option<(String, bool)> {
    let keys = key_names();
    for mask in 0u8..16 {
        let mods: Vec<&str> = MODS.iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).map(|(_, m)| *m).collect();
        for key in &keys {
            let spec = if mods.is_empty() { key.clone() } else { format!("{}+{key}", mods.join("+")) };
            if let Ok(chord) = spec.parse::<Chord>()
                && chord.matches(ev) {
                    return Some((chord.to_string(), mask & 0b0111 != 0));
                }
        }
    }
    None
}

pub fn begin_recording(s: &mut App, c: Cmd) {
    s.recording = if s.recording == Some(c) { None } else { Some(c) };
    s.key_error = None;
    s.draft.focused = false;
}

/// Takes the key pressed while recording: Escape cancels, Backspace or Delete clears, anything
/// else with a modifier (or a function key) binds unless another command has it.
fn record(s: &mut App, ev: &KeyEvent) {
    let Some(c) = s.recording else { return };
    // A modifier on its own, or a key no chord can name: keep listening.
    let Some((spec, modified)) = chord_of(ev) else { return };
    match spec.as_str() {
        "escape" => {
            s.recording = None;
            s.key_error = None;
            return;
        }
        "backspace" | "delete" => {
            bind(s, c, "none");
            return;
        }
        _ => {}
    }
    let fn_key = spec.rsplit('+').next().is_some_and(|k| k.len() > 1 && k.starts_with('f') && k[1..].parse::<u8>().is_ok());
    let label = Binding::new(&spec).label;
    if !modified && !fn_key {
        s.key_error = Some((c, format!("Add ⌘, ⌃ or ⌥ to {label}: on its own it would type.")));
        return;
    }
    if let Some(other) = s.keys.taken(c, &spec) {
        s.key_error = Some((c, format!("{label} is already used by “{}”.", other.title())));
        return;
    }
    bind(s, c, &spec);
}

fn bind(s: &mut App, c: Cmd, spec: &str) {
    s.keys.set(c, spec);
    s.keys.save(&mut s.prefs);
    s.recording = None;
    s.key_error = None;
}

pub fn reset(s: &mut App, c: Cmd) {
    bind(s, c, c.default_spec());
}

pub fn reset_all(s: &mut App) {
    s.keys = Keys::defaults();
    s.keys.save(&mut s.prefs);
    s.recording = None;
    s.key_error = None;
}
