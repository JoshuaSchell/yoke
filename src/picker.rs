//! The model picker's state, after T3 Code's: a rail of harnesses (and Favorites) down the
//! side, a search over the chosen one's models, and the reasoning effort beside them, all in
//! one popover behind one chip. `ui::picker` draws it.
//!
//! Keys while it's open (the search field has focus): ↑↓ move, ↩ picks, ⌘1–⌘9 pick a row by
//! its number, Esc closes.
//!
//! The project picker, in the new-thread screen's headline ("What should we build in yoke?"),
//! is where a new thread's project is chosen, added and removed, as in T3 Code: the sidebar
//! lists every project's threads together. Same keys, less the numbers.

use rune::{Key, KeyEvent, TextEdit, Tx};

use crate::app::App;
use crate::harness::{self, Harness, models};

/// The menu id in `App::open_menu`.
pub const MENU: &str = "model";
/// Starred models, as `(harness id, model value)` in the order starred.
const FAVORITES_KEY: &str = "models.favorites";
/// A harness lists this many models before "Show more", while the search is empty.
pub const SHOWN: usize = 5;

#[derive(Default)]
pub struct Picker {
    pub search: TextEdit,
    /// What the rail has chosen: a harness's models, or `None` for Favorites.
    pub rail: Option<Harness>,
    /// "Show more" was pressed for this harness.
    pub more: bool,
    /// The row the arrows are on, an index into `App::picker_rows`.
    pub active: usize,
    pub favorites: Vec<(String, String)>,
}

impl Picker {
    pub fn load(prefs: &rune::prefs::Prefs) -> Picker {
        Picker { favorites: prefs.get(FAVORITES_KEY).unwrap_or_default(), ..Picker::default() }
    }
}

/// One model in the list.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub harness: Harness,
    pub value: String,
    pub label: String,
    /// False for a harness that isn't installed, or isn't the thread's.
    pub enabled: bool,
}

impl Row {
    pub fn key(&self) -> String {
        format!("pick-{}-{}", self.harness.id(), self.value)
    }
}

/// How an effort level reads in the picker and on the chip.
pub fn effort_label(e: &str) -> &str {
    match e {
        harness::DEFAULT_EFFORT => "Default",
        "minimal" => "Minimal",
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "Extra High",
        "max" => "Max",
        other => other,
    }
}

/// Every word of `query` is in the model's label, value or harness name.
fn matches(query: &str, h: Harness, value: &str, label: &str) -> bool {
    let hay = format!("{label} {value} {}", h.name()).to_lowercase();
    query.to_lowercase().split_whitespace().all(|w| hay.contains(w))
}

impl App {
    pub fn picker_open(&self) -> bool {
        self.open_menu == Some(MENU)
    }

    /// Whether models of `h` can be chosen: it's installed, and it's the thread's harness
    /// (a thread keeps its harness).
    pub fn pickable(&self, h: Harness) -> bool {
        self.binary(h).is_some() && self.thread().is_none_or(|t| t.harness == h)
    }

    /// Opens on the current harness, with the current model under the arrows.
    pub fn open_picker(&mut self) {
        let h = self.active_harness();
        self.open_menu = Some(MENU);
        self.draft.focused = false;
        self.picker.search = TextEdit::new("");
        self.picker.search.focused = true;
        self.picker.rail = Some(h);
        let hidden = self.models(h).iter().position(|m| m.value == self.model).is_some_and(|i| i >= SHOWN);
        self.picker.more = hidden;
        self.picker.active = self.current_row().unwrap_or(0);
    }

    pub fn close_picker(&mut self) {
        if self.picker_open() { self.open_menu = None; }
        self.picker.search.focused = false;
        self.picker.search.preedit = None;
        self.draft.focused = true;
    }

    pub fn toggle_picker(&mut self) {
        if self.picker_open() { self.close_picker() } else { self.open_picker() }
    }

    /// Shows Favorites (`None`) or a harness's models.
    pub fn picker_rail(&mut self, rail: Option<Harness>) {
        self.picker.rail = rail;
        self.picker.more = false;
        self.picker.active = self.current_row().unwrap_or(0);
        self.picker.search.focused = true;
    }

    pub fn picker_more(&mut self) {
        self.picker.more = true;
        self.picker.search.focused = true;
    }

    fn current_row(&self) -> Option<usize> {
        let h = self.active_harness();
        self.picker_rows().0.iter().position(|r| r.harness == h && r.value == self.model)
    }

    /// The rows the list shows, and how many more "Show more" would add.
    pub fn picker_rows(&self) -> (Vec<Row>, usize) {
        let query = self.picker.search.text.trim();
        let row = |h: Harness, value: &str, label: &str| Row { harness: h, value: value.to_owned(), label: label.to_owned(), enabled: self.pickable(h) };
        let all: Vec<Row> = match self.picker.rail {
            Some(h) => self.models(h).iter().map(|m| row(h, &m.value, &m.label)).collect(),
            None => self.picker.favorites.iter()
                .filter_map(|(id, value)| {
                    let h = harness::ALL.into_iter().find(|h| h.id() == id)?;
                    Some(row(h, value, self.model_label(h, value)))
                })
                .collect(),
        };
        if !query.is_empty() {
            return (all.into_iter().filter(|r| matches(query, r.harness, &r.value, &r.label)).collect(), 0);
        }
        if self.picker.rail.is_some() && !self.picker.more && all.len() > SHOWN {
            let hidden = all.len() - SHOWN;
            return (all.into_iter().take(SHOWN).collect(), hidden);
        }
        (all, 0)
    }

    pub fn is_favorite(&self, h: Harness, value: &str) -> bool {
        self.picker.favorites.iter().any(|(id, v)| id == h.id() && v == value)
    }

    pub fn toggle_favorite(&mut self, h: Harness, value: &str) {
        let before = self.picker.favorites.len();
        self.picker.favorites.retain(|(id, v)| !(id == h.id() && v == value));
        if self.picker.favorites.len() == before {
            self.picker.favorites.push((h.id().to_owned(), value.to_owned()));
        }
        let _ = self.prefs.set(FAVORITES_KEY, &self.picker.favorites);
        self.picker.search.focused = true;
    }

    /// Chooses `value` of harness `h` (switching harness on the new-thread screen) and closes.
    pub fn pick(&mut self, h: Harness, value: &str) {
        if !self.pickable(h) { return; }
        if self.thread().is_none() && self.harness != h { self.choose_harness(h); }
        self.model = value.to_owned();
        self.close_picker();
    }

    /// Sets the effort; the picker stays open, so a model can still be chosen after.
    pub fn pick_effort(&mut self, e: &str) {
        self.effort = e.to_owned();
        self.picker.search.focused = true;
    }

    fn pick_row(&mut self, i: usize) -> bool {
        let Some(r) = self.picker_rows().0.into_iter().nth(i).filter(|r| r.enabled) else { return false };
        self.pick(r.harness, &r.value);
        true
    }

    /// The search field's keys. True when the key was the picker's.
    pub fn picker_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if self.picker.search.preedit.is_some() { return false; }
        let (rows, _) = self.picker_rows();
        match &ev.key {
            Key::Escape => { if !tx.key_repeat() { self.close_picker(); } true }
            Key::Enter => { if !tx.key_repeat() { self.pick_row(self.picker.active); } true }
            Key::Command(c) if c.is_ascii_digit() && *c != '0' && !ev.mods.shift && !ev.mods.alt => {
                self.pick_row(*c as usize - '1' as usize);
                true
            }
            Key::Up | Key::Down if !ev.mods.cmd && !ev.mods.ctrl => {
                let on: Vec<usize> = rows.iter().enumerate().filter(|(_, r)| r.enabled).map(|(i, _)| i).collect();
                if on.is_empty() { return true; }
                let at = on.iter().position(|i| *i == self.picker.active);
                let step: isize = if ev.key == Key::Down { 1 } else { -1 };
                let next = match at {
                    Some(p) => (p as isize + step).rem_euclid(on.len() as isize) as usize,
                    None if step > 0 => 0,
                    None => on.len() - 1,
                };
                self.picker.active = on[next];
                tx.scroll_into_view(rows[on[next]].key());
                true
            }
            _ => false,
        }
    }

    /// After the search changes: the first row that can be chosen is under the arrows.
    pub fn picker_searched(&mut self) {
        self.picker.active = self.picker_rows().0.iter().position(|r| r.enabled).unwrap_or(0);
    }

    /// The chip's text: "Opus 5.5 High", "Default model".
    pub fn picker_summary(&self) -> (String, Option<&'static str>) {
        let h = self.active_harness();
        let model = if self.model == models::DEFAULT { "Default model".to_owned() } else { self.model_label(h, &self.model).to_owned() };
        let effort = h.efforts().iter().find(|e| **e == self.effort && **e != harness::DEFAULT_EFFORT).map(|e| effort_label(e));
        (model, effort)
    }
}

// ---------------------------------------------------------------------------------------------
// The project picker

/// The project picker's id in `App::open_menu`.
pub const PROJECT_MENU: &str = "project";

#[derive(Default)]
pub struct ProjectPick {
    pub search: TextEdit,
    /// The row the arrows are on, an index into `App::project_rows`.
    pub active: usize,
}

impl App {
    pub fn project_picker_open(&self) -> bool {
        self.open_menu == Some(PROJECT_MENU)
    }

    /// Opens with the current project under the arrows.
    pub fn open_project_picker(&mut self) {
        self.open_menu = Some(PROJECT_MENU);
        self.draft.focused = false;
        self.project_pick.search = TextEdit::new("");
        self.project_pick.search.focused = true;
        self.project_pick.active = self.project_rows().iter().position(|p| self.current.as_ref() == Some(p)).unwrap_or(0);
    }

    pub fn close_project_picker(&mut self) {
        if self.project_picker_open() { self.open_menu = None; }
        self.project_pick.search.focused = false;
        self.project_pick.search.preedit = None;
        self.draft.focused = true;
    }

    pub fn toggle_project_picker(&mut self) {
        if self.project_picker_open() { self.close_project_picker() } else { self.open_project_picker() }
    }

    /// The projects the picker lists, in the order added: those whose name or path holds every
    /// word of the search.
    pub fn project_rows(&self) -> Vec<std::path::PathBuf> {
        let query = self.project_pick.search.text.trim().to_lowercase();
        self.projects.iter()
            .filter(|p| {
                let hay = format!("{} {}", p.name, p.path.display()).to_lowercase();
                query.split_whitespace().all(|w| hay.contains(w))
            })
            .map(|p| p.path.clone())
            .collect()
    }

    /// New threads start in `path` from now on; the composer stays on the new-thread screen.
    pub fn choose_project(&mut self, path: std::path::PathBuf) {
        self.set_current(path);
        self.close_project_picker();
    }

    /// The search field's keys. True when the key was the picker's.
    pub fn project_picker_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if self.project_pick.search.preedit.is_some() { return false; }
        let rows = self.project_rows();
        match &ev.key {
            Key::Escape => { if !tx.key_repeat() { self.close_project_picker(); } true }
            Key::Enter => {
                if !tx.key_repeat() && let Some(p) = rows.get(self.project_pick.active) { self.choose_project(p.clone()); }
                true
            }
            Key::Up | Key::Down if !ev.mods.cmd && !ev.mods.ctrl => {
                if rows.is_empty() { return true; }
                let n = rows.len() as isize;
                let step: isize = if ev.key == Key::Down { 1 } else { -1 };
                self.project_pick.active = (self.project_pick.active as isize + step).rem_euclid(n) as usize;
                tx.scroll_into_view(project_row_key(&rows[self.project_pick.active]));
                true
            }
            _ => false,
        }
    }

    /// After the search changes: the first match is under the arrows.
    pub fn project_picker_searched(&mut self) {
        self.project_pick.active = 0;
    }
}

/// A project row's key in the picker.
pub fn project_row_key(path: &std::path::Path) -> String {
    format!("pick-project-{}", path.display())
}
