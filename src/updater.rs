//! Updates. The installed app checks GitHub Releases for a newer Yoke at launch and every few
//! hours. One that's newer downloads in the background, its signature is checked against the
//! public key built in (`assets/updater.pub`, whose private half only the release workflow
//! has), and it replaces the app on disk. Then the toolbar offers "Restart to update".
//!
//! The release workflow publishes `latest.json` beside each release's `Yoke.app.tar.gz`
//! (cargo-packager's update bundle) and its `.sig`; GitHub's `releases/latest/download/…` link
//! always serves the newest one. A build not running from a `.app` (`cargo run`) never updates
//! itself. `YOKE_UPDATE_ENDPOINT` points the check somewhere else, for trying an update locally.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use cargo_packager_updater::{Config, UpdaterBuilder};

use crate::app::App;

/// Where the newest release's update manifest is.
pub const ENDPOINT: &str = "https://github.com/JoshuaSchell/yoke/releases/latest/download/latest.json";
/// The public half of the key the release workflow signs updates with.
pub const PUBKEY: &str = include_str!("../assets/updater.pub");
/// How often a running app looks again.
pub const EVERY: Duration = Duration::from_secs(6 * 3600);

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Not checked yet, or not an installed app.
    Idle,
    Checking,
    UpToDate,
    /// Downloading and installing this version.
    Installing(String),
    /// This version is installed on disk: restarting runs it.
    Ready(String),
    /// Why the last check or install failed; the next check tries again.
    Failed(String),
}

pub struct Updater {
    pub status: Status,
    rx: Option<Receiver<Status>>,
    last: Option<Instant>,
}

impl Updater {
    /// How long ago the last check started.
    pub fn checked(&self) -> Option<Duration> {
        self.last.map(|t| t.elapsed())
    }

    /// A check or a download is under way.
    pub fn working(&self) -> bool {
        matches!(self.status, Status::Checking | Status::Installing(_))
    }
}

impl Default for Updater {
    fn default() -> Updater {
        Updater { status: Status::Idle, rx: None, last: None }
    }
}

/// Whether this process runs from an app bundle, so updates come to it. Asked once.
pub fn installed() -> bool {
    static INSTALLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *INSTALLED.get_or_init(|| bundle().is_some())
}

/// Whether `YOKE_UPDATE_ENDPOINT` may point here: https, or http on this computer.
fn allowed_endpoint(url: &str) -> bool {
    url.starts_with("https://") || ["http://127.0.0.1:", "http://127.0.0.1/", "http://localhost:", "http://localhost/"].iter().any(|p| url.starts_with(p))
}

/// The app bundle this process runs from (`…/Yoke.app`), or `None` outside one.
pub fn bundle() -> Option<PathBuf> {
    bundle_of(&std::env::current_exe().ok()?)
}

fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let app = exe.parent()?.parent()?.parent()?;
    (exe.parent()?.ends_with("Contents/MacOS") && app.extension().is_some_and(|x| x == "app")).then(|| app.to_path_buf())
}

/// Checks `endpoint` for a release newer than `current` and, if there is one, downloads it,
/// checks its signature against `pubkey`, and installs it over the bundle `exe` runs from.
/// Blocks on the network; call it off the UI thread.
pub fn check_and_install(exe: &Path, endpoint: &str, current: &str, pubkey: &str, progress: &dyn Fn(Status)) -> Status {
    let run = || -> Result<Status, String> {
        let config = Config { endpoints: vec![endpoint.parse().map_err(|e| format!("{e}"))?], pubkey: pubkey.trim().into(), ..Default::default() };
        let version = current.parse().map_err(|e| format!("{e}"))?;
        let updater = UpdaterBuilder::new(version, config)
            .executable_path(exe)
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| e.to_string())?;
        let Some(update) = updater.check().map_err(|e| e.to_string())? else { return Ok(Status::UpToDate) };
        progress(Status::Installing(update.version.clone()));
        update.download_and_install().map_err(|e| e.to_string())?;
        Ok(Status::Ready(update.version))
    };
    run().unwrap_or_else(Status::Failed)
}

impl App {
    /// Looks for an update off the UI thread, unless this isn't an installed app, a check is
    /// under way, or an update already waits for a restart.
    pub fn check_for_update(&mut self) {
        if self.update.rx.is_some() || matches!(self.update.status, Status::Ready(_)) || self.waker.is_none() { return; }
        self.update.last = Some(Instant::now());
        // Somewhere else only over https, or on this computer for trying an update locally: a
        // plain-http endpoint elsewhere could offer an older signed build as the newest.
        let endpoint = std::env::var("YOKE_UPDATE_ENDPOINT").ok().filter(|e| allowed_endpoint(e));
        let Some(exe) = std::env::current_exe().ok().filter(|e| bundle_of(e).is_some()) else { return };
        let (tx, rx) = channel();
        self.update.rx = Some(rx);
        self.update.status = Status::Checking;
        let wake = self.waker.clone();
        std::thread::spawn(move || {
            let endpoint = endpoint.unwrap_or_else(|| ENDPOINT.to_owned());
            let send = |s: Status| {
                let _ = tx.send(s);
                if let Some(w) = &wake { w(); }
            };
            let done = check_and_install(&exe, &endpoint, env!("CARGO_PKG_VERSION"), PUBKEY, &send);
            send(done);
        });
    }

    /// Lands the check's progress, and starts the next check when one is due. True when the
    /// status changed.
    pub fn update_tick(&mut self) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.update.rx {
            while let Ok(s) = rx.try_recv() {
                let done = !matches!(s, Status::Checking | Status::Installing(_));
                self.update.status = s;
                changed = true;
                if done { self.update.rx = None; break; }
            }
        }
        if self.update.last.is_none_or(|t| t.elapsed() >= EVERY) { self.check_for_update(); }
        changed
    }

    /// The version waiting for a restart, if any.
    pub fn update_ready(&self) -> Option<&str> {
        match &self.update.status {
            Status::Ready(v) => Some(v),
            _ => None,
        }
    }

    /// Quits and opens the updated app. Refused while a turn runs, since quitting would cut it off.
    pub fn restart_to_update(&mut self) -> bool {
        if self.update_ready().is_none() || self.threads.iter().any(|t| t.busy) { return false; }
        let Some(app) = bundle() else { return false };
        if std::process::Command::new("/usr/bin/open").arg("-n").arg(&app).spawn().is_err() { return false; }
        std::process::exit(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_or_this_computer_can_stand_in_for_releases() {
        assert!(allowed_endpoint("https://example.com/latest.json"));
        assert!(allowed_endpoint("http://127.0.0.1:8080/latest.json"));
        assert!(!allowed_endpoint("http://example.com/latest.json"));
        assert!(!allowed_endpoint("http://127.0.0.1.example.com/latest.json"));
    }

    #[test]
    fn finds_the_bundle_it_runs_from() {
        assert_eq!(bundle_of(Path::new("/Applications/Yoke.app/Contents/MacOS/yoke")), Some(PathBuf::from("/Applications/Yoke.app")));
        assert_eq!(bundle_of(Path::new("/Users/me/yoke/target/release/yoke")), None, "cargo run doesn't update itself");
    }

    /// A whole update against a release's files, served from here: `YOKE_UPDATE_FILES` is a
    /// folder holding a release's `latest.json` and `.app.tar.gz` (the Release workflow's
    /// artifact). It updates a scratch copy of /Applications/Yoke.app, checking the signature
    /// against `assets/updater.pub` on the way:
    /// `YOKE_UPDATE_FILES=… cargo test -- --ignored an_update_installs --nocapture`.
    #[test]
    #[ignore]
    fn an_update_installs() {
        use std::io::{BufRead, BufReader, Write};
        let files = PathBuf::from(std::env::var("YOKE_UPDATE_FILES").expect("YOKE_UPDATE_FILES"));
        let mut manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(files.join("latest.json")).unwrap()).unwrap();
        let version = manifest["version"].as_str().unwrap().to_owned();
        let url = manifest["platforms"]["macos-aarch64"]["url"].as_str().unwrap().to_owned();
        let name = url.rsplit('/').next().unwrap().to_owned();

        // A one-file-at-a-time HTTP server for latest.json and the bundle.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        manifest["platforms"]["macos-aarch64"]["url"] = format!("http://127.0.0.1:{port}/{name}").into();
        let json = manifest.to_string();
        let dir = files.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut line = String::new();
                if BufReader::new(&stream).read_line(&mut line).is_err() { continue; }
                let path = line.split_whitespace().nth(1).unwrap_or("/").trim_start_matches('/').to_owned();
                let body = if path == "latest.json" { json.clone().into_bytes() } else { std::fs::read(dir.join(&path)).unwrap_or_default() };
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(&body);
            }
        });

        let root = std::env::temp_dir().join(format!("yoke-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let app = root.join("Yoke.app");
        assert!(std::process::Command::new("ditto").arg("/Applications/Yoke.app").arg(&app).status().unwrap().success());
        let plist = app.join("Contents/Info.plist");
        let read_version = || {
            let out = std::process::Command::new("/usr/libexec/PlistBuddy").args(["-c", "Print CFBundleShortVersionString"]).arg(&plist).output().unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        let before = read_version();
        assert_ne!(before, version, "the scratch app must be older than the release");

        let exe = app.join("Contents/MacOS/yoke");
        let seen = std::sync::Mutex::new(Vec::new());
        let done = check_and_install(&exe, &format!("http://127.0.0.1:{port}/latest.json"), &before, PUBKEY, &|s| seen.lock().unwrap().push(s));
        eprintln!("{before} -> {done:?}, along the way {:?}", seen.lock().unwrap());
        assert_eq!(done, Status::Ready(version.clone()));
        assert_eq!(read_version(), version, "the bundle on disk is the new version");
        assert!(std::process::Command::new("codesign").arg("-v").arg(&app).status().unwrap().success(), "and its signature holds");

        // A bundle signed by some other key is refused.
        let bad = check_and_install(&exe, &format!("http://127.0.0.1:{port}/latest.json"), "0.0.1", "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEU3RDg0OTY1MDY5Qzg3Q0YKUldUUGg1d0daVW5ZNTZoNDdMVVFHbXd1SWFtMFBWMm1MNVdveEo5Sy8vanJhQUVRak56Wm9UU3MK", &|_| {});
        assert!(matches!(bad, Status::Failed(_)), "{bad:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_public_key_is_a_minisign_key() {
        use base64::Engine;
        let text = base64::engine::general_purpose::STANDARD.decode(PUBKEY.trim()).unwrap();
        assert!(String::from_utf8(text).unwrap().starts_with("untrusted comment: minisign public key"));
    }
}
