//! Skills across the agents (Settings › Skills). Each CLI reads skills from its own folders:
//!
//! | Folder | Claude Code | Codex | OpenCode |
//! | --- | --- | --- | --- |
//! | `~/.agents/skills` (shared), a project's `.agents/skills` | | ✓ | ✓ |
//! | `~/.claude/skills`, a project's `.claude/skills` | ✓ | | ✓ |
//! | `~/.codex/skills` (Codex's old place) | | ✓ | |
//! | `~/.config/opencode/skill(s)`, a project's `.opencode/skill(s)` | | | ✓ |
//!
//! So a skill added for one isn't seen by the others. Sharing a skill links its folder into
//! the folders of the agents that can't see it (`~/.claude/skills/<name>` for Claude Code,
//! `~/.agents/skills/<name>` for Codex), never moving or copying it, so a skill another tool
//! installed (and updates or removes) stays where it put it. Yoke keeps a list of the links it
//! made, and Stop sharing removes only those. A new skill made here lives in `~/.agents/skills`
//! with a link for Claude Code. Skills synced from claude.ai (`~/.claude/skills/synced`) are
//! written for Claude Code's own tools, so they stay Claude Code's.

use std::path::{Path, PathBuf};

use crate::harness::Harness;

/// A folder skills are read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Shared,
    Claude,
    ClaudeSynced,
    CodexOld,
    OpenCode,
}

impl Place {
    /// Whether `h`'s CLI reads skills from here.
    pub fn read_by(self, h: Harness) -> bool {
        match (self, h) {
            (Place::Claude | Place::ClaudeSynced, Harness::Claude) => true,
            (Place::Shared | Place::CodexOld, Harness::Codex) => true,
            (_, Harness::OpenCode) => self != Place::CodexOld,
            _ => false,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Place::Shared => "Shared",
            Place::Claude => "Claude Code",
            Place::ClaudeSynced => "claude.ai",
            Place::CodexOld => "Codex",
            Place::OpenCode => "OpenCode",
        }
    }
}

/// Where a scan looks: a home folder (`~`), or a project folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    User,
    Project,
}

/// The folders of `place` under `root` (a home or a project folder).
fn dirs(root: &Path, scope: Scope, place: Place) -> Vec<PathBuf> {
    match (scope, place) {
        (Scope::User, Place::Shared) | (Scope::Project, Place::Shared) => vec![root.join(".agents/skills")],
        (_, Place::Claude) => vec![root.join(".claude/skills")],
        (Scope::User, Place::ClaudeSynced) => vec![root.join(".claude/skills/synced")],
        (Scope::User, Place::CodexOld) => vec![root.join(".codex/skills")],
        (Scope::Project, Place::CodexOld) => vec![root.join(".codex/skills")],
        (Scope::User, Place::OpenCode) => vec![root.join(".config/opencode/skills"), root.join(".config/opencode/skill")],
        (Scope::Project, Place::OpenCode) => vec![root.join(".opencode/skills"), root.join(".opencode/skill")],
        (Scope::Project, Place::ClaudeSynced) => Vec::new(),
    }
}

/// The folder a share links into for `h` (the one it reads that every other agent's skill
/// can be linked into), or None if it already reads every folder that matters.
fn link_place(h: Harness) -> Option<Place> {
    match h {
        Harness::Claude => Some(Place::Claude),
        Harness::Codex => Some(Place::Shared),
        Harness::OpenCode => None,
    }
}

/// One folder a skill was found in.
#[derive(Clone, Debug, PartialEq)]
pub struct Copy {
    pub place: Place,
    /// The skill's folder there (`…/skills/<name>`).
    pub path: PathBuf,
    /// Where it points, if it's a link.
    pub link: Option<PathBuf>,
}

/// A skill, by its folder name, with every place it was found.
#[derive(Clone, Debug, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub scope: Scope,
    pub copies: Vec<Copy>,
}

impl Skill {
    /// Whether `h`'s CLI sees it.
    pub fn seen_by(&self, h: Harness) -> bool {
        self.copies.iter().any(|c| c.place.read_by(h))
    }

    /// The folder its files are in: the first copy that isn't a link (else where a link
    /// points).
    pub fn home(&self) -> Option<PathBuf> {
        self.copies.iter().find(|c| c.link.is_none()).map(|c| c.path.clone())
            .or_else(|| self.copies.first().and_then(|c| c.link.clone()))
    }

    /// Synced from claude.ai: Claude Code's alone.
    pub fn claude_only(&self) -> bool {
        self.copies.iter().any(|c| c.place == Place::ClaudeSynced)
    }

    /// Two copies that aren't the same folder (neither links to the other): the agents may
    /// each be reading a different one.
    pub fn diverged(&self) -> bool {
        let real: Vec<PathBuf> = self.copies.iter().map(|c| canonical(c.link.as_ref().unwrap_or(&c.path))).collect();
        real.iter().any(|r| *r != real[0])
    }

    /// Whether a share would give some agent it.
    pub fn shareable(&self) -> bool {
        !self.claude_only() && crate::harness::ALL.into_iter().any(|h| !self.seen_by(h) && link_place(h).is_some())
    }
}

fn canonical(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_owned())
}

/// The skills under `root` (a home folder for `Scope::User`, else a project), by name.
pub fn scan(root: &Path, scope: Scope) -> Vec<Skill> {
    let mut skills: Vec<Skill> = Vec::new();
    for place in [Place::Shared, Place::Claude, Place::ClaudeSynced, Place::CodexOld, Place::OpenCode] {
        for dir in dirs(root, scope, place) {
            let mut found = Vec::new();
            if place == Place::ClaudeSynced {
                // synced/<account>/<name>/SKILL.md
                for account in read_dir(&dir) {
                    found.extend(read_dir(&account));
                }
            } else {
                found = read_dir(&dir);
            }
            for path in found {
                let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else { continue };
                if name.starts_with('.') || (place == Place::Claude && name == "synced") { continue; }
                let file = path.join("SKILL.md");
                let Ok(text) = std::fs::read_to_string(&file) else { continue };
                let link = std::fs::read_link(&path).ok().map(|l| if l.is_absolute() { l } else { path.parent().unwrap_or(root).join(l) });
                let copy = Copy { place, path, link };
                match skills.iter_mut().find(|s| s.name == name) {
                    Some(s) => s.copies.push(copy),
                    None => skills.push(Skill { description: frontmatter(&text, "description").unwrap_or_default(), name, scope, copies: vec![copy] }),
                }
            }
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

fn read_dir(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    out.sort();
    out
}

/// A field of a SKILL.md's YAML front matter: `key: value`, quoted or not, or a `|` / `>`
/// block on the lines below, joined with spaces.
pub fn frontmatter(text: &str, key: &str) -> Option<String> {
    let body = text.strip_prefix("---")?.split("\n---").next()?;
    let mut lines = body.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(rest) = line.strip_prefix(key).and_then(|r| r.trim_start().strip_prefix(':')) else { continue };
        let v = rest.trim();
        if v.starts_with('|') || v.starts_with('>') || v.is_empty() {
            let mut parts = Vec::new();
            while let Some(next) = lines.peek().filter(|l| l.starts_with(' ') || l.starts_with('\t') || l.trim().is_empty()) {
                if !next.trim().is_empty() { parts.push(next.trim().to_owned()); }
                lines.next();
            }
            return Some(parts.join(" "));
        }
        let unquoted = v.strip_prefix('"').and_then(|x| x.strip_suffix('"')).or_else(|| v.strip_prefix('\'').and_then(|x| x.strip_suffix('\''))).unwrap_or(v);
        return Some(unquoted.replace("\\\"", "\""));
    }
    None
}

/// A skill name as the Agent Skills spec has it: 1–64 lowercase letters, digits and
/// hyphens, not starting or ending with one or doubling it.
pub fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-') && !name.ends_with('-') && !name.contains("--")
}

/// Links `skill` into the folders of the agents that can't see it, under `root` (the home or
/// project folder it was scanned from). Returns the links made, for Stop sharing.
pub fn share(skill: &Skill, root: &Path) -> Result<Vec<PathBuf>, String> {
    if skill.claude_only() {
        return Err(format!("{} is synced from claude.ai for Claude Code's own tools, so it stays Claude Code's.", skill.name));
    }
    let home = skill.home().ok_or("The skill's folder is gone.")?;
    let mut made = Vec::new();
    for h in crate::harness::ALL {
        if skill.seen_by(h) { continue; }
        let Some(place) = link_place(h) else { continue };
        let Some(dir) = dirs(root, skill.scope, place).into_iter().next() else { continue };
        let at = dir.join(&skill.name);
        if made.contains(&at) { continue; }
        if std::fs::symlink_metadata(&at).is_ok() {
            return Err(format!("{} already has something at {}; move it aside first.", skill.name, at.display()));
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't make {}: {e}", dir.display()))?;
        // In a project, a relative link, so the repository can be moved or cloned.
        let target = match skill.scope {
            Scope::Project => relative(&home, &dir).unwrap_or_else(|| home.clone()),
            Scope::User => home.clone(),
        };
        symlink(&target, &at).map_err(|e| format!("Couldn't link {}: {e}", at.display()))?;
        made.push(at);
    }
    Ok(made)
}

/// Removes the links in `links` that are still links (never a real folder).
pub fn unshare(links: &[PathBuf]) {
    for l in links {
        if std::fs::symlink_metadata(l).is_ok_and(|m| m.file_type().is_symlink()) {
            let _ = std::fs::remove_file(l);
        }
    }
}

/// Makes a new skill `name` in the shared folder under `home`, with a SKILL.md to fill in,
/// linked for Claude Code. Returns its SKILL.md and the link made.
pub fn create(home: &Path, name: &str, description: &str) -> Result<(PathBuf, Vec<PathBuf>), String> {
    if !valid_name(name) {
        return Err("A skill's name is 1–64 lowercase letters, digits and single hyphens.".into());
    }
    let dir = home.join(".agents/skills").join(name);
    if std::fs::symlink_metadata(&dir).is_ok() || scan(home, Scope::User).iter().any(|s| s.name == name) {
        return Err(format!("There's already a skill called {name}."));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't make {}: {e}", dir.display()))?;
    let what = if description.trim().is_empty() { "What this skill does, and when to use it." } else { description.trim() };
    let file = dir.join("SKILL.md");
    let body = format!("---\nname: {name}\ndescription: {}\n---\n\n# {name}\n\nInstructions for the agent go here.\n", yaml_string(what));
    std::fs::write(&file, body).map_err(|e| format!("Couldn't write {}: {e}", file.display()))?;
    let skill = scan(home, Scope::User).into_iter().find(|s| s.name == name).ok_or("The new skill wasn't found.")?;
    let links = share(&skill, home)?;
    Ok((file, links))
}

fn yaml_string(s: &str) -> String {
    let one_line = s.replace(['\n', '\r'], " ");
    if one_line.contains(": ") || one_line.starts_with(['"', '\'', '#', '-', '[', '{', '>', '|', '*', '&', '!', '%', '@', '`']) {
        format!("\"{}\"", one_line.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        one_line
    }
}

/// `to` relative to the folder `from` (both absolute), for a link that lives in `from`.
fn relative(to: &Path, from: &Path) -> Option<PathBuf> {
    let (to, from) = (canonical(to), canonical(from));
    let common = to.components().zip(from.components()).take_while(|(a, b)| a == b).count();
    if common == 0 { return None; }
    let mut out = PathBuf::new();
    for _ in from.components().skip(common) { out.push(".."); }
    for c in to.components().skip(common) { out.push(c); }
    Some(out)
}

#[cfg(unix)]
fn symlink(target: &Path, at: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, at)
}

#[cfg(windows)]
fn symlink(target: &Path, at: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, at)
}

// ---------------------------------------------------------------------------------------------
// Settings › Skills

/// The links Yoke made sharing skills (paths), so Stop sharing removes only those.
const LINKS_KEY: &str = "skills.links";

#[derive(Default)]
pub struct SkillsPage {
    /// Your skills (under your home folder) and the current project's.
    pub user: Vec<Skill>,
    pub project: Vec<Skill>,
    /// Why the last action failed.
    pub error: Option<String>,
    /// The new skill's name, while the New skill row is open.
    pub new_name: Option<rune::TextEdit>,
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

impl crate::app::App {
    fn skill_links(&self) -> Vec<PathBuf> {
        self.prefs.get::<Vec<String>>(LINKS_KEY).unwrap_or_default().into_iter().map(PathBuf::from).collect()
    }

    fn set_skill_links(&mut self, links: &[PathBuf]) {
        let v: Vec<String> = links.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        let _ = self.prefs.set(LINKS_KEY, &v);
    }

    /// Reads the skill folders again (opening the page, and after each change).
    pub fn scan_skills(&mut self) {
        self.skills.user = home_dir().map(|h| scan(&h, Scope::User)).unwrap_or_default();
        self.skills.project = self.current.as_deref().map(|p| scan(p, Scope::Project)).unwrap_or_default();
    }

    fn skill_root(&self, scope: Scope) -> Option<PathBuf> {
        match scope {
            Scope::User => home_dir(),
            Scope::Project => self.current.clone(),
        }
    }

    pub fn share_skill(&mut self, name: &str, scope: Scope) {
        let list = if scope == Scope::User { &self.skills.user } else { &self.skills.project };
        let (Some(skill), Some(root)) = (list.iter().find(|s| s.name == name).cloned(), self.skill_root(scope)) else { return };
        match share(&skill, &root) {
            Ok(made) => {
                let mut links = self.skill_links();
                links.extend(made);
                self.set_skill_links(&links);
                self.skills.error = None;
            }
            Err(e) => self.skills.error = Some(e),
        }
        self.scan_skills();
    }

    /// The links Yoke made for `skill`, still there.
    pub fn links_of(&self, skill: &Skill) -> Vec<PathBuf> {
        let ours = self.skill_links();
        skill.copies.iter().filter(|c| c.link.is_some() && ours.contains(&c.path)).map(|c| c.path.clone()).collect()
    }

    pub fn unshare_skill(&mut self, name: &str, scope: Scope) {
        let list = if scope == Scope::User { &self.skills.user } else { &self.skills.project };
        let Some(skill) = list.iter().find(|s| s.name == name).cloned() else { return };
        let mine = self.links_of(&skill);
        unshare(&mine);
        let rest: Vec<PathBuf> = self.skill_links().into_iter().filter(|l| !mine.contains(l)).collect();
        self.set_skill_links(&rest);
        self.scan_skills();
    }

    pub fn start_new_skill(&mut self) {
        let mut name = rune::TextEdit::new("");
        name.focused = true;
        self.draft.focused = false;
        self.skills.new_name = Some(name);
        self.skills.error = None;
    }

    pub fn cancel_new_skill(&mut self) {
        self.skills.new_name = None;
    }

    /// Makes the skill named in the New skill row and shows its SKILL.md in Finder to fill in.
    pub fn create_skill(&mut self) {
        let Some(name) = self.skills.new_name.as_ref().map(|t| t.text.trim().to_owned()) else { return };
        let Some(home) = home_dir() else { return };
        match create(&home, &name, "") {
            Ok((file, made)) => {
                let mut links = self.skill_links();
                links.extend(made);
                self.set_skill_links(&links);
                self.skills.new_name = None;
                self.skills.error = None;
                reveal(&file);
            }
            Err(e) => self.skills.error = Some(e),
        }
        self.scan_skills();
    }
}

/// Shows `path` in Finder (its folder elsewhere).
pub fn reveal(path: &Path) {
    #[cfg(target_os = "macos")]
    let spawned = std::process::Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let spawned = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let spawned = std::process::Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn();
    let _ = spawned;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(name: &str) -> PathBuf {
        let h = std::env::temp_dir().join(format!("yoke-skills-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&h);
        std::fs::create_dir_all(&h).unwrap();
        h
    }

    fn skill(dir: &Path, name: &str, description: &str) {
        std::fs::create_dir_all(dir.join(name)).unwrap();
        std::fs::write(dir.join(name).join("SKILL.md"), format!("---\nname: {name}\ndescription: {description}\n---\nbody\n")).unwrap();
    }

    #[test]
    fn a_claude_skill_shared_is_linked_for_codex_and_unshared_again() {
        let h = home("share");
        skill(&h.join(".claude/skills"), "rex", "Drive Rex.");
        skill(&h.join(".claude/skills/synced/acct"), "pdf", "PDFs.");
        let all = scan(&h, Scope::User);
        let rex = all.iter().find(|s| s.name == "rex").unwrap();
        assert!(rex.seen_by(Harness::Claude) && rex.seen_by(Harness::OpenCode) && !rex.seen_by(Harness::Codex));
        assert!(rex.shareable());
        let pdf = all.iter().find(|s| s.name == "pdf").unwrap();
        assert!(pdf.claude_only() && !pdf.shareable());
        assert!(share(pdf, &h).is_err());

        let made = share(rex, &h).unwrap();
        assert_eq!(made, vec![h.join(".agents/skills/rex")]);
        let rex = scan(&h, Scope::User).into_iter().find(|s| s.name == "rex").unwrap();
        assert!(crate::harness::ALL.into_iter().all(|a| rex.seen_by(a)));
        assert!(!rex.diverged() && !rex.shareable());
        assert_eq!(rex.home(), Some(h.join(".claude/skills/rex")), "nothing moved");

        unshare(&made);
        assert!(!h.join(".agents/skills/rex").exists());
        assert!(h.join(".claude/skills/rex/SKILL.md").exists());
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn a_new_skill_lives_in_the_shared_folder_linked_for_claude() {
        let h = home("new");
        assert!(create(&h, "Bad Name", "").is_err());
        let (file, links) = create(&h, "release-notes", "Write release notes: from the changelog.").unwrap();
        assert_eq!(file, h.join(".agents/skills/release-notes/SKILL.md"));
        assert_eq!(links, vec![h.join(".claude/skills/release-notes")]);
        let s = scan(&h, Scope::User).into_iter().find(|s| s.name == "release-notes").unwrap();
        assert_eq!(s.description, "Write release notes: from the changelog.");
        assert!(crate::harness::ALL.into_iter().all(|a| s.seen_by(a)));
        assert!(create(&h, "release-notes", "").is_err(), "names are unique");
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn project_skills_share_with_relative_links_and_copies_that_differ_show() {
        let p = home("project");
        skill(&p.join(".agents/skills"), "deploy", "Ship it.");
        let deploy = scan(&p, Scope::Project).into_iter().next().unwrap();
        assert!(!deploy.seen_by(Harness::Claude));
        share(&deploy, &p).unwrap();
        assert_eq!(std::fs::read_link(p.join(".claude/skills/deploy")).unwrap(), PathBuf::from("../../.agents/skills/deploy"));
        assert!(p.join(".claude/skills/deploy/SKILL.md").exists());
        // A second, separate copy with the same name.
        skill(&p.join(".opencode/skills"), "deploy", "Ship it, differently.");
        let deploy = scan(&p, Scope::Project).into_iter().next().unwrap();
        assert!(deploy.diverged());
        let _ = std::fs::remove_dir_all(&p);
    }

    #[test]
    fn front_matter_reads_plain_quoted_and_block_values() {
        let t = "---\nname: x\ndescription: \"Say \\\"hi\\\": politely\"\nmetadata:\n  a: 1\n---\nbody";
        assert_eq!(frontmatter(t, "description").as_deref(), Some("Say \"hi\": politely"));
        let b = "---\nname: x\ndescription: >\n  Two lines\n  joined.\nlicense: MIT\n---\n";
        assert_eq!(frontmatter(b, "description").as_deref(), Some("Two lines joined."));
        assert_eq!(frontmatter("no front matter", "name"), None);
        assert!(valid_name("pdf-tools") && !valid_name("-x") && !valid_name("a--b") && !valid_name("A"));
        assert_eq!(yaml_string("Write notes: now"), "\"Write notes: now\"");
    }
}
