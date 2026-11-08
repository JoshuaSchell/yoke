//! Settings (⌘, or Rune Code › Settings…): appearance, frame stats, keyboard shortcuts, and
//! which agent CLIs were found. It takes the main pane while it's open, since nothing else
//! there can be used meanwhile; Esc, Done, or picking a thread closes it.

use rune::{El, ScrollSpec, TextStyle, Tx, col, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, SegmentedControl, Switch};
use rune_theme_opencode::icons;

use crate::app::{APPEARANCES, App};
use crate::harness;
use crate::shortcuts::{self, Cmd};
use crate::ui::theme::THEMES;

/// The page's reading column.
const W: f32 = 720.0;

pub fn view(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let appearance = SegmentedControl::new(&APPEARANCES.iter().map(|a| a.label()).collect::<Vec<_>>(), APPEARANCES.iter().position(|a| *a == s.appearance))
        .label("Appearance")
        .fit()
        .on_pick(|s: &mut App, i: usize, _: &mut Tx| s.set_appearance(APPEARANCES[i]))
        .build(k)
        .key("appearance");
    let themes = SegmentedControl::new(&THEMES.iter().map(|t| t.label()).collect::<Vec<_>>(), THEMES.iter().position(|t| *t == s.theme))
        .label("Theme")
        .fit()
        .on_pick(|s: &mut App, i: usize, _: &mut Tx| s.set_theme(THEMES[i]))
        .build(k)
        .key("theme");
    let hud = Switch::new(s.hud)
        .hide_label()
        .label("Show frame stats")
        .on_toggle(|s: &mut App| s.hud = !s.hud)
        .build(k)
        .key("hud");
    let frame_stats = format!("Per-frame timings drawn over the transcript{}.", paren(&s.keys.label(Cmd::FrameStats)));
    let general = card(k, vec![
        setting(k, "Theme", "Colours for the whole window and code, in light and dark.", themes),
        setting(k, "Appearance", "Follow the system, or always light or dark.", appearance),
        setting(k, "Frame stats", &frame_stats, hud),
    ]);

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
        clis.push(row().w_full().min_h(52.0).px(18.0).py(10.0).gap(16.0).center_y()
            .child(crate::ui::sidebar::avatar(h, k))
            .child(col().grow(1.0).min_w(0.0).gap(2.0)
                .child(text(h.name()).font(label_style(k)).color(c.text_strong))
                .child(text(found).font(help_style(k)).color(color).truncate()))
            .child(text(models).font(help_style(k)).color(c.text_faint)));
    }

    let done = Button::new("Done")
        .variant(ButtonVariant::Outline)
        .on_press(|s: &mut App| s.close_settings())
        .build(k)
        .key("settings-done");
    let head = row().w_full().center_y().gap(12.0).pb(4.0)
        .child(col().grow(1.0).min_w(0.0).gap(4.0)
            .child(text("Settings").font(TextStyle { size: 26.0, line_height: 32.0, ..k.ty.body_medium }).color(c.text_strong))
            .child(text("Esc closes Settings. Changes save as you make them.").font(help_style(k)).color(c.text_muted)))
        .child(done);

    let page = col().w_full().max_w(W).gap(12.0)
        .child(head)
        .child(section_title(k, "General", None))
        .child(general)
        .child(shortcut_section(s, k))
        .child(section_title(k, "Agent CLIs", Some("Found on PATH and in the usual install folders. Restart to look again.")))
        .child(card(k, clis));
    col().key("settings").w_full().grow(1.0).min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .scroll_edge_fade(18.0)
        .child(row().w_full().justify_center().px(32.0).pt(28.0).pb(40.0).child(page))
        // A chord being recorded arrives here when the recorder button has focus.
        .on_key(|s: &mut App, ev: &rune::KeyEvent, tx: &mut Tx| s.recording.is_some() && shortcuts::dispatch(s, ev, tx))
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
    if let Some(h) = help { c = c.child(text(h.to_owned()).font(help_style(k)).color(k.c.text_muted)); }
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
fn shortcut_section(s: &App, k: &Kit) -> El {
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
        .child(section_title(k, "Keyboard shortcuts", Some("Click a shortcut, then press the new keys. Esc cancels, ⌫ clears.")).grow(1.0).min_w(0.0))
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
    for (title, keys) in shortcuts::FIXED {
        rows.push(row().w_full().min_h(48.0).px(18.0).py(6.0).gap(12.0).center_y()
            .child(text(title).font(TextStyle { size: 14.0, line_height: 20.0, ..k.ty.body }).color(c.text_base).truncate().grow(1.0).min_w(0.0))
            .child(text(keys).font(help_style(k)).color(c.text_faint).pr(12.0)));
    }
    col().w_full().gap(12.0).child(head).child(card(k, rows))
}
