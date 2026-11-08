//! The diff panel beside the transcript: a header (what it compares, the totals, unified or
//! split, wrap, refresh, close) over Rune's diff view in the theme's code colours.

use rune::{El, Tx, col, row, text};
use rune_kit::diff::{DiffMode, DiffView};
use rune_kit::{ControlSize, IconButton, IconButtonVariant, Kit, SegmentedControl, Select, Spinner};
use rune_theme_opencode::icons;

use crate::app::App;

/// The scope menu's id in `App::open_menu`.
pub const SCOPE_MENU: &str = "diff-scope";
const HEADER_H: f32 = 44.0;

pub fn view(s: &App, k: &Kit, layer: &mut Vec<El>) -> El {
    let c = &k.c;
    let scopes = s.diff_scopes();
    let labels: Vec<&str> = scopes.iter().map(|(l, _)| l.as_str()).collect();
    let current = scopes.iter().find(|(_, sc)| *sc == s.diff.scope).map(|(l, _)| l.as_str());
    let scope = Select::new("diff-scope", &labels)
        .current(current)
        .label("Compare")
        .open(s.open_menu == Some(SCOPE_MENU))
        .on_toggle(|s: &mut App| s.toggle_menu(SCOPE_MENU))
        .on_close(|s: &mut App| s.open_menu = None)
        .on_choose(|s: &mut App, label: &str, _: &mut Tx| {
            s.open_menu = None;
            if let Some((_, sc)) = s.diff_scopes().into_iter().find(|(l, _)| l == label) { s.set_diff_scope(sc); }
        })
        .build(k, layer)
        .key("diff-scope")
        .shrink()
        .min_w(0.0);
    let split = s.diff.state.mode() == DiffMode::Split;
    let mode = SegmentedControl::new(&["Unified", "Split"], Some(split as usize))
        .label("Layout")
        .fit()
        .on_pick(|s: &mut App, i: usize, _: &mut Tx| s.set_diff_mode(i == 1))
        .build(k)
        .key("diff-mode");
    let wrap = s.diff.state.wrap();
    let button = |glyph: &'static rune::Icon, label: &str| IconButton::new(glyph).control_size(ControlSize::Small).variant(IconButtonVariant::GhostMuted).label(label).help(label);
    let wrap_button = button(&icons::ALIGN_RIGHT, if wrap { "Don't wrap lines" } else { "Wrap lines" })
        .expanded(wrap)
        .on_press(|s: &mut App| { let w = !s.diff.state.wrap(); s.diff.state.set_wrap(w); })
        .build(k)
        .key("diff-wrap");
    let refresh = button(&icons::REFRESH, "Refresh").on_press(|s: &mut App| s.load_diff()).build(k).key("diff-refresh");
    let close_key = s.keys.label(crate::shortcuts::Cmd::ToggleDiff);
    let close = button(&icons::CLOSE_SMALL, "Close").shortcut(&close_key).on_press(|s: &mut App| s.toggle_diff()).build(k).key("diff-close");

    let totals = if s.diff.loading && s.diff.state.files().is_empty() {
        row().key("diff-totals").child(Spinner::new().size(12.0).build(k))
    } else {
        let st = s.diff.stat;
        let files = if st.files == 1 { "1 file".to_owned() } else { format!("{} files", st.files) };
        row().key("diff-totals").gap(6.0).center_y()
            .child(text(files).font(k.ty.small).color(c.text_muted))
            .child(text(format!("+{}", st.added)).font(k.ty.small_medium).color(c.success.icon))
            .child(text(format!("\u{2212}{}", st.removed)).font(k.ty.small_medium).color(c.danger.icon))
    };
    let header = row().key("diff-header").w_full().h(HEADER_H).px(10.0).gap(8.0).center_y().border_sides([0.0, 0.0, 1.0, 0.0], c.border_weak)
        .child(scope)
        .child(totals)
        .child(rune::el().grow(1.0))
        .child(mode)
        .child(wrap_button)
        .child(refresh)
        .child(close);

    let body = if let Some(e) = &s.diff.error {
        note(e, k)
    } else if s.diff.state.files().is_empty() {
        if s.diff.loading { col().key("diff-empty").grow(1.0) } else { note("No changes.", k) }
    } else {
        let view = DiffView::new(|s: &App| &s.diff.state, |s: &mut App| &mut s.diff.state)
            .syntax_theme(crate::ui::theme::syntax(s.theme, k))
            .build(s, k)
            .grow(1.0)
            .min_h(0.0);
        let mut body = col().key("diff-body").w_full().grow(1.0).min_h(0.0).child(view);
        if s.diff.truncated {
            body = body.child(text("The diff was too long to show it all; the files after this were left out.").font(k.ty.small).color(c.warning.icon).px(12.0).py(8.0).wrap());
        }
        body
    };
    col().key("diff-panel").role("region").label("Diff").w_full().h_full().min_h(0.0).fill(c.bg_base)
        .child(header)
        .child(body)
}

fn note(what: &str, k: &Kit) -> El {
    col().key("diff-note").w_full().grow(1.0).center().px(24.0)
        .child(text(what.to_owned()).font(k.ty.small).color(k.c.text_faint).wrap())
}

/// The toolbar's toggle for the panel.
pub fn toolbar_button(s: &App, k: &Kit) -> El {
    let open = s.diff.pane.is_open();
    let key = s.keys.label(crate::shortcuts::Cmd::ToggleDiff);
    IconButton::new(if open { &icons::REVIEW_ACTIVE } else { &icons::REVIEW })
        .variant(IconButtonVariant::GhostMuted)
        .expanded(open)
        .label(if open { "Hide the diff" } else { "Show the diff" })
        .shortcut(&key)
        .on_press(|s: &mut App| s.toggle_diff())
        .build(k)
        .key("diff-toggle")
}
