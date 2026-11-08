//! Keeping you posted while you're away, as T3 Code does: a system notification and a Dock
//! badge for each thread that finishes, fails or asks for approval while the window isn't
//! focused (the badge clears when it is), and threads idle for a week settling on their own.
//!
//! Notifications need a bundled app on macOS (`cargo packager`, not `cargo run`) and post
//! nothing on Linux; the Dock badge is macOS only. Headless runs (no waker) do neither.

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use rune::{Tx, Waker};

use crate::app::{Alert, App, ThreadId};

/// Clicked notifications' threads, from whichever thread the system calls back on.
static CLICKS: Mutex<Vec<ThreadId>> = Mutex::new(Vec::new());
static WAKER: OnceLock<Waker> = OnceLock::new();

/// How often the auto-settle sweep may run while the app is being used. When idle the app
/// only wakes when the next thread is due, so a quiet window costs nothing.
const SWEEP_EVERY: f64 = 60.0;

/// Call first thing in `main`: gives the process its identity on Windows, where toasts need
/// one, and routes clicks to `poll`.
pub fn install() {
    let _ = rune_notify::register(&rune_notify::Identity::new(crate::app::APP_ID, "Yoke"));
    rune_notify::on_click(|click| {
        if let Some(id) = click.get("thread").and_then(|t| t.parse().ok()) {
            CLICKS.lock().unwrap_or_else(|e| e.into_inner()).push(id);
            if let Some(w) = WAKER.get() { w(); }
        }
    });
}

/// From `AppSpec::setup`: lets a click wake the loop.
pub fn set_waker(w: &Waker) {
    let _ = WAKER.set(w.clone());
}

/// Wakes the loop, from outside it (a refused quit, `ui::mac`).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn wake() {
    if let Some(w) = WAKER.get() { w(); }
}

/// From `poll`: opens clicked threads, posts what happened while away, and settles idle
/// threads. True if anything on screen changed.
pub fn poll(s: &mut App, clock: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    let clicks = std::mem::take(&mut *CLICKS.lock().unwrap_or_else(|e| e.into_inner()));
    for id in clicks {
        if s.threads.iter().any(|t| t.id == id) {
            s.select(id);
            changed = true;
        }
        rune_notify::bring_to_front();
    }
    notify(s);
    changed | sweep(s, clock, tx)
}

/// Posts the alerts `apply` queued, unless the window is focused: then you've seen them.
fn notify(s: &mut App) {
    let alerts = std::mem::take(&mut s.alerts);
    if alerts.is_empty() || !s.has_waker() || rune_notify::app_focused() { return; }
    for (id, alert) in alerts {
        let Some(t) = s.threads.iter().find(|t| t.id == id) else { continue };
        let body = match &alert {
            Alert::Done => "Finished".to_owned(),
            Alert::Failed => match t.items.iter().rev().find_map(|i| if let crate::app::Kind::Error(m) = &i.kind { Some(m) } else { None }) {
                Some(m) => format!("Failed: {}", crate::harness::first_line(m, 120)),
                None => "Failed".to_owned(),
            },
            Alert::Approval(tool) => format!("Wants to use {tool}"),
        };
        // One notification per thread: a newer one replaces the last.
        let n = rune_notify::Notification::new(format!("thread-{id}"), t.title.clone()).body(body).info("thread", id.to_string());
        rune_notify::post(&n);
        s.unseen += 1;
    }
    dock_badge(s.unseen);
}

/// From `App::on_focus`: back at the window, the badge has done its job.
pub fn on_focus(s: &mut App, focused: bool, _: &mut Tx) {
    if focused && s.unseen > 0 {
        s.unseen = 0;
        dock_badge(0);
    }
}

/// Settles threads idle for `App::settle_after`, at most once a minute, and asks to be woken
/// when the next one is due; and now and then looks for merged worktree branches.
fn sweep(s: &mut App, clock: f64, tx: &mut Tx) -> bool {
    s.check_merged(clock);
    if clock < s.next_sweep { return false; }
    s.next_sweep = clock + SWEEP_EVERY;
    let (moved, next) = s.auto_settle();
    if let Some(due) = next {
        tx.wake_after(due.max(Duration::from_secs_f64(SWEEP_EVERY)));
    }
    moved
}

#[cfg(target_os = "macos")]
fn dock_badge(n: usize) {
    crate::ui::mac::dock_badge(n);
}

#[cfg(not(target_os = "macos"))]
fn dock_badge(_: usize) {}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use crate::app::{Alert, App, Lane, Thread};
    use crate::harness::{Event, Harness};

    fn app() -> App {
        App::new(Some(std::env::current_dir().unwrap()), rune::prefs::Prefs::memory())
    }

    fn thread(id: u64, idle_days: u64) -> Thread {
        let mut t = Thread::new(id, format!("t{id}"), Harness::Claude, "opus".into());
        t.updated = SystemTime::now() - Duration::from_secs(idle_days * 24 * 3600);
        t
    }

    #[test]
    fn auto_settle_takes_week_old_idle_threads_only() {
        let mut s = app();
        let mut busy = thread(3, 9);
        busy.busy = true;
        let mut queued = thread(4, 9);
        queued.queue.push(crate::app::Queued { id: 1, text: "x".into(), model: String::new(), effort: String::new(), images: Vec::new(), files: Vec::new(), sent: None });
        s.threads = vec![thread(1, 8), thread(2, 2), busy, queued];
        let (moved, next) = s.auto_settle();
        assert!(moved);
        let lanes: Vec<Lane> = s.threads.iter().map(App::lane).collect();
        assert_eq!(lanes, [Lane::Settled, Lane::Inbox, Lane::Working, Lane::Inbox]);
        // The two-day-old one is due in about five days.
        let due = next.unwrap();
        assert!(due > Duration::from_secs(4 * 24 * 3600) && due < Duration::from_secs(6 * 24 * 3600), "{due:?}");
        // A thread kept in the inbox by hand stays.
        let mut kept = thread(5, 30);
        kept.session = Some("s5".into());
        s.threads.push(kept);
        s.settle(5);
        s.unsettle(5);
        s.auto_settle();
        assert_eq!(App::lane(s.threads.iter().find(|t| t.id == 5).unwrap()), Lane::Inbox);
    }

    /// Settings › Chat › Settle after: one day settles the two-day-old thread; Never none.
    #[test]
    fn settle_after_follows_the_setting() {
        let mut s = app();
        s.set_settle_after(1);
        s.threads = vec![thread(1, 2), thread(2, 0)];
        assert!(s.auto_settle().0);
        assert_eq!(s.threads.iter().map(App::lane).collect::<Vec<_>>(), [Lane::Settled, Lane::Inbox]);
        s.set_settle_after(0);
        s.threads = vec![thread(3, 400)];
        assert_eq!(s.auto_settle(), (false, None));
        assert_eq!(App::lane(&s.threads[0]), Lane::Inbox);
    }

    /// A thread whose branch was merged settles, unless it's running or kept by hand; once it
    /// does something again the same merge doesn't settle it a second time.
    #[test]
    fn merged_branches_settle_once() {
        let mut s = app();
        let in_worktree = |id: u64| {
            let mut t = thread(id, 0);
            t.project = std::path::PathBuf::from("/repo");
            t.worktree = Some(crate::worktree::Worktree { path: format!("/wt/{id}").into(), branch: format!("yoke/b{id}") });
            t
        };
        let mut busy = in_worktree(2);
        busy.busy = true;
        let mut kept = in_worktree(3);
        kept.session = Some("s3".into());
        s.threads = vec![in_worktree(1), busy, kept, in_worktree(4), thread(5, 0)];
        s.settle(3);
        s.unsettle(3);
        let ids = |s: &App| s.merge_candidates().into_iter().map(|(id, _, _)| id).collect::<Vec<_>>();
        assert_eq!(ids(&s), [1, 4], "idle worktree threads only, not one kept by hand");
        s.land_merged(vec![1, 2, 3]);
        assert_eq!(s.threads.iter().map(App::lane).collect::<Vec<_>>(), [Lane::Settled, Lane::Working, Lane::Inbox, Lane::Inbox, Lane::Inbox]);
        s.unsettle(1);
        assert_eq!(ids(&s), [4], "no second look at a thread its merge already settled");
    }

    #[test]
    fn finishing_failing_and_asking_queue_alerts() {
        let mut s = app();
        let mut t = thread(1, 0);
        t.busy = true;
        s.threads = vec![t];
        s.apply(1, Event::Exited { code: Some(0), stderr: String::new() });
        let mut t = thread(2, 0);
        t.busy = true;
        s.threads.push(t);
        s.apply(2, Event::Exited { code: Some(1), stderr: "boom".into() });
        assert_eq!(s.alerts, [(1, Alert::Done), (2, Alert::Failed)]);
        // A turn you stopped isn't news.
        s.alerts.clear();
        let mut t = thread(3, 0);
        t.busy = true;
        t.stop_requested = true;
        s.threads.push(t);
        s.apply(3, Event::Exited { code: None, stderr: String::new() });
        assert!(s.alerts.is_empty());
    }
}
