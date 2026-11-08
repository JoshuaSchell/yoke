//! The project icon editor's dialog: a preview, Automatic / Image / Letters, and for letters a
//! field and a row of colours.

use rune::{El, Tx, col, el, row, text};
use rune_kit::{Button, ButtonVariant, Dialog, Kit, SegmentedControl, TextInput};

use crate::app::App;
use crate::project_icon::{Choice, tint, tint_count};
use crate::ui::sidebar::project_avatar;

const MODES: [&str; 3] = ["Automatic", "Image", "Letters"];
const SWATCH: f32 = 15.0;

pub fn dialog(s: &App, k: &Kit, layer: &mut Vec<El>) {
    let Some(edit) = &s.icon_edit else { return };
    let Some(p) = s.projects.iter().find(|p| p.path == edit.path) else { return };
    let c = &k.c;
    let mode = match p.choice { Choice::Auto => 0, Choice::Image(_) => 1, Choice::Letters { .. } => 2 };

    let preview = row().key("icon-preview").gap(12.0).center_y()
        .child(project_avatar(p, 44.0, k))
        .child(col().shrink().min_w(0.0).gap(2.0)
            .child(text(p.name.clone()).font(k.ty.ui_medium).color(c.text_strong).truncate())
            .child(text(crate::ui::tilde(&p.path)).font(k.ty.small).color(c.text_faint).truncate()));
    let modes = SegmentedControl::new(&MODES, Some(mode))
        .label("Icon")
        .fit()
        .on_pick(|s: &mut App, i: usize, tx: &mut Tx| s.pick_icon_mode(i, tx))
        .build(k)
        .key("icon-mode");

    let detail = match &p.choice {
        Choice::Auto => {
            let note = match &p.icon_file {
                Some(f) => format!("Uses {}, found in the folder.", f.strip_prefix(&p.path).unwrap_or(f).display()),
                None => "No icon in the folder, so its initials.".to_owned(),
            };
            note_line(note, k)
        }
        Choice::Image(f) => {
            let name = f.file_name().map_or_else(|| f.display().to_string(), |n| n.to_string_lossy().into_owned());
            let state = if p.icon_file.is_none() { format!("{name} is missing, so its initials.") } else { name };
            row().key("icon-image").w_full().gap(8.0).center_y()
                .child(text(state).font(k.ty.small).color(c.text_muted).truncate().grow(1.0).shrink().min_w(0.0))
                .child(Button::new("Choose…").on_press_tx(|s: &mut App, tx: &mut Tx| s.choose_icon_image(tx)).build(k).key("icon-choose"))
        }
        Choice::Letters { .. } => letters_editor(s, p.tint, &crate::project_icon::monogram(&p.name), k),
    };
    let error = edit.error.clone().map(|e| text(e).key("icon-error").font(k.ty.small).color(c.danger.icon).wrap());

    let body = col().key("icon-body").w_full().px(16.0).pb(4.0).gap(14.0)
        .child(preview)
        .child(modes)
        .child(detail)
        .children(error);
    let buttons = row().key("icon-buttons").w_full().padding(16.0).justify_end()
        .child(Button::new("Done").variant(ButtonVariant::Contrast).on_press(|s: &mut App| s.close_icon_editor()).build(k).default_button().key("icon-done"));
    Dialog::new("project-icon", "Project icon").width(380.0).fit().open(true)
        .on_close(|s: &mut App| s.close_icon_editor())
        .child(body)
        .child(buttons)
        .build(k, layer);
}

fn note_line(note: String, k: &Kit) -> El {
    text(note).key("icon-note").font(k.ty.small).color(k.c.text_faint).wrap()
}

/// The letters field (its placeholder the initials) and the colours, the chosen one ringed.
fn letters_editor(s: &App, chosen: usize, initials: &str, k: &Kit) -> El {
    let Some(edit) = &s.icon_edit else { return el() };
    let field = TextInput::new(&edit.letters)
        .width(Some(72.0))
        .placeholder(initials.to_owned())
        .label("Letters")
        .on_edit(|s: &mut App| &mut s.icon_edit.as_mut().expect("the icon editor is open").letters)
        .on_change(|s: &mut App, _: &mut Tx| s.icon_letters_typed())
        .build(k)
        .key("icon-letters");
    let swatches = (0..tint_count()).map(|i| {
        let ring = if i == chosen { k.c.text_strong } else { rune::Color::TRANSPARENT };
        row().key(format!("tint-{i}")).size(SWATCH + 4.0, SWATCH + 4.0).radius((SWATCH + 4.0) / 2.0).center().border(1.5, ring)
            .role("radio").label(format!("Colour {}", i + 1)).checked(i == chosen)
            .cursor(rune::Cursor::Pointer)
            .on_press(move |s: &mut App| s.pick_icon_tint(i))
            .child(el().size(SWATCH, SWATCH).radius(SWATCH / 2.0).fill(tint(i, k.dark)))
    });
    col().key("icon-letters-editor").w_full().gap(10.0)
        .child(row().gap(8.0).center_y()
            .child(field)
            .child(text("Up to two; empty for its initials.").font(k.ty.small).color(k.c.text_faint)))
        .child(row().key("icon-tints").w_full().gap(0.0).flex_wrap().role("radiogroup").label("Colour").children(swatches))
}
