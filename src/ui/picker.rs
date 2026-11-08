//! The model picker, after T3 Code's: one chip in the composer ("Opus 5.5 · High") opening a
//! popover with Favorites and the harnesses down a rail, a search over the chosen one's models
//! (starred ones pinned to Favorites, ⌘1–⌘9 on the first nine), and the reasoning effort in a
//! column beside them. State and keys live in `crate::picker`.

use rune::{Cursor, El, Icon, ScrollSpec, Tx, col, el, icon, row, text};
use rune_kit::{Keybind, KeybindVariant, Kit, Placement, TextInput, popover, scrim};
use rune_theme_opencode::icons;

use crate::app::App;
use crate::harness::{self, ALL, Harness};
use crate::picker::{Row, effort_label};
use crate::ui::sidebar::{claude_orange, provider_icon};

pub static STAR: Icon = Icon { name: "star", view: 20.0, body: r##"<path d="M10 2.9l2.2 4.46 4.92.72-3.56 3.47.84 4.9L10 14.13l-4.4 2.32.84-4.9-3.56-3.47 4.92-.72z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/>"## };
pub static STAR_FILLED: Icon = Icon { name: "star-filled", view: 20.0, body: r##"<path d="M10 2.9l2.2 4.46 4.92.72-3.56 3.47.84 4.9L10 14.13l-4.4 2.32.84-4.9-3.56-3.47 4.92-.72z" fill="currentColor" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/>"## };

const TRIGGER: &str = "model-picker-trigger";
const RAIL_W: f32 = 52.0;
const LIST_W: f32 = 330.0;
const EFFORT_W: f32 = 220.0;
const PANEL_H: f32 = 340.0;
const ROW_H: f32 = 34.0;

fn star_color() -> rune::Color {
    rune::color::hex("#e8b339").unwrap_or(rune::Color::WHITE)
}

fn harness_icon(h: Harness, size: f32, k: &Kit) -> El {
    icon(provider_icon(h), size).color(if h == Harness::Claude { claude_orange() } else { k.c.icon_base })
}

/// "⌘1" on a Mac, "Ctrl+1" elsewhere.
fn quick_key(n: usize) -> String {
    if cfg!(target_os = "macos") { format!("⌘{n}") } else { format!("Ctrl+{n}") }
}

/// The chip, with the popover pushed into `layer` while it's open.
pub fn view(s: &App, k: &Kit, layer: &mut Vec<El>) -> El {
    let c = &k.c;
    let open = s.picker_open();
    let h = s.active_harness();
    let (model, effort) = s.picker_summary();
    let key = s.keys.label(crate::shortcuts::Cmd::ModelMenu);
    let mut chip = row().key("model-chip").h(28.0).pl(6.0).pr(8.0).gap(6.0).center_y().radius(8.0)
        .cursor(Cursor::Pointer)
        .role("button")
        .expanded(open)
        .fill(if open { c.surface_hover } else { rune::Color::TRANSPARENT })
        .hover(c.hover)
        .label(format!("Model: {}, {model}{}", h.name(), effort.map_or(String::new(), |e| format!(", {e} reasoning"))))
        .help(if key.is_empty() { "Choose model".to_owned() } else { format!("Choose model ({key})") })
        .child(harness_icon(h, 14.0, k))
        .child(text(model).font(k.ty.ui_medium).color(c.text_strong));
    if let Some(e) = effort {
        chip = chip.child(text(e).font(k.ty.ui).color(c.text_muted));
    }
    let chip = chip
        .child(icon(&icons::CHEVRON_DOWN, 12.0).color(c.icon_weaker))
        .on_press(|s: &mut App| s.toggle_picker());
    let trigger = row().key(TRIGGER).child(chip);
    if open {
        layer.push(scrim("model-picker-scrim", |s: &mut App| s.close_picker()));
        let surface = popover::raised(k, panel(s, k).key("model-picker").role("dialog").label("Choose model").focus_return(TRIGGER), 12.0);
        layer.push(popover::anchored(k, surface, TRIGGER, Placement::TopStart, 8.0));
    }
    trigger
}

fn panel(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let h = s.picker.rail.unwrap_or_else(|| s.active_harness());
    let vline = || el().w(1.0).h_full().fill(c.border_weak);
    let mut body = row().h(PANEL_H).shrink().min_h(0.0)
        .child(rail(s, k))
        .child(vline())
        .child(list(s, k));
    if !h.efforts().is_empty() {
        body = body.child(vline()).child(efforts(s, h, k));
    }
    col().clip_rounded(12.0).child(body)
}

/// Favorites, then each harness. One that isn't installed, or isn't the thread's, is dimmed.
fn rail(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let item = |key: String, label: String, help: String, glyph: El, on: bool, enabled: bool, rail: Option<Harness>| {
        let mut b = row().key(key).size(36.0, 36.0).center().radius(10.0)
            .role("tab").label(label).help(help).checked(on)
            .fill(if on { c.surface_hover } else { rune::Color::TRANSPARENT })
            .child(glyph);
        if enabled || rail.is_none() {
            b = b.cursor(Cursor::Pointer).hover(c.hover).on_press(move |s: &mut App| s.picker_rail(rail));
        } else {
            b = b.layer_opacity(0.4);
        }
        // T3's marker: a short accent bar on the rail's edge beside the chosen one.
        let bar = el().w(3.0).h(18.0).radius(1.5).fill(if on { c.bg_accent } else { rune::Color::TRANSPARENT });
        row().w_full().h(40.0).center_y().gap(5.0).child(el().w(3.0)).child(b).child(bar)
    };
    let fav_on = s.picker.rail.is_none();
    let mut r = col().w(RAIL_W).h_full().pt(8.0).gap(4.0).fill(c.layer_02.alpha(0.35))
        .child(item("rail-favorites".into(), "Favorites".into(), "Favorites".into(),
            icon(if fav_on { &STAR_FILLED } else { &STAR }, 18.0).color(if fav_on { c.text_strong } else { c.icon_base }), fav_on, true, None))
        .child(el().w(24.0).h(1.0).ml(14.0).my(4.0).fill(c.border_weak));
    for h in ALL {
        let help = if s.binary(h).is_none() {
            format!("{} isn't installed", h.name())
        } else if !s.pickable(h) {
            format!("{}: this thread runs on {}", h.name(), s.active_harness().name())
        } else {
            h.name().to_owned()
        };
        r = r.child(item(format!("rail-{}", h.id()), h.name().into(), help, harness_icon(h, 18.0, k), s.picker.rail == Some(h), s.binary(h).is_some(), Some(h)));
    }
    r
}

/// The search over the models, the rows, and "Show more".
fn list(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let search = TextInput::new(&s.picker.search)
        .plain()
        .leading_icon(k.glyph.search)
        .height(44.0)
        .width(None)
        .placeholder("Search models…")
        .label("Search models")
        .on_edit(|s: &mut App| &mut s.picker.search)
        .on_change(|s: &mut App, _: &mut Tx| s.picker_searched())
        .on_key(|s: &mut App, ev: &rune::KeyEvent, tx: &mut Tx| s.picker_key(ev, tx))
        .build(k)
        .key("model-search");
    let (rows, hidden) = s.picker_rows();
    let current = s.active_harness();
    let mut items = col().key("model-rows").w_full().gap(2.0).px(8.0).py(8.0);
    if rows.is_empty() {
        let note = if !s.picker.search.text.trim().is_empty() {
            "No models match"
        } else if s.picker.rail.is_none() {
            "No favorites yet. Star a model to pin it here."
        } else {
            "No models"
        };
        items = items.child(text(note).font(k.ty.small).color(c.text_faint).px(10.0).py(10.0));
    }
    for (i, r) in rows.iter().enumerate() {
        let chosen = r.harness == current && r.value == s.model;
        items = items.child(model_row(s, r, i, chosen, s.picker.active == i, k));
    }
    if hidden > 0 {
        items = items.child(row().key("model-more").w_full().h(ROW_H).px(10.0).center_y().radius(8.0)
            .cursor(Cursor::Pointer).hover(c.hover).role("button")
            .label(format!("Show {hidden} more"))
            .child(text("Show more").font(k.ty.ui).color(c.text_muted))
            .on_press(|s: &mut App| s.picker_more()));
    }
    let scroller = col().key("model-scroll").w_full().grow(1.0).shrink().min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .scroll_edge_fade(16.0)
        .scroll_margin(16.0)
        .child(items);
    col().w(LIST_W).h_full().min_h(0.0)
        .child(col().w_full().px(8.0).child(search))
        .child(el().w_full().h(1.0).fill(c.border_weak))
        .child(scroller)
}

fn model_row(s: &App, r: &Row, i: usize, chosen: bool, active: bool, k: &Kit) -> El {
    let c = &k.c;
    let (h, value) = (r.harness, r.value.clone());
    let fav = s.is_favorite(h, &r.value);
    let star = row().key("star").size(24.0, 24.0).center().radius(6.0)
        .cursor(Cursor::Pointer).hover(c.hover)
        .role("button").label(if fav { "Remove from favorites" } else { "Add to favorites" })
        .child(icon(if fav { &STAR_FILLED } else { &STAR }, 14.0).color(if fav { star_color() } else { c.icon_weaker }))
        .on_press({ let value = value.clone(); move |s: &mut App| s.toggle_favorite(h, &value) });
    let mut line = row().key(r.key()).w_full().h(ROW_H).pl(10.0).pr(6.0).gap(10.0).center_y().radius(8.0)
        .role("option").label(format!("{} {}", h.name(), r.label)).checked(chosen)
        .fill(if active { c.hover } else { rune::Color::TRANSPARENT })
        .child(harness_icon(h, 15.0, k))
        .child(text(r.label.clone()).font(k.ty.ui).color(if r.enabled { c.text_strong } else { c.text_faint }).truncate().shrink().min_w(0.0));
    if chosen {
        line = line.child(icon(&icons::CHECK_SMALL, 14.0).color(c.icon_base));
    }
    line = line.child(el().grow(1.0)).child(star);
    if i < 9 {
        line = line.child(Keybind::new(&[quick_key(i + 1)]).variant(KeybindVariant::Neutral).build(k).w(36.0));
    } else {
        line = line.child(el().w(36.0));
    }
    if r.enabled {
        line.cursor(Cursor::Pointer).hover(c.hover).on_press(move |s: &mut App| s.pick(h, &value))
    } else {
        line.layer_opacity(0.6).help(format!("{} isn't available for this thread", h.name()))
    }
}

/// The harness's effort levels; the first sends no flag (the CLI's own default).
fn efforts(s: &App, h: Harness, k: &Kit) -> El {
    let c = &k.c;
    let model = if s.picker.rail == Some(s.active_harness()) || s.picker.rail.is_none() { s.picker_summary().0 } else { h.name().to_owned() };
    let head = row().w_full().h(44.0).px(14.0).gap(8.0).center_y()
        .child(text("Reasoning").font(k.ty.ui_medium).color(c.text_muted).grow(1.0))
        .child(text(model).font(k.ty.small).color(c.text_faint).truncate().shrink().min_w(0.0));
    let enabled = s.pickable(h) && h == s.active_harness();
    let mut levels = col().key("efforts").w_full().gap(2.0).px(8.0).py(8.0);
    for e in h.efforts().iter().copied() {
        let chosen = enabled && s.effort == e;
        let mut line = row().key(format!("effort-{e}")).w_full().h(ROW_H).px(10.0).gap(8.0).center_y().radius(8.0)
            .role("option").label(format!("{} reasoning", effort_label(e))).checked(chosen)
            .child(text(effort_label(e)).font(k.ty.ui).color(c.text_strong).grow(1.0));
        if e == harness::DEFAULT_EFFORT {
            line = line.help("Sends no effort flag: the CLI picks").child(text("CLI's choice").font(k.ty.small).color(c.text_faint));
        }
        if chosen {
            line = line.child(icon(&icons::CHECK_SMALL, 14.0).color(c.icon_base));
        }
        line = if enabled {
            line.cursor(Cursor::Pointer).hover(c.hover).on_press(move |s: &mut App| s.pick_effort(e))
        } else {
            line.layer_opacity(0.5)
        };
        levels = levels.child(line);
    }
    col().w(EFFORT_W).h_full()
        .child(head)
        .child(el().w_full().h(1.0).fill(c.border_weak))
        .child(levels)
}
