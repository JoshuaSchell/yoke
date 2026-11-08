//! Colour themes: OpenCode (Rune's own theme crate), Nord and Gruvbox, each light and dark.
//!
//! Nord and Gruvbox reuse the OpenCode kit (fonts, type, radii, motion, shadows, glyphs) and
//! recolour it. OpenCode's palette is one grey ramp plus an accent and four status colours, so
//! every grey maps by its lightness onto the theme's own ramp (interpolated in Oklab between
//! anchors), translucent overlays keep their alpha over the mapped colour, and the accent,
//! status and code colours come from the theme's palette.

use std::sync::OnceLock;

use rune::Color;
use rune_kit::markdown::{MarkdownStyle, Syntax};
use rune_kit::syntax::SyntaxTheme;
use rune_kit::{Kit, Palette, StateColors};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Theme {
    OpenCode,
    Nord,
    Gruvbox,
}

pub const THEMES: [Theme; 3] = [Theme::OpenCode, Theme::Nord, Theme::Gruvbox];

impl Theme {
    pub fn label(self) -> &'static str {
        match self {
            Theme::OpenCode => "OpenCode",
            Theme::Nord => "Nord",
            Theme::Gruvbox => "Gruvbox",
        }
    }

    pub fn from_label(l: &str) -> Option<Theme> {
        THEMES.into_iter().find(|t| t.label() == l)
    }

    fn spec(self, dark: bool) -> Option<&'static Spec> {
        match (self, dark) {
            (Theme::OpenCode, _) => None,
            (Theme::Nord, true) => Some(&NORD_DARK),
            (Theme::Nord, false) => Some(&NORD_LIGHT),
            (Theme::Gruvbox, true) => Some(&GRUVBOX_DARK),
            (Theme::Gruvbox, false) => Some(&GRUVBOX_LIGHT),
        }
    }
}

/// The kit for a theme and appearance, built once per window blend mode, as
/// `rune_theme_opencode::kit` is.
pub fn kit(theme: Theme, dark: bool) -> &'static Kit {
    let Some(spec) = theme.spec(dark) else { return rune_theme_opencode::kit(dark) };
    static KITS: [[[OnceLock<Kit>; 2]; 2]; 3] = [const { [const { [const { OnceLock::new() }; 2] }; 2] }; 3];
    let srgb = rune::theme::blends_srgb();
    KITS[theme as usize][dark as usize][srgb as usize].get_or_init(|| recolor(rune_theme_opencode::kit(dark), spec))
}

/// Markdown in the theme's code colours.
pub fn markdown(theme: Theme, k: &Kit) -> MarkdownStyle {
    let style = MarkdownStyle::from_kit(k);
    let Some(spec) = theme.spec(k.dark) else { return style };
    let mut syn = SyntaxTheme::from_kit(k);
    for (kind, c) in [
        (Syntax::Keyword, spec.syntax[0]),
        (Syntax::String, spec.syntax[1]),
        (Syntax::Number, spec.syntax[2]),
        (Syntax::Type, spec.syntax[3]),
        (Syntax::Function, spec.syntax[4]),
        (Syntax::Property, spec.syntax[5]),
        (Syntax::Comment, spec.syntax[6]),
    ] {
        syn = syn.with(kind, hex(c));
    }
    style.syntax_theme(syn)
}

/// A theme for one appearance, as sRGB hex.
struct Spec {
    /// (OpenCode grey level 0–255, this theme's colour), dark to light. OpenCode's levels are
    /// its roles: in dark, 0x08 the window, 0x16 the card, 0x24–0x5c raised layers, 0x80 faint
    /// text, 0xae muted text, 0xfa text. In light, 0xff the card, 0xfa the window and so on down.
    ramp: &'static [(u8, &'static str)],
    /// Filled accent (unread dot, focus), and accent text and icons.
    accent: &'static str,
    accent_text: &'static str,
    success: &'static str,
    warning: &'static str,
    danger: &'static str,
    info: &'static str,
    /// Keyword, string, number, type, function, property, comment.
    syntax: [&'static str; 7],
}

// https://www.nordtheme.com/docs/colors-and-palettes
const NORD_DARK: Spec = Spec {
    ramp: &[
        (0x00, "#1b1f27"), (0x08, "#242933"), (0x16, "#2e3440"), (0x24, "#3b4252"), (0x2e, "#434c5e"),
        (0x3a, "#4c566a"), (0x5c, "#5e6a82"), (0x80, "#7b88a1"), (0xae, "#a7b1c3"), (0xdb, "#d8dee9"),
        (0xfa, "#eceff4"), (0xff, "#f5f7fa"),
    ],
    accent: "#5e81ac",
    accent_text: "#88c0d0",
    success: "#a3be8c",
    warning: "#ebcb8b",
    danger: "#bf616a",
    info: "#81a1c1",
    syntax: ["#81a1c1", "#a3be8c", "#b48ead", "#8fbcbb", "#88c0d0", "#d08770", "#7b88a1"],
};

const NORD_LIGHT: Spec = Spec {
    ramp: &[
        (0x00, "#1f232b"), (0x16, "#2e3440"), (0x3a, "#3b4252"), (0x5c, "#4c566a"), (0x80, "#6c7a93"),
        (0xae, "#9aa5b8"), (0xdb, "#c9d1de"), (0xee, "#d8dee9"), (0xf2, "#e2e7ef"), (0xfa, "#e9edf3"),
        (0xff, "#f6f8fb"),
    ],
    accent: "#5e81ac",
    accent_text: "#4c6f99",
    success: "#5f7f45",
    warning: "#a8792a",
    danger: "#b0444f",
    info: "#5e81ac",
    syntax: ["#5e81ac", "#5f7f45", "#9a5f93", "#3d7f7d", "#4c7ea3", "#b2654f", "#7b88a1"],
};

// https://github.com/morhetz/gruvbox
const GRUVBOX_DARK: Spec = Spec {
    ramp: &[
        (0x00, "#141617"), (0x08, "#1d2021"), (0x16, "#282828"), (0x24, "#32302f"), (0x2e, "#3c3836"),
        (0x3a, "#504945"), (0x5c, "#665c54"), (0x80, "#928374"), (0xae, "#bdae93"), (0xdb, "#d5c4a1"),
        (0xfa, "#ebdbb2"), (0xff, "#fbf1c7"),
    ],
    accent: "#458588",
    accent_text: "#83a598",
    success: "#b8bb26",
    warning: "#fabd2f",
    danger: "#fb4934",
    info: "#83a598",
    syntax: ["#fb4934", "#b8bb26", "#d3869b", "#fabd2f", "#8ec07c", "#83a598", "#928374"],
};

const GRUVBOX_LIGHT: Spec = Spec {
    ramp: &[
        (0x00, "#1d2021"), (0x16, "#282828"), (0x3a, "#3c3836"), (0x5c, "#504945"), (0x80, "#7c6f64"),
        (0xae, "#a89984"), (0xdb, "#d5c4a1"), (0xee, "#e6d5ae"), (0xf2, "#ebdbb2"), (0xfa, "#f2e5bc"),
        (0xff, "#fbf1c7"),
    ],
    accent: "#076678",
    accent_text: "#076678",
    success: "#79740e",
    warning: "#b57614",
    danger: "#9d0006",
    info: "#076678",
    syntax: ["#9d0006", "#79740e", "#8f3f71", "#b57614", "#427b58", "#076678", "#928374"],
};

fn hex(s: &str) -> Color {
    rune::color::hex(s).unwrap_or_else(|| panic!("bad colour {s}"))
}

/// A grey's sRGB level (0–255), or `None` for a colour with a hue.
fn grey_level(c: Color) -> Option<f32> {
    let [r, g, b] = [c.r, c.g, c.b].map(|x| rune::color::srgb_encode(x) * 255.0);
    let (lo, hi) = (r.min(g).min(b), r.max(g).max(b));
    (hi - lo < 14.0).then_some((r + g + b) / 3.0)
}

/// Where OpenCode grey level `level` falls on `ramp`, between its two nearest anchors.
fn on_ramp(ramp: &[(u8, &str)], level: f32) -> Color {
    let at = ramp.iter().position(|(l, _)| *l as f32 >= level).unwrap_or(ramp.len() - 1);
    if at == 0 {
        return hex(ramp[0].1);
    }
    let ((l0, c0), (l1, c1)) = (ramp[at - 1], ramp[at]);
    let t = ((level - l0 as f32) / (l1 as f32 - l0 as f32).max(1.0)).clamp(0.0, 1.0);
    let (a, b) = (hex(c0).to_oklab(), hex(c1).to_oklab());
    Color::from_oklab([0, 1, 2, 3].map(|i| a[i] + (b[i] - a[i]) * t))
}

fn recolor(base: &Kit, spec: &Spec) -> Kit {
    let grey = |c: Color| match grey_level(c) {
        Some(level) => on_ramp(spec.ramp, level).alpha(c.a),
        None => c,
    };
    let p = &base.c;
    let card = on_ramp(spec.ramp, grey_level(p.bg_base).unwrap_or(0.0));
    let state = |hue: &str| {
        let h = hex(hue);
        StateColors { bg: card.mix(h, 0.14), fg: h, border: card.mix(h, 0.4), icon: h }
    };
    let (accent, accent_text) = (hex(spec.accent), hex(spec.accent_text));
    let mut c = Palette {
        page: grey(p.page),
        bg_base: grey(p.bg_base),
        bg_deep: grey(p.bg_deep),
        layer_01: grey(p.layer_01),
        layer_02: grey(p.layer_02),
        layer_03: grey(p.layer_03),
        layer_04: grey(p.layer_04),
        bg_inverse: grey(p.bg_inverse),
        bg_contrast: grey(p.bg_contrast),
        bg_button_neutral: grey(p.bg_button_neutral),
        bg_icon_button_contrast: grey(p.bg_icon_button_contrast),
        bg_accent: accent,
        submit_highlight: grey(p.submit_highlight),
        submit_hover: grey(p.submit_hover),
        submit_fg: grey(p.submit_fg),
        submit_icon: grey(p.submit_icon),
        text_base: grey(p.text_base),
        text_muted: grey(p.text_muted),
        text_faint: grey(p.text_faint),
        text_inverse: grey(p.text_inverse),
        text_contrast: grey(p.text_contrast),
        text_accent: accent_text,
        icon_base: grey(p.icon_base),
        icon_muted: grey(p.icon_muted),
        icon_faint: grey(p.icon_faint),
        icon_inverse: grey(p.icon_inverse),
        icon_contrast: grey(p.icon_contrast),
        icon_accent: accent_text,
        border_muted: grey(p.border_muted),
        border_base: grey(p.border_base),
        border_strong: grey(p.border_strong),
        border_focus: accent_text,
        ..*p
    };
    for x in [
        &mut c.hover, &mut c.pressed, &mut c.contrast_hover, &mut c.contrast_pressed, &mut c.scrim, &mut c.knob, &mut c.knob_on,
        &mut c.surface_base, &mut c.surface_weak, &mut c.surface_hover, &mut c.surface_raised, &mut c.surface_raised_base,
        &mut c.surface_raised_hover, &mut c.surface_float, &mut c.surface_disabled, &mut c.border_weak, &mut c.border_hover,
        &mut c.border_active, &mut c.border_disabled, &mut c.text_strong, &mut c.text_weak, &mut c.text_invert_strong,
        &mut c.icon_weaker, &mut c.std_border, &mut c.std_icon, &mut c.std_icon_invert, &mut c.std_icon_strong, &mut c.std_text,
    ] {
        *x = grey(*x);
    }
    c.success = state(spec.success);
    c.warning = state(spec.warning);
    c.danger = state(spec.danger);
    c.info = state(spec.info);
    let mut k = base.clone();
    k.c = c;
    k.blend_over = Some(c.bg_base);
    k
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every theme keeps OpenCode's order of greys (window darker than the card in dark, text
    /// brighter than muted than faint), and its text reads on its card.
    #[test]
    fn themes_keep_the_ramp_in_order() {
        let l = |c: Color| c.to_oklab()[0];
        for theme in THEMES {
            for dark in [true, false] {
                let k = kit(theme, dark);
                let c = &k.c;
                assert_eq!(k.dark, dark);
                if dark {
                    assert!(l(c.bg_deep) < l(c.bg_base) && l(c.bg_base) < l(c.layer_01), "{theme:?} dark layers");
                    assert!(l(c.text_base) > l(c.text_muted) && l(c.text_muted) > l(c.text_faint), "{theme:?} dark text");
                } else {
                    assert!(l(c.bg_deep) < l(c.bg_base), "{theme:?} light window under the card");
                    assert!(l(c.text_base) < l(c.text_muted) && l(c.text_muted) < l(c.text_faint), "{theme:?} light text");
                }
                assert!((l(c.text_base) - l(c.bg_base)).abs() > 0.6, "{theme:?} {dark} text contrast");
            }
        }
        assert_ne!(kit(Theme::Nord, true).c.bg_base, kit(Theme::Gruvbox, true).c.bg_base);
    }
}
