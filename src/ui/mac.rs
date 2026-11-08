//! macOS menu bar fixups Rune doesn't do itself (as of the pinned rev):
//!
//! - Register the Window menu as `NSApp.windowsMenu`. Only then does AppKit add Fill, Center
//!   and Move & Resize to it, and those items are what the system's window tiling shortcuts
//!   (Fn-Control-arrows, Fn-Control-F, Fn-Control-C) fire. Without it they do nothing.
//! - Put the app's Settings… item where Mac apps have it: after About, in its own group.
//!   Rune inserts custom app-menu items at the top.

use std::cell::Cell;

use objc2::sel;
use objc2_app_kit::{NSApplication, NSMenuItem};
use objc2_foundation::{MainThreadMarker, NSString};

thread_local! {
    static DONE: Cell<bool> = const { Cell::new(false) };
}

pub const SETTINGS_TITLE: &str = "Settings…";

/// Runs from `poll` (main thread) until the menu bar is in place, then never again.
pub fn fix_menus() {
    if DONE.with(Cell::get) { return; }
    if apply() { DONE.with(|d| d.set(true)); }
}

fn apply() -> bool {
    let Some(mtm) = MainThreadMarker::new() else { return false };
    // SAFETY: main thread (checked above); every menu and item is retained while used.
    unsafe {
        let app = NSApplication::sharedApplication(mtm);
        let Some(root) = app.mainMenu() else { return false };
        let tops: Vec<_> = (0..root.numberOfItems()).filter_map(|i| root.itemAtIndex(i)).collect();
        let Some(window) = tops.iter().find(|i| i.title().to_string() == "Window").and_then(|i| i.submenu()) else { return false };
        let Some(app_menu) = tops.first().and_then(|i| i.submenu()) else { return false };
        let items: Vec<_> = (0..app_menu.numberOfItems()).filter_map(|i| app_menu.itemAtIndex(i)).collect();
        // Rune installs custom items on a later drain than the standard menus; wait for it.
        let Some(settings) = items.iter().position(|i| i.title().to_string() == SETTINGS_TITLE) else { return false };

        app.setWindowsMenu(Some(&window));

        let about = sel!(orderFrontStandardAboutPanel:);
        if let Some(a) = items.iter().position(|i| i.action() == Some(about)).filter(|a| settings < *a) {
            let item = items[settings].clone();
            app_menu.removeItem(&item);
            // About moved up one; the new separator and Settings… go right after it.
            app_menu.insertItem_atIndex(&NSMenuItem::separatorItem(mtm), a as isize);
            app_menu.insertItem_atIndex(&item, a as isize + 1);
        }
    }
    true
}

/// The Dock icon's badge: `n`, or none at 0. Main thread only (from `poll` or a focus
/// callback); elsewhere it does nothing.
pub fn dock_badge(n: usize) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    // SAFETY: main thread (checked above).
    unsafe {
        let tile = NSApplication::sharedApplication(mtm).dockTile();
        let label = (n > 0).then(|| NSString::from_str(&n.to_string()));
        tile.setBadgeLabel(label.as_deref());
    }
}
