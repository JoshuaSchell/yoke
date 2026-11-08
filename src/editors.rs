//! "Open in …": the editors, terminals and file manager on this computer that can open a
//! thread's folder, as T3 Code's toolbar has. Found once, on first use: on macOS by their app
//! bundles (in /Applications, ~/Applications or the system's), elsewhere by their command on
//! PATH. The one picked last is saved in prefs and is the toolbar button's; the rest are in the
//! menu beside it.
//!
//! A tool row's file opens in the same editor at its line (`Editor::open_file`), through the
//! editor's own command, which apps bundle (Zed's `cli`, VS Code's `bin/code`…) so it works
//! without the command installed on PATH: `zed file:line`, `code -g file:line` (and its forks),
//! `subl file:line`, `xed -l line file`. Without one, VS Code and its forks take a
//! `vscode://file/…:line` link; anything else opens the file at its top.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// One way to open a folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Editor {
    /// Saved in prefs, and the `rune_theme_opencode::app_icons` name where there's a mark.
    pub id: &'static str,
    pub name: &'static str,
    /// The macOS app bundle's name, without `.app`.
    app: &'static str,
    /// The command elsewhere (and the fallback on macOS), taking the folder as its argument.
    cli: &'static str,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Editor,
    Terminal,
    Files,
}

/// In the menu's order: editors, then terminals, then the file manager.
pub const ALL: &[Editor] = &[
    Editor { id: "zed", name: "Zed", app: "Zed", cli: "zed", kind: Kind::Editor },
    Editor { id: "cursor", name: "Cursor", app: "Cursor", cli: "cursor", kind: Kind::Editor },
    Editor { id: "vscode", name: "VS Code", app: "Visual Studio Code", cli: "code", kind: Kind::Editor },
    Editor { id: "windsurf", name: "Windsurf", app: "Windsurf", cli: "windsurf", kind: Kind::Editor },
    Editor { id: "antigravity", name: "Antigravity", app: "Antigravity", cli: "antigravity", kind: Kind::Editor },
    Editor { id: "sublime-text", name: "Sublime Text", app: "Sublime Text", cli: "subl", kind: Kind::Editor },
    Editor { id: "xcode", name: "Xcode", app: "Xcode", cli: "", kind: Kind::Editor },
    Editor { id: "ghostty", name: "Ghostty", app: "Ghostty", cli: "", kind: Kind::Terminal },
    Editor { id: "iterm2", name: "iTerm", app: "iTerm", cli: "", kind: Kind::Terminal },
    Editor { id: "terminal", name: "Terminal", app: "Terminal", cli: "", kind: Kind::Terminal },
    Editor { id: "file-explorer", name: FILES, app: "Finder", cli: "", kind: Kind::Files },
];

#[cfg(target_os = "macos")]
const FILES: &str = "Finder";
#[cfg(target_os = "windows")]
const FILES: &str = "File Explorer";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const FILES: &str = "Files";

/// The ones on this computer, in `ALL`'s order. The file manager is always there.
pub fn installed() -> &'static [Editor] {
    static FOUND: OnceLock<Vec<Editor>> = OnceLock::new();
    FOUND.get_or_init(|| ALL.iter().copied().filter(|e| e.kind == Kind::Files || e.bundle().is_some() || e.on_path().is_some()).collect())
}

/// What opens a file: the saved choice, unless it's a terminal (which can't), then the first
/// editor found, else the file manager (which shows the file in its folder).
pub fn for_files(saved: Option<&str>) -> Editor {
    let chosen = preferred(saved);
    if chosen.kind != Kind::Terminal { return chosen; }
    let found = installed();
    found.iter().find(|e| e.kind == Kind::Editor).or_else(|| found.iter().find(|e| e.kind == Kind::Files)).copied().unwrap_or(ALL[ALL.len() - 1])
}

/// A path as a URL's path: everything but unreserved characters and `/` percent-encoded, so a
/// space, `#` or `%` in it doesn't break the link.
fn url_path(path: &Path) -> String {
    let mut out = String::new();
    for b in path.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) { out.push(b as char); } else { out.push_str(&format!("%{b:02X}")); }
    }
    out
}

/// The saved choice if it's still here, else the first editor found, else the file manager.
pub fn preferred(saved: Option<&str>) -> Editor {
    let found = installed();
    saved.and_then(|id| found.iter().find(|e| e.id == id))
        .or_else(|| found.iter().find(|e| e.kind == Kind::Editor))
        .or_else(|| found.last())
        .copied()
        .unwrap_or(ALL[ALL.len() - 1])
}

impl Editor {
    /// The app bundle, on macOS.
    fn bundle(&self) -> Option<PathBuf> {
        if !cfg!(target_os = "macos") || self.app.is_empty() { return None; }
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let name = format!("{}.app", self.app);
        [PathBuf::from("/Applications"), home.join("Applications"), PathBuf::from("/System/Applications"), PathBuf::from("/System/Applications/Utilities")]
            .into_iter()
            .map(|d| d.join(&name))
            .find(|p| p.exists())
    }

    fn on_path(&self) -> Option<PathBuf> {
        if self.cli.is_empty() { return None; }
        std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(self.cli)).find(|p| p.is_file())
    }

    /// The editor's command: the one its app bundles, else the one on PATH.
    fn launcher(&self) -> Option<PathBuf> {
        let inside = match self.id {
            "zed" => "Contents/MacOS/cli",
            "vscode" => "Contents/Resources/app/bin/code",
            "cursor" => "Contents/Resources/app/bin/cursor",
            "windsurf" => "Contents/Resources/app/bin/windsurf",
            "antigravity" => "Contents/Resources/app/bin/antigravity",
            "sublime-text" => "Contents/SharedSupport/bin/subl",
            _ => "",
        };
        let bundled = (!inside.is_empty()).then(|| self.bundle()).flatten().map(|b| b.join(inside)).filter(|p| p.is_file());
        bundled.or_else(|| self.on_path())
    }

    /// Opens `file` in it at `line` (1-based) where it can go to one, without waiting. The file
    /// manager shows the file in its folder.
    pub fn open_file(&self, file: &Path, line: Option<u32>) -> Result<(), String> {
        let at = |l: u32| format!("{}:{l}", file.display());
        let mut cmd = match (self.kind, self.id) {
            (Kind::Files, _) if cfg!(target_os = "macos") => {
                let mut c = std::process::Command::new("/usr/bin/open");
                c.arg("-R").arg(file);
                c
            }
            (Kind::Files, _) => return self.open(file.parent().unwrap_or(file)),
            (_, "xcode") if Path::new("/usr/bin/xed").exists() => {
                let mut c = std::process::Command::new("/usr/bin/xed");
                if let Some(l) = line { c.arg("-l").arg(l.to_string()); }
                c.arg(file);
                c
            }
            (_, "vscode" | "cursor" | "windsurf" | "antigravity") => match self.launcher() {
                Some(cli) => {
                    let mut c = std::process::Command::new(cli);
                    match line {
                        Some(l) => { c.arg("-g").arg(at(l)); }
                        None => { c.arg(file); }
                    }
                    c
                }
                // The apps answer their own links even without the command.
                None if cfg!(target_os = "macos") && file.is_absolute() && self.bundle().is_some() => {
                    let scheme = if self.id == "vscode" { "vscode" } else { self.id };
                    let mut c = std::process::Command::new("/usr/bin/open");
                    c.arg(format!("{scheme}://file{}:{}", url_path(file), line.unwrap_or(1)));
                    c
                }
                None => return self.open(file),
            },
            (_, "zed" | "sublime-text") => match self.launcher() {
                Some(cli) => {
                    let mut c = std::process::Command::new(cli);
                    match line {
                        Some(l) => { c.arg(at(l)); }
                        None => { c.arg(file); }
                    }
                    c
                }
                None => return self.open(file),
            },
            _ => return self.open(file),
        };
        spawn(&mut cmd, self.name)
    }

    /// Opens `folder` in it, without waiting.
    pub fn open(&self, folder: &Path) -> Result<(), String> {
        let mut cmd = self.command(folder).ok_or_else(|| format!("{} isn't installed", self.name))?;
        spawn(&mut cmd, self.name)
    }

    fn command(&self, folder: &Path) -> Option<std::process::Command> {
        if self.kind == Kind::Files {
            let opener = if cfg!(target_os = "macos") { "open" } else if cfg!(target_os = "windows") { "explorer" } else { "xdg-open" };
            let mut c = std::process::Command::new(opener);
            c.arg(folder);
            return Some(c);
        }
        if let Some(app) = self.bundle() {
            let mut c = std::process::Command::new("/usr/bin/open");
            c.arg("-a").arg(app).arg(folder);
            return Some(c);
        }
        let mut c = std::process::Command::new(self.on_path()?);
        c.arg(folder);
        Some(c)
    }
}

/// Starts `cmd` with no terminal attached and doesn't wait for it.
fn spawn(cmd: &mut std::process::Command, name: &str) -> Result<(), String> {
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let mut child = cmd.spawn().map_err(|e| format!("Couldn't open {name}: {e}"))?;
    // Reaped off the UI thread, so it doesn't linger as a zombie.
    std::thread::spawn(move || { let _ = child.wait(); });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_manager_is_always_there_and_a_saved_choice_wins() {
        assert!(installed().iter().any(|e| e.kind == Kind::Files));
        assert_eq!(preferred(Some("file-explorer")).id, "file-explorer");
        assert_eq!(preferred(Some("not-an-editor")).id, preferred(None).id, "an unknown choice falls back");
        assert!(ALL.iter().all(|e| ALL.iter().filter(|o| o.id == e.id).count() == 1), "ids are unique");
        assert_ne!(for_files(Some("ghostty")).kind, Kind::Terminal, "a terminal doesn't open files");
        assert_eq!(for_files(Some("file-explorer")).kind, Kind::Files, "Finder picked shows the file");
        assert_eq!(url_path(Path::new("/a b/c#d%e.rs")), "/a%20b/c%23d%25e.rs");
    }
}
