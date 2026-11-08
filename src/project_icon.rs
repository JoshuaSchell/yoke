//! A project's mark, as T3 Code gives each project one (`ProjectFaviconResolver.ts` and
//! `projectIdentity.ts`, MIT): its own icon when it has one, else a two-letter monogram on a
//! tint picked from its name, so projects tell apart at a glance in the sidebar.
//!
//! The icon is the first of a list of well-known favicon and app-icon paths that exists, then
//! one a `public/` folder a level down holds (`site/public/favicon.svg`), then the one an
//! `index.html` links. ICO files are left out: Rune decodes SVG, PNG, JPEG and ICNS.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What a project's mark is, as chosen in its icon editor; saved per project folder.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Choice {
    /// Its own icon if the folder has one, else its initials.
    #[default]
    Auto,
    /// An image file the user picked.
    Image(PathBuf),
    /// Letters (its initials when empty) in a tint (the one it would get when `None`).
    Letters { text: String, tint: Option<usize> },
}

impl Choice {
    fn to_json(&self) -> Option<serde_json::Value> {
        match self {
            Choice::Auto => None,
            Choice::Image(p) => Some(serde_json::json!({"kind": "image", "path": p.to_string_lossy()})),
            Choice::Letters { text, tint } => Some(serde_json::json!({"kind": "letters", "text": text, "tint": tint})),
        }
    }

    fn from_json(v: &serde_json::Value) -> Option<Choice> {
        match v.get("kind")?.as_str()? {
            "image" => Some(Choice::Image(PathBuf::from(v.get("path")?.as_str()?))),
            "letters" => Some(Choice::Letters {
                text: letters(v.get("text").and_then(|t| t.as_str()).unwrap_or("")),
                tint: v.get("tint").and_then(|t| t.as_u64()).map(|t| t as usize % TINTS.len()),
            }),
            _ => None,
        }
    }

    /// The tint the user fixed, if any.
    pub fn tint(&self) -> Option<usize> {
        match self { Choice::Letters { tint, .. } => *tint, _ => None }
    }
}

/// Every saved choice, by project folder, from the prefs' JSON object.
pub fn load_choices(v: Option<serde_json::Value>) -> HashMap<PathBuf, Choice> {
    let Some(serde_json::Value::Object(map)) = v else { return HashMap::new() };
    map.iter().filter_map(|(k, v)| Some((PathBuf::from(k), Choice::from_json(v)?))).collect()
}

/// `choices` as the prefs' JSON object; automatic ones aren't kept.
pub fn save_choices<'a>(choices: impl IntoIterator<Item = (&'a Path, &'a Choice)>) -> serde_json::Value {
    serde_json::Value::Object(choices.into_iter().filter_map(|(p, c)| Some((p.to_string_lossy().into_owned(), c.to_json()?))).collect())
}

/// What the user typed as letters, as drawn: two at most, spaces dropped, upper case.
pub fn letters(typed: &str) -> String {
    typed.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_uppercase).take(2).collect()
}

/// The image formats a project icon can be (what Rune decodes).
pub const IMAGE_EXTENSIONS: [&str; 5] = ["svg", "png", "jpg", "jpeg", "icns"];

/// T3 Code's list, in its order, less the `.ico` entries.
const CANDIDATES: &[&str] = &[
    "favicon.svg",
    "favicon.png",
    "public/favicon.svg",
    "public/favicon.png",
    "app/favicon.png",
    "app/icon.svg",
    "app/icon.png",
    "src/favicon.svg",
    "src/app/icon.svg",
    "src/app/icon.png",
    "assets/icon.svg",
    "assets/icon.png",
    "assets/logo.svg",
    "assets/logo.png",
    ".idea/icon.svg",
    "Resources/AppIcon.icns",
    "AppIcon.icns",
];

/// Under a folder a level down (a monorepo's `site/`, `www/` or `web/`).
const NESTED: &[&str] = &["public/favicon.svg", "public/favicon.png", "static/favicon.svg", "static/favicon.png"];

/// Folders a level down not worth looking in: build output and dependencies.
const SKIP: &[&str] = &["target", "node_modules", "build", "dist", "out", "result", "vendor", "thirdparty", "third_party"];

/// Pages that may link the icon (`<link rel="icon" href="…">`).
const SOURCES: &[&str] = &["index.html", "public/index.html", "src/index.html"];

/// An icon file is at most this big; anything larger isn't a favicon.
const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// The project's own icon, if it has one. A few dozen `stat`s and at most three small reads.
pub fn find(root: &Path) -> Option<PathBuf> {
    let usable = |p: PathBuf| std::fs::metadata(&p).ok().filter(|m| m.is_file() && m.len() <= MAX_BYTES).map(|_| p);
    if let Some(p) = CANDIDATES.iter().find_map(|c| usable(root.join(c))) {
        return Some(p);
    }
    let mut subdirs: Vec<PathBuf> = std::fs::read_dir(root).ok()?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| e.file_name().to_str().is_some_and(|n| !n.starts_with('.') && !SKIP.contains(&n)))
        .map(|e| e.path())
        .collect();
    subdirs.sort();
    if let Some(p) = subdirs.iter().find_map(|d| NESTED.iter().find_map(|c| usable(d.join(c)))) {
        return Some(p);
    }
    SOURCES.iter().find_map(|src| {
        let html = std::fs::read_to_string(root.join(src)).ok()?;
        let href = icon_href(&html)?;
        let clean = href.trim_start_matches('/');
        // Stay inside the project.
        if clean.split('/').any(|part| part == "..") { return None; }
        [root.join("public").join(clean), root.join(clean)].into_iter().find_map(usable)
    })
}

/// The `href` of the first `<link rel="icon">` (or `shortcut icon`) in `html`, unless it's an ICO.
fn icon_href(html: &str) -> Option<&str> {
    // ASCII lowercasing keeps every byte where it was, so spans found in `lower` index `html`.
    let lower = html.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find("<link") {
        let start = from + i;
        let end = lower[start..].find('>').map_or(lower.len(), |e| start + e);
        let tag = &lower[start..end];
        from = end;
        let rel = attr(tag, "rel").map(|r| &tag[r]);
        if !rel.is_some_and(|r| r == "icon" || r == "shortcut icon") { continue; }
        let Some(at) = attr(tag, "href") else { continue };
        let href = html[start + at.start..start + at.end].split(['?', '#']).next().unwrap_or("");
        let lower_href = href.to_ascii_lowercase();
        if href.is_empty() || href.contains("://") || lower_href.starts_with("data:") || lower_href.ends_with(".ico") { continue; }
        return Some(href);
    }
    None
}

/// Where a quoted attribute's value is in a lowercased tag.
fn attr(tag: &str, name: &str) -> Option<std::ops::Range<usize>> {
    let mut at = 0;
    loop {
        let i = at + tag[at..].find(name)?;
        at = i + name.len();
        if !tag[..i].chars().next_back().is_some_and(char::is_whitespace) { continue; }
        let rest = &tag[at..];
        let Some(after) = rest.trim_start().strip_prefix('=') else { continue };
        let after = after.trim_start();
        let quote = after.chars().next().filter(|q| *q == '"' || *q == '\'')?;
        let open = tag.len() - after.len() + 1;
        return tag[open..].find(quote).map(|e| open..open + e);
    }
}

/// Two letters for a project with no icon, after T3 Code's rule: the first letter, then the
/// first digit in the first word, else the last word's first letter, else the first word's
/// last (one letter alone for a one-letter name). Words split at anything not a letter or digit and at a lowercase-to-uppercase step,
/// so `RadDSL` is RD and `api-client` AC.
pub fn monogram(name: &str) -> String {
    let words = words(name);
    let Some(first) = words.first() else { return "PR".into() };
    let glyphs: Vec<char> = first.chars().collect();
    let a = glyphs[0];
    let b = glyphs[1..].iter().copied().find(char::is_ascii_digit)
        .or_else(|| (words.len() > 1).then(|| words[words.len() - 1].chars().next()).flatten())
        .or_else(|| glyphs.last().copied().filter(|_| glyphs.len() > 1));
    a.to_uppercase().chain(b.into_iter().flat_map(char::to_uppercase)).take(2).collect()
}

fn words(name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for ch in name.trim().chars() {
        if !ch.is_alphanumeric() {
            if !cur.is_empty() { out.push(std::mem::take(&mut cur)); }
            prev_lower = false;
            continue;
        }
        if ch.is_uppercase() && prev_lower && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        prev_lower = ch.is_lowercase() || ch.is_ascii_digit();
        cur.push(ch);
    }
    if !cur.is_empty() { out.push(cur); }
    out
}

/// The monogram tints: Tailwind's 600 (light) and 400 (dark) of each hue T3 Code offers, less
/// grey, so a monogram is never mistaken for a disabled control.
const TINTS: &[(&str, &str)] = &[
    ("#dc2626", "#f87171"), // red
    ("#ea580c", "#fb923c"), // orange
    ("#d97706", "#fbbf24"), // amber
    ("#ca8a04", "#facc15"), // yellow
    ("#65a30d", "#a3e635"), // lime
    ("#16a34a", "#4ade80"), // green
    ("#059669", "#34d399"), // emerald
    ("#0d9488", "#2dd4bf"), // teal
    ("#0891b2", "#22d3ee"), // cyan
    ("#0284c7", "#38bdf8"), // sky
    ("#2563eb", "#60a5fa"), // blue
    ("#4f46e5", "#818cf8"), // indigo
    ("#7c3aed", "#a78bfa"), // violet
    ("#9333ea", "#c084fc"), // purple
    ("#c026d3", "#e879f9"), // fuchsia
    ("#db2777", "#f472b6"), // pink
    ("#e11d48", "#fb7185"), // rose
];

/// Which tint the project's name picks: the same name always gets the same one.
pub fn tint_index(name: &str) -> usize {
    let seed = name.trim().to_lowercase();
    let seed = if seed.is_empty() { "project".to_owned() } else { seed };
    seed.chars().fold(0usize, |i, ch| (i * 31 + ch as usize) % TINTS.len())
}

/// Tints for a list of projects, in order, each a name and the tint the user fixed for it, if
/// any. A fixed tint is kept. The others take their name's tint unless a fixed one or an
/// earlier project has it or a hue beside it, and then the nearest one clear of the others,
/// so no two projects look alike. Earlier projects keep theirs as projects are added.
pub fn spread<'a>(projects: impl IntoIterator<Item = (&'a str, Option<usize>)>) -> Vec<usize> {
    let n = TINTS.len();
    let gap = |a: usize, b: usize| a.abs_diff(b).min(n - a.abs_diff(b));
    let projects: Vec<(&str, Option<usize>)> = projects.into_iter().collect();
    let mut taken: Vec<usize> = projects.iter().filter_map(|(_, t)| *t).collect();
    let mut out = Vec::with_capacity(projects.len());
    for (name, fixed) in projects {
        if let Some(t) = fixed {
            out.push(t % n);
            continue;
        }
        let own = tint_index(name);
        let pick = [2, 1, 0].into_iter()
            .find_map(|min| (0..n).map(|step| (own + step) % n).find(|i| taken.iter().all(|t| gap(*i, *t) >= min.max(1)) || min == 0))
            .unwrap_or(own);
        taken.push(pick);
        out.push(pick);
    }
    out
}

/// How many tints there are to pick from.
pub fn tint_count() -> usize {
    TINTS.len()
}

pub fn tint(index: usize, dark: bool) -> rune::Color {
    let (light, night) = TINTS[index % TINTS.len()];
    rune::color::hex(if dark { night } else { light }).unwrap_or(rune::Color::WHITE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monograms() {
        assert_eq!(monogram("rune"), "RE");
        assert_eq!(monogram("yoke"), "YE");
        assert_eq!(monogram("joken"), "JN");
        assert_eq!(monogram("RadDSL"), "RD");
        assert_eq!(monogram("FlyDSL"), "FD");
        assert_eq!(monogram("api-client"), "AC");
        assert_eq!(monogram("web3"), "W3");
        assert_eq!(monogram("x"), "X");
        assert_eq!(monogram("  "), "PR");
    }

    #[test]
    fn tints_are_stable_and_spread() {
        assert_eq!(tint_index("rune"), tint_index("Rune"));
        let names = ["rune", "yoke", "joken", "RadDSL", "FlyDSL"];
        let mut seen: Vec<usize> = names.iter().map(|n| tint_index(n)).collect();
        seen.sort();
        seen.dedup();
        assert!(seen.len() >= 4, "{seen:?}");
    }

    #[test]
    fn spread_keeps_projects_apart() {
        let names = ["api-client", "desktop", "rune", "yoke", "joken", "RadDSL", "FlyDSL"];
        let tints = spread(names.map(|n| (n, None)));
        assert_eq!(tints[0], tint_index("api-client"), "the first keeps its own");
        for (i, a) in tints.iter().enumerate() {
            for b in &tints[i + 1..] {
                let d = a.abs_diff(*b).min(TINTS.len() - a.abs_diff(*b));
                assert!(d >= 2, "{tints:?}");
            }
        }
        // More projects than hues still gets every one a tint.
        assert_eq!(spread((0..40).map(|_| ("x", None))).len(), 40);
        // A fixed tint stays, and the others keep clear of it.
        let fixed = tint_index("desktop");
        let tints = spread([("api-client", None), ("desktop", Some(fixed)), ("rune", None)]);
        assert_eq!(tints[1], fixed);
        assert!(tints[0] != fixed && tints[2] != fixed);
    }

    #[test]
    fn choices_round_trip() {
        let a = (PathBuf::from("/p/a"), Choice::Image(PathBuf::from("/p/a/logo.svg")));
        let b = (PathBuf::from("/p/b"), Choice::Letters { text: "XY".into(), tint: Some(3) });
        let c = (PathBuf::from("/p/c"), Choice::Auto);
        let saved = save_choices([(a.0.as_path(), &a.1), (b.0.as_path(), &b.1), (c.0.as_path(), &c.1)]);
        let back = load_choices(Some(saved));
        assert_eq!(back.len(), 2, "automatic isn't saved");
        assert_eq!(back[&a.0], a.1);
        assert_eq!(back[&b.0], b.1);
        assert!(load_choices(Some(serde_json::json!({"/p": {"kind": "mystery"}}))).is_empty());
        assert_eq!(letters(" ab c"), "AB");
        assert_eq!(letters("é"), "É");
    }

    #[test]
    fn links_in_html() {
        assert_eq!(icon_href(r#"<head><link rel="stylesheet" href="a.css"><link rel="icon" type="image/svg+xml" href="/Logo.svg?v=2"></head>"#), Some("/Logo.svg"));
        assert_eq!(icon_href(r#"<LINK href='/fav.png' REL='shortcut icon'>"#), Some("/fav.png"));
        assert_eq!(icon_href(r#"<link rel="icon" href="/favicon.ico"><link rel="icon" href="/icon.png">"#), Some("/icon.png"));
        assert_eq!(icon_href(r#"<link rel="apple-touch-icon" href="/t.png">"#), None);
        assert_eq!(icon_href(r#"<link data-rel="icon" href="/t.png">"#), None);
    }

    #[test]
    fn finds_icons() {
        let root = std::env::temp_dir().join(format!("yoke-icon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("site/public")).unwrap();
        assert_eq!(find(&root), None);
        std::fs::write(root.join("site/public/favicon.svg"), "<svg/>").unwrap();
        assert_eq!(find(&root), Some(root.join("site/public/favicon.svg")));
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/icon.png"), "png").unwrap();
        assert_eq!(find(&root), Some(root.join("assets/icon.png")));
        std::fs::remove_dir_all(&root).unwrap();

        std::fs::create_dir_all(root.join("img")).unwrap();
        std::fs::write(root.join("index.html"), r#"<link rel="icon" href="/img/mark.svg">"#).unwrap();
        std::fs::write(root.join("img/mark.svg"), "<svg/>").unwrap();
        assert_eq!(find(&root), Some(root.join("img/mark.svg")));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
