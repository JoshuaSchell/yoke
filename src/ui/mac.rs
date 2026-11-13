//! macOS menu bar fixups Rune doesn't do itself (as of the pinned rev):
//!
//! - Register the Window menu as `NSApp.windowsMenu`. Only then does AppKit add Fill, Center
//!   and Move & Resize to it, and those items are what the system's window tiling shortcuts
//!   (Fn-Control-arrows, Fn-Control-F, Fn-Control-C) fire. Without it they do nothing.

use std::cell::Cell;

use objc2_app_kit::NSApplication;
use objc2_foundation::{MainThreadMarker, NSString};

thread_local! {
    static DONE: Cell<bool> = const { Cell::new(false) };
}

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
        // Rune installs the app's own items on a later drain than the standard menus; wait
        // for them, so the menu bar is the one that stays.
        let Some(app_menu) = tops.first().and_then(|i| i.submenu()) else { return false };
        if app_menu.indexOfItemWithTitle(&NSString::from_str(crate::SETTINGS_TITLE)) < 0 { return false; }
        app.setWindowsMenu(Some(&window));
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

// ---------------------------------------------------------------------------------------------
// Asking before quitting
//
// Rune's quit can't be refused: AppKit asks the app delegate `applicationShouldTerminate:`
// (⌘Q, the Dock's Quit, logout) and Rune's answer is always yes, and winit's window delegate
// turns the close button into a close at once. While a thread's CLI is at work (a turn, or work
// in the background that ending it would stop), Yoke answers no instead and asks in its own
// dialog (`App::may_quit`); the dialog's Quit goes through `Tx::quit`, which asks neither.

use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use objc2::{msg_send, sel};
use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};

/// Quitting now would stop work: refuse and ask.
static GUARD: AtomicBool = AtomicBool::new(false);
/// A quit or close was refused since the app last looked.
static ASKED: AtomicBool = AtomicBool::new(false);
/// The methods Yoke's replaced, called when it lets a quit or close through.
static TERMINATE: AtomicUsize = AtomicUsize::new(0);
static SHOULD_CLOSE: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static GUARDED: Cell<(bool, bool)> = const { Cell::new((false, false)) };
}

unsafe extern "C" {
    fn class_getInstanceMethod(class: *const AnyClass, selector: Sel) -> *const c_void;
    fn method_getImplementation(method: *const c_void) -> *const c_void;
    fn class_replaceMethod(class: *const AnyClass, selector: Sel, implementation: *const c_void, types: *const c_char) -> *const c_void;
}

type TerminateFn = extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> usize;
type CloseFn = extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> Bool;

/// `NSTerminateCancel`.
const TERMINATE_CANCEL: usize = 0;

extern "C" fn should_terminate(this: *mut AnyObject, sel: Sel, sender: *mut AnyObject) -> usize {
    if GUARD.load(Ordering::Acquire) {
        ASKED.store(true, Ordering::Release);
        crate::attention::wake();
        return TERMINATE_CANCEL;
    }
    let old = TERMINATE.load(Ordering::Acquire);
    if old == 0 { return 1; }
    // SAFETY: `old` is the method this replaced, with the same signature.
    let f: TerminateFn = unsafe { std::mem::transmute::<usize, TerminateFn>(old) };
    f(this, sel, sender)
}

extern "C" fn window_should_close(this: *mut AnyObject, sel: Sel, sender: *mut AnyObject) -> Bool {
    if GUARD.load(Ordering::Acquire) {
        ASKED.store(true, Ordering::Release);
        crate::attention::wake();
        return Bool::NO;
    }
    let old = SHOULD_CLOSE.load(Ordering::Acquire);
    if old == 0 { return Bool::YES; }
    // SAFETY: as above.
    let f: CloseFn = unsafe { std::mem::transmute::<usize, CloseFn>(old) };
    f(this, sel, sender)
}

/// Replaces `selector` on `object`'s class with `imp`, keeping the old one in `slot`. Once.
///
/// SAFETY: main thread; `imp` has the selector's signature.
unsafe fn hook(object: *mut AnyObject, selector: Sel, imp: *const c_void, types: &std::ffi::CStr, slot: &AtomicUsize) -> bool {
    let Some(object) = (unsafe { object.as_ref() }) else { return false };
    let class = object.class();
    let method = unsafe { class_getInstanceMethod(class, selector) };
    if method.is_null() { return false; }
    let old = unsafe { method_getImplementation(method) };
    if old == imp { return true; }
    slot.store(old as usize, Ordering::Release);
    unsafe { class_replaceMethod(class, selector, imp, types.as_ptr()) };
    true
}

/// Runs from `poll`: `guard` is whether quitting now would stop work. Returns true when a quit
/// or close was refused since the last call, for the app to ask.
pub fn quit_guard(guard: bool) -> bool {
    GUARD.store(guard, Ordering::Release);
    let Some(mtm) = MainThreadMarker::new() else { return false };
    let (app_done, window_done) = GUARDED.with(Cell::get);
    if !(app_done && window_done) {
        // SAFETY: main thread (checked above); both functions have their selectors' ABI
        // (`NSApplicationTerminateReply` is an `NSUInteger`, `BOOL` a `Bool`).
        unsafe {
            let app = NSApplication::sharedApplication(mtm);
            let mut app_hooked = app_done;
            if !app_done {
                let delegate: *mut AnyObject = msg_send![&app, delegate];
                app_hooked = hook(delegate, sel!(applicationShouldTerminate:), should_terminate as TerminateFn as *const c_void, c"Q@:@", &TERMINATE);
            }
            let mut window_hooked = window_done;
            if !window_done {
                let windows: *mut AnyObject = msg_send![&app, windows];
                let count: usize = if windows.is_null() { 0 } else { msg_send![windows, count] };
                for i in 0..count {
                    let w: *mut AnyObject = msg_send![windows, objectAtIndex: i];
                    let delegate: *mut AnyObject = msg_send![w, delegate];
                    if hook(delegate, sel!(windowShouldClose:), window_should_close as CloseFn as *const c_void, c"c@:@", &SHOULD_CLOSE) {
                        window_hooked = true;
                        break;
                    }
                }
            }
            GUARDED.with(|g| g.set((app_hooked, window_hooked)));
        }
    }
    ASKED.swap(false, Ordering::AcqRel)
}
