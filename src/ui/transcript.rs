//! The conversation: a virtualised list that sticks to the bottom while a turn streams.
//!
//! Only the rows near the viewport are built (`virtual_rows`), and every finished row sits in a
//! `memo` keyed by its revision, so a streaming delta rebuilds one Markdown block, not the
//! whole transcript.

use std::borrow::Cow;

use rune::{El, GrowthSpec, Icon, ScrollSpec, col, icon, memo, row, text};
use crate::ui::theme::{self, Theme};
use rune_kit::diff::FileDiff;
use rune_kit::syntax::SyntaxTheme;
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, Markdown, Spinner};
use rune_theme_opencode::icons;

use crate::app::{App, Item, Kind, Pending, Thread, ToolState};
use crate::harness::{Decision, ToolBody};
use crate::shortcuts::Cmd;

/// The column the conversation reads in.
pub const MAX_W: f32 = 760.0;

fn rows(s: &App) -> usize {
    s.thread().map_or(0, |t| t.items.len() + t.busy as usize)
}

/// `look` is the theme and appearance: finished messages are memoised on it. `clear` is how
/// tall the composer floating over the transcript's foot is: the last row ends above it.
pub fn view(s: &App, k: &Kit, look: (Theme, bool), clear: f32) -> El {
    let n = rows(s);
    let thread = s.thread().map_or(0, |t| t.id);
    col().key("transcript").w_full().grow(1.0).min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: true })
        .scroll_edge_fade(18.0)
        .child(
            col().key(format!("rows-{thread}")).w_full().growth(GrowthSpec::default()).virtual_rows(
                n,
                64.0,
                |s: &App, i| {
                    let t = s.thread().unwrap();
                    match t.items.get(i) {
                        Some(item) => Cow::Owned(format!("i{}", item.id)),
                        None => Cow::Borrowed("working"),
                    }
                },
                {
                    let k = k.clone();
                    move |s: &App, i| {
                        let t = s.thread().unwrap();
                        let last = i + 1 == rows(s);
                        let body = match t.items.get(i) {
                            Some(item) => item_view(s, t, item, &k, look),
                            None => working(t, &s.keys.label(crate::shortcuts::Cmd::Stop), &k),
                        };
                        // Rows carry their own vertical padding: virtual rows take no gaps.
                        row().w_full().justify_center().px(24.0).pt(if i == 0 { 20.0 } else { 6.0 }).pb(if last { clear + 8.0 } else { 6.0 })
                            .child(col().w_full().max_w(MAX_W).child(body))
                    }
                },
            ),
        )
}

fn item_view(s: &App, t: &Thread, item: &Item, k: &Kit, look: (Theme, bool)) -> El {
    let streaming = matches!(item.kind, Kind::Assistant { streaming: true, .. });
    let open = s.expanded.contains(&item.id);
    // What a file row's button opens it in; its name is in the button's tooltip.
    let editor = crate::editors::for_files(s.editor.as_deref());
    let build = || match &item.kind {
        Kind::User(m) => user(m, k),
        Kind::Assistant { text, streaming } => {
            Markdown::new(text.as_str()).style(theme::markdown(look.0, k)).selectable(true).streaming(*streaming).build(k)
        }
        Kind::Tool { name, detail, state, output, body, .. } => {
            let syn = theme::syntax(look.0, k);
            let card = tool(item.id, name, detail, *state, output, body, open, t.ask_on(item.id).is_some(), editor, &syn, k);
            match t.ask_on(item.id) {
                Some(p) => card.child(approval(s, t.id, p, k)),
                None => card,
            }
        }
        Kind::Error(m) => error(m, k),
        Kind::TurnEnd { secs, cost_usd, stopped } => turn_end(*secs, *cost_usd, *stopped, t, k),
    };
    if streaming {
        // A streaming message changes every delta: building it is the work.
        build().streaming(true)
    } else {
        memo(format!("item-{}", item.id), (item.rev, open, look, editor.id), build)
    }
}

fn user(m: &str, k: &Kit) -> El {
    let c = &k.c;
    row().w_full().justify_end().child(
        col().max_w(MAX_W * 0.78).px(14.0).py(10.0).radius(16.0).fill(c.surface_raised_base).border(1.0, c.border_weak)
            .child(text(m.to_owned()).font(k.ty.body).color(c.text_strong).wrap().selectable(true).selection_color(k.selection())),
    )
}

fn tool_icon(name: &str) -> &'static Icon {
    match name.to_ascii_lowercase().as_str() {
        "bash" | "shell" | "run_shell_command" => &icons::CONSOLE,
        "edit" | "write" | "multiedit" | "notebookedit" | "replace" | "write_file" => &icons::PENCIL_LINE,
        "read" | "read_file" | "read_many_files" => &icons::FILE_DOCUMENT,
        "grep" | "glob" | "search" | "search_file_content" | "list_directory" | "ls" => &icons::MAGNIFYING_GLASS,
        "webfetch" | "websearch" | "web_fetch" | "google_web_search" => &icons::GLOBE,
        "task" | "agent" => &icons::TASK,
        "todowrite" => &icons::CHECKLIST,
        _ => &icons::CODE,
    }
}

#[allow(clippy::too_many_arguments)]
fn tool(id: u64, name: &str, detail: &str, state: ToolState, output: &str, body: &ToolBody, open: bool, waiting: bool, editor: crate::editors::Editor, syn: &SyntaxTheme, k: &Kit) -> El {
    let c = &k.c;
    let status = match state {
        ToolState::Running if waiting => text("Needs approval").font(k.ty.text_xsmall_medium).color(c.text_base),
        ToolState::Running => Spinner::new().size(12.0).build(k),
        ToolState::Ok => icon(&icons::CHECK_SMALL, 14.0).color(c.icon_weaker),
        ToolState::Failed => icon(&icons::CIRCLE_X, 13.0).color(c.danger.fg),
    };
    let has_output = !output.trim().is_empty() || !matches!(body, ToolBody::None);
    // An edit's diffs, made once for both the row's counts and its opened body.
    let diffs: Vec<FileDiff> = match body {
        ToolBody::Edits(edits) => edits.iter().map(|(old, new)| edit_diff(detail, old, new)).collect(),
        _ => Vec::new(),
    };
    let mut head = row().key("head").w_full().h(30.0).px(10.0).gap(8.0).center_y().radius(8.0).hover_group()
        .child(icon(tool_icon(name), 14.0).color(c.icon_muted))
        .child(text(name.to_owned()).font(k.ty.small_medium).color(c.text_base))
        .child(text(detail.to_owned()).font(k.ty.mono).color(c.text_weak).truncate().shrink().grow(1.0));
    if let Some((added, removed)) = edit_counts(&diffs) {
        head = head.child(row().gap(6.0).center_y()
            .child(text(format!("+{added}")).font(k.ty.text_xsmall_medium).color(c.success.icon))
            .child(text(format!("\u{2212}{removed}")).font(k.ty.text_xsmall_medium).color(c.danger.icon)));
    }
    // A read, edit or write: open the file in the editor, at the line, from under the pointer.
    if crate::app::file_tool(name) && !detail.trim().is_empty() {
        let file = detail.rsplit(['/', '\\']).next().unwrap_or(detail);
        let verb = if editor.kind == crate::editors::Kind::Files { format!("Show {file} in {}", editor.name) } else { format!("Open {file} in {}", editor.name) };
        head = head.child(IconButton::new(&icons::SQUARE_ARROW_TOP_RIGHT)
            .variant(IconButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .label(verb.clone())
            .help(verb)
            .on_press(move |s: &mut App| s.open_tool_file(id))
            .build(k)
            .focusable(false)
            .group_opacity(0.0, 1.0)
            .key("open-file"));
    }
    if has_output {
        head = head.child(icon(if open { &icons::CHEVRON_DOWN } else { &icons::CHEVRON_RIGHT }, 12.0).color(c.icon_weaker).a11y_hidden());
    }
    head = head.child(status);
    if has_output {
        head = head.hover(c.surface_hover).cursor(rune::Cursor::Pointer).on_press(move |s: &mut App| {
            if !s.expanded.remove(&id) { s.expanded.insert(id); }
        });
    }
    let mut card = col().w_full().radius(9.0).border(1.0, c.border_weak).clip_rounded(9.0).child(head);
    if open && has_output {
        card = card.child(rune::el().w_full().h(1.0).fill(c.border_weak)).child(opened(name, detail, state, output, body, &diffs, syn, k));
    }
    card
}

// ---------------------------------------------------------------------------------------------
// Code in tool rows

/// The most lines an opened row draws; it scrolls inside `CODE_MAX_H`.
const CODE_LINES: usize = 400;
const CODE_MAX_H: f32 = 360.0;

/// What an opened tool row shows: an edit as a diff, a written file or a long command as code,
/// then the output. A file read comes back numbered (`cat -n`), so it's drawn as the file, in
/// its language, beside its line numbers. Output an edit or a write only confirms is left out.
#[allow(clippy::too_many_arguments)]
fn opened(name: &str, detail: &str, state: ToolState, output: &str, body: &ToolBody, diffs: &[FileDiff], syn: &SyntaxTheme, k: &Kit) -> El {
    let c = &k.c;
    let lower = name.to_ascii_lowercase();
    let shell = matches!(lower.as_str(), "bash" | "shell" | "run_shell_command");
    // Only a read's output is the file numbered; a command's digits-then-tab (`du`, `git
    // shortlog -sn`) are its own.
    let read = matches!(lower.as_str(), "read" | "read_file" | "read_many_files");
    let path_lang = language(detail);
    let mut col_ = col().key("output").w_full().max_h(CODE_MAX_H).scroll(ScrollSpec { stick_bottom: false }).py(6.0);
    let mut confirms = false;
    match body {
        ToolBody::Edits(_) => {
            for (i, d) in diffs.iter().enumerate() {
                if i > 0 { col_ = col_.child(rune::el().w_full().h(1.0).my(4.0).fill(c.border_weak)); }
                col_ = col_.child(diff(language(detail), d, syn, k).key(format!("edit-{i}")));
            }
            confirms = true;
        }
        ToolBody::Code(code) => {
            let lang = if shell { Some("bash") } else { path_lang };
            col_ = col_.child(code_block(code, lang, (!shell).then_some(1), syn, k).key("body"));
            confirms = !shell;
        }
        ToolBody::None => {}
    }
    // An edit's or a write's "has been updated" adds nothing to the diff; a failure does.
    let show_output = !output.trim().is_empty() && !(confirms && state == ToolState::Ok);
    if show_output {
        if !matches!(body, ToolBody::None) { col_ = col_.child(rune::el().w_full().h(1.0).my(4.0).fill(c.border_weak)); }
        let out = match numbered(output).filter(|_| read) {
            Some((first, code)) => code_block(&code, path_lang, Some(first), syn, k),
            // `git diff` and the like print a diff; other output is the program's own text.
            None if shell && ["git diff", "git show", "git log -p"].iter().any(|p| detail.starts_with(p)) => code_block(output, Some("diff"), None, syn, k),
            None => code_block(output, None, None, syn, k),
        };
        col_ = col_.child(out.key("result"));
    }
    col_
}

/// A path's language for the highlighter: its extension, or `None` for one it doesn't know.
fn language(path: &str) -> Option<&str> {
    let name = path.rsplit(['/', '\\']).next()?;
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && rune_kit::syntax::known(ext)).then_some(ext)
}

/// Claude Code's Read output (`     1\tline`, or `     1→line`): the first line's number and the
/// file's text without the numbers. Lines after the numbered run (a note about the file) are
/// dropped. `None` if it isn't numbered.
pub fn numbered(output: &str) -> Option<(u32, String)> {
    let mut first = None;
    let mut text = String::new();
    for line in output.lines() {
        let t = line.trim_start();
        let digits = t.len() - t.trim_start_matches(|ch: char| ch.is_ascii_digit()).len();
        let rest = &t[digits..];
        let code = rest.strip_prefix('\t').or_else(|| rest.strip_prefix('→'));
        match (digits, code) {
            (1.., Some(code)) => {
                if first.is_none() { first = t[..digits].parse().ok(); } else { text.push('\n'); }
                text.push_str(code);
            }
            _ if first.is_none() => return None,
            _ => break,
        }
    }
    first.map(|f| (f, text))
}

/// Code a line to a row: highlighted in `lang` (plain without one), the line numbers from
/// `first` in a gutter when given. Long lines wrap, and the gutter stays with each line's top.
fn code_block(code: &str, lang: Option<&str>, first: Option<u32>, syn: &SyntaxTheme, k: &Kit) -> El {
    let c = &k.c;
    let code = code.trim_end_matches('\n');
    let lines = code.split('\n').count();
    // Only the lines drawn are highlighted.
    let shown = match code.match_indices('\n').nth(CODE_LINES - 1) {
        Some((cut, _)) => &code[..cut],
        None => code,
    };
    let found = rune_kit::syntax::highlight(lang, shown);
    let mut spans = LineSpans::new(syn, &found);
    let gutter = first.map(|f| format!("{}", f as usize + lines).len() as f32 * 7.5 + 8.0);
    let mut out = col().w_full();
    let mut at = 0;
    for (i, line) in shown.split('\n').enumerate() {
        let mut r = row().w_full().px(12.0).items_start();
        if let (Some(f), Some(w)) = (first, gutter) {
            r = r.child(text(format!("{}", f as usize + i)).font(k.ty.mono).color(c.text_faint).w(w).pr(10.0).align(rune::Align::End).a11y_hidden());
        }
        r = r.child(text(line.to_owned()).font(k.ty.mono).color(c.text_base).spans(spans.line(at, at + line.len())).wrap_anywhere().selectable(true).grow(1.0).shrink().min_w(0.0));
        out = out.child(r);
        at += line.len() + 1;
    }
    if lines > CODE_LINES {
        out = out.child(text(format!("{} more lines", lines - CODE_LINES)).font(k.ty.text_xsmall).color(c.text_faint).px(12.0).pt(4.0));
    }
    out
}

/// A code's highlights handed out a line at a time, in order: the highlights are sorted and
/// don't overlap, so one cursor moves through them once (a highlight across lines is kept for
/// the next).
struct LineSpans<'a> {
    syn: &'a SyntaxTheme,
    found: &'a [rune_kit::syntax::Highlight],
    next: usize,
}

impl<'a> LineSpans<'a> {
    fn new(syn: &'a SyntaxTheme, found: &'a [rune_kit::syntax::Highlight]) -> LineSpans<'a> {
        LineSpans { syn, found, next: 0 }
    }

    /// The highlights inside `start..end` of the code, moved to start at the line. Lines are
    /// asked for in order.
    fn line(&mut self, start: usize, end: usize) -> Vec<rune::Span> {
        while self.found.get(self.next).is_some_and(|(r, _)| r.end <= start) { self.next += 1; }
        let mine: Vec<rune_kit::syntax::Highlight> = self.found[self.next..].iter()
            .take_while(|(r, _)| r.start < end)
            .map(|(r, p)| (r.start.max(start) - start..r.end.min(end) - start, *p))
            .collect();
        self.syn.spans(&mine, 0)
    }
}

/// One replacement as a diff: removed lines on a red wash, added on a green one, each signed,
/// the words that changed a shade stronger, all in the file's language.
fn diff(lang: Option<&str>, file: &FileDiff, syn: &SyntaxTheme, k: &Kit) -> El {
    use rune_kit::diff::LineKind;
    let c = &k.c;
    // Highlighted as one text, so a string or comment over several lines colours right.
    let lines: Vec<_> = file.hunks.iter().flat_map(|h| h.lines.iter()).take(CODE_LINES).collect();
    let joined = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
    let found = rune_kit::syntax::highlight(lang, &joined);
    let mut line_spans = LineSpans::new(syn, &found);
    // Dark themes' greens and reds are bright: a lighter wash keeps the code the thing you read.
    let strength = if k.dark { 0.08 } else { 0.12 };
    let mut out = col().w_full();
    let mut at = 0;
    for (i, l) in lines.iter().enumerate() {
        let (sign, ink, wash) = match l.kind {
            LineKind::Added => ("+", c.success.icon, Some(c.success.icon.alpha(strength))),
            LineKind::Removed => ("-", c.danger.icon, Some(c.danger.icon.alpha(strength))),
            LineKind::Context => (" ", c.text_faint, None),
        };
        let mut spans = line_spans.line(at, at + l.text.len());
        if let Some(w) = wash {
            for r in &l.changes {
                let mut sp = rune::Span::new(r.start, r.end);
                sp.bg = Some(w.alpha(0.28));
                spans.push(sp);
            }
        }
        let mut r = row().key(format!("d{i}")).w_full().px(12.0).items_start()
            .child(text(sign).font(k.ty.mono).color(ink).w(16.0).a11y_hidden())
            .child(text(l.text.clone()).font(k.ty.mono).color(c.text_base).spans(spans).wrap_anywhere().selectable(true).grow(1.0).shrink().min_w(0.0));
        if let Some(w) = wash { r = r.fill(w); }
        out = out.child(r);
        at += l.text.len() + 1;
    }
    out
}

/// A replacement's diff. Its texts are snippets, which rarely end in a line break: both get
/// one, so lines added after the last don't mark it as changed for the break it gained.
fn edit_diff(path: &str, old: &str, new: &str) -> FileDiff {
    let end = |s: &str| if s.is_empty() || s.ends_with('\n') { s.to_owned() } else { format!("{s}\n") };
    rune_kit::diff::FileDiff::from_texts(path, &end(old), &end(new))
}

/// "+12 −3" for an edit's row, from its replacements' diffs (`None` for a row that isn't one).
fn edit_counts(diffs: &[FileDiff]) -> Option<(usize, usize)> {
    (!diffs.is_empty()).then(|| diffs.iter().fold((0, 0), |(a, d), f| (a + f.additions(), d + f.deletions())))
}

/// Under a tool row waiting for approval: the full command (or path), why the CLI asks, and
/// Deny / Always allow / Allow. Esc stops the whole turn instead.
fn approval(s: &App, thread: u64, p: &Pending, k: &Kit) -> El {
    let c = &k.c;
    let item = p.item;
    let mut body = col().key("ask").w_full().px(12.0).pt(2.0).pb(10.0).gap(8.0);
    let shown = ask_body(&p.ask.input);
    if !shown.is_empty() {
        body = body.child(col().key("ask-input").w_full().max_h(ASK_MAX_H).scroll(ScrollSpec { stick_bottom: false }).px(10.0).py(8.0).radius(6.0).fill(c.layer_02)
            .child(text(shown).font(k.ty.mono).color(c.text_base).wrap_anywhere().selectable(true)));
    }
    if let Some(why) = &p.ask.reason {
        body = body.child(text(why.clone()).font(k.ty.small).color(c.text_muted).wrap());
    }
    // What "Always allow" would save, so it's never more than the card says.
    let rules = p.ask.rule_labels();
    if !rules.is_empty() {
        body = body.child(text(format!("Always allow saves {} to .claude/settings.local.json", rules.join(", "))).font(k.ty.text_xsmall).color(c.text_faint).wrap());
    }
    let allow_key = s.keys.label(Cmd::Allow);
    let always_key = s.keys.label(Cmd::AlwaysAllow);
    let mut hints = Vec::new();
    if !allow_key.is_empty() { hints.push(format!("{allow_key} allow")); }
    if !always_key.is_empty() && !rules.is_empty() { hints.push(format!("{always_key} always")); }
    let stop_key = s.keys.label(Cmd::Stop);
    if !stop_key.is_empty() { hints.push(format!("{} stops the turn", stop_key.to_lowercase())); }

    let deny = Button::new("Deny")
        .variant(ButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .on_press(move |s: &mut App| s.answer(thread, item, Decision::Deny))
        .build(k)
        .key("deny");
    let allow = Button::new("Allow")
        .variant(ButtonVariant::Contrast)
        .control_size(ControlSize::Small)
        .on_press(move |s: &mut App| s.answer(thread, item, Decision::Allow))
        .build(k)
        .key("allow");
    let mut buttons = row().gap(6.0).center_y().child(deny);
    // "Always" adds the rules the CLI suggests that are safe to keep (`AskRequest::safe_rules`);
    // without any there's nothing to remember.
    if !rules.is_empty() {
        buttons = buttons.child(Button::new("Always allow")
            .variant(ButtonVariant::Outline)
            .control_size(ControlSize::Small)
            .on_press(move |s: &mut App| s.answer(thread, item, Decision::AlwaysAllow))
            .build(k)
            .key("always-allow"));
    }
    body.child(row().w_full().gap(8.0).center_y()
        .child(text(hints.join("  ·  ")).font(k.ty.text_xsmall).color(c.text_faint).truncate().shrink().grow(1.0))
        .child(buttons.child(allow)))
}

/// The approval card's input scrolls past this.
const ASK_MAX_H: f32 = 260.0;

/// What the approval is for, in full: the command, path or URL first, then every other field
/// of the input (a file's content, an MCP tool's arguments) as JSON, so nothing it would do is
/// left off the card. Characters that hide or reorder text are shown by their code.
fn ask_body(input: &serde_json::Value) -> String {
    const MAIN: [&str; 9] = ["command", "file_path", "filePath", "notebook_path", "path", "url", "pattern", "query", "plan"];
    let main = MAIN.iter().find(|k| input.get(**k).is_some_and(|v| v.is_string()));
    let mut out = main.and_then(|k| input[*k].as_str()).map_or_else(String::new, |v| v.trim().to_owned());
    if let Some(fields) = input.as_object() {
        // A command's description is the CLI's own summary, not something it does.
        let rest: serde_json::Map<String, serde_json::Value> = fields.iter()
            .filter(|(k, _)| Some(&k.as_str()) != main && k.as_str() != "description")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if !rest.is_empty() {
            if !out.is_empty() { out.push_str("\n\n"); }
            out.push_str(&serde_json::to_string_pretty(&rest).unwrap_or_default());
        }
    }
    reveal_hidden(&out)
}

/// `s` with characters that hide or reorder text (bidirectional controls, zero-width marks)
/// written out as `⟨U+202E⟩`, so a command can't read differently from what runs.
fn reveal_hidden(s: &str) -> String {
    s.chars().map(|ch| match ch {
        '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}' => format!("⟨U+{:04X}⟩", ch as u32),
        _ => ch.to_string(),
    }).collect()
}

fn error(m: &str, k: &Kit) -> El {
    let c = &k.c;
    row().w_full().px(12.0).py(10.0).gap(8.0).radius(9.0).fill(c.danger.bg).border(1.0, c.danger.border)
        .child(icon(&icons::WARNING, 14.0).color(c.danger.fg))
        .child(text(m.to_owned()).font(k.ty.small).color(c.text_strong).wrap().shrink().grow(1.0).selectable(true))
}

fn turn_end(secs: f64, cost: Option<f64>, stopped: bool, t: &Thread, k: &Kit) -> El {
    let c = &k.c;
    let mut parts = vec![if stopped { format!("Stopped after {}", duration(secs)) } else { format!("Worked for {}", duration(secs)) }];
    if let Some(m) = &t.resolved_model { parts.push(m.clone()); }
    if let Some(usd) = cost { parts.push(format!("${usd:.4}")); }
    let rule = || el_rule(k);
    row().w_full().gap(10.0).center_y().py(4.0)
        .child(rule())
        .child(text(parts.join("  ·  ")).font(k.ty.text_xsmall).color(c.text_faint))
        .child(rule())
}

fn el_rule(k: &Kit) -> El {
    rune::el().h(1.0).grow(1.0).fill(k.c.border_weak)
}

fn working(t: &Thread, stop_key: &str, k: &Kit) -> El {
    let c = &k.c;
    let elapsed = t.turn_started.map_or(0.0, |s| s.elapsed().as_secs_f64());
    let status = if !t.asks.is_empty() { "Waiting for approval…" } else if t.steering { "Steering…" } else { "Working…" };
    let mut r = row().key("working").gap(8.0).center_y().h(24.0).px(2.0)
        .child(Spinner::new().size(12.0).build(k))
        .child(text(format!("{status}  {}", duration(elapsed))).font(k.ty.small).color(c.text_muted));
    if !stop_key.is_empty() {
        r = r.child(text(format!("{} to stop", stop_key.to_lowercase())).font(k.ty.text_xsmall).color(c.text_faint).ml(6.0));
    }
    r
}

pub fn duration(secs: f64) -> String {
    if secs < 60.0 { format!("{secs:.0}s") } else { format!("{}m {:02}s", (secs / 60.0) as u64, secs as u64 % 60) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_is_the_file_without_its_numbers() {
        assert_eq!(numbered("     7\tfn a() {\n     8\t}\n"), Some((7, "fn a() {\n}".to_owned())));
        assert_eq!(numbered("    12→let x = 1;"), Some((12, "let x = 1;".to_owned())));
        assert_eq!(numbered("     1\tok\n\n<system-reminder>note</system-reminder>"), Some((1, "ok".to_owned())), "a note after the file is dropped");
        assert_eq!(numbered("running 3 tests\n1\tx"), None, "output that isn't numbered stays as it is");
        assert_eq!(numbered(""), None);
    }

    /// The approval card shows the whole input: a Write's content, an MCP tool's other fields,
    /// all of a long command, and hidden characters by their code.
    #[test]
    fn the_approval_card_shows_everything_the_tool_would_do() {
        use serde_json::json;
        let write = ask_body(&json!({"file_path": "/a/.zshrc", "content": "curl x | sh"}));
        assert!(write.starts_with("/a/.zshrc") && write.contains("curl x | sh"), "{write}");
        let mcp = ask_body(&json!({"query": "status", "sql": "DROP TABLE users"}));
        assert!(mcp.contains("DROP TABLE users"), "{mcp}");
        let long = (1..=30).map(|i| format!("echo {i}")).collect::<Vec<_>>().join("\n");
        assert!(ask_body(&json!({"command": long, "description": "count"})).contains("echo 30"));
        assert!(!ask_body(&json!({"command": "ls", "description": "list"})).contains("list"), "the CLI's summary is left out");
        assert_eq!(ask_body(&json!({"command": "ls\u{202E}txt"})), "ls⟨U+202E⟩txt");
    }

    #[test]
    fn languages_come_from_known_extensions() {
        assert_eq!(language("src/client.rs"), Some("rs"));
        assert_eq!(language("/a/b/App.tsx"), Some("tsx"));
        assert_eq!(language("Cargo.lock"), None);
        assert_eq!(language(".gitignore"), None);
    }

    #[test]
    fn an_edit_counts_its_lines() {
        assert_eq!(edit_counts(&[edit_diff("a.rs", "a\nb\nc", "a\nB\nc\nd")]), Some((2, 1)));
        assert_eq!(edit_counts(&[]), None);
    }

    /// Spans a line at a time match spans found line by line, including a highlight across a
    /// line break (a block comment).
    #[test]
    fn line_spans_hand_out_each_lines_highlights() {
        let k = rune_theme_opencode::kit(true);
        let syn = SyntaxTheme::from_kit(k);
        let code = "let a = 1; /* x\ny */ let b = \"s\";\nfn c() {}";
        let found = rune_kit::syntax::highlight(Some("rs"), code);
        let mut cursor = LineSpans::new(&syn, &found);
        let mut at = 0;
        for line in code.split('\n') {
            let end = at + line.len();
            let want: Vec<(u32, u32)> = found.iter().filter(|(r, _)| r.end > at && r.start < end).map(|(r, _)| ((r.start.max(at) - at) as u32, (r.end.min(end) - at) as u32)).collect();
            let got: Vec<(u32, u32)> = cursor.line(at, end).iter().map(|s| (s.start, s.end)).collect();
            assert_eq!(got, want, "line {line:?}");
            at = end + 1;
        }
    }

    /// Random text (multi-byte characters, tabs, arrows, digits, line breaks, quotes and
    /// comment marks) through everything that slices by byte offsets: a read's numbers, the
    /// highlighter a line at a time, diffs, and the opened row as a whole. None may panic.
    #[test]
    fn fuzzed_tool_rows_never_panic() {
        let k = rune_theme_opencode::kit(true);
        let syn = SyntaxTheme::from_kit(k);
        let pieces = ["a", "é", "日本", "🦀", "\t", "→", "1", "42", " ", "\n", "\n\n", "\"", "'", "/*", "*/", "//", "#", "`", "\\", "\r\n", "fn ", "let x = ", "-", "+", "@@"];
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; seed };
        let mut text = |n: u64| -> String { (0..n).map(|_| pieces[(next() % pieces.len() as u64) as usize]).collect() };
        for i in 0..1500u64 {
            let (a, b, out) = (text(i % 60), text((i * 7) % 60), text((i * 3) % 80));
            let lang = ["rs", "ts", "py", "sh", "json", "diff", "html", "yaml", "nope"][(i % 9) as usize];
            let path = format!("/p/f.{lang}");
            let _ = numbered(&out);
            let d = edit_diff(&path, &a, &b);
            let _ = diff(language(&path), &d, &syn, k);
            let _ = code_block(&a, language(&path), Some(1), &syn, k);
            for name in ["Read", "Edit", "Write", "Bash"] {
                let body = match name {
                    "Edit" => ToolBody::Edits(vec![(a.clone(), b.clone())]),
                    "Write" => ToolBody::Code(b.clone()),
                    _ => ToolBody::None,
                };
                let diffs: Vec<FileDiff> = match &body { ToolBody::Edits(e) => e.iter().map(|(o, n)| edit_diff(&path, o, n)).collect(), _ => Vec::new() };
                let _ = opened(name, &path, ToolState::Ok, &out, &body, &diffs, &syn, k);
            }
        }
    }
}
