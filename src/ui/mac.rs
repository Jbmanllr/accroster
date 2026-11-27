//! What the UI does to the rest of the Mac when the core asks: Terminal,
//! the Claude desktop app, bringing a window forward, the Trash, alerts,
//! and watching the network path.

use super::UI;
use accroster_core::Event;
use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSApplication, NSApplicationActivationOptions,
    NSRunningApplication, NSTextField, NSWindow, NSWorkspace,
};
use objc2_foundation::{NSBundle, NSFileManager, NSPoint, NSRect, NSSize, NSString, ns_string};
use std::ffi::{c_int, c_void};

/// Opens a new Terminal window running `line` (already quoted for `sh`).
/// The line is passed as an argument, so nothing is spliced into the script.
pub(super) fn run_in_terminal(line: &str) {
    osascript(
        &[
            "on run argv",
            "tell application \"Terminal\"",
            "activate",
            "do script (item 1 of argv)",
            "end tell",
            "end run",
        ],
        &[line],
    );
}

/// Brings forward the Terminal tab on `/dev/<tty>`, else just Terminal.
pub(super) fn show_in_terminal(tty: Option<&str>) {
    let Some(tty) = tty else {
        osascript(&["tell application \"Terminal\" to activate"], &[]);
        return;
    };
    let device = format!("/dev/{tty}");
    osascript(
        &[
            "on run argv",
            "tell application \"Terminal\"",
            "activate",
            "repeat with w in windows",
            "repeat with t in tabs of w",
            "if tty of t is (item 1 of argv) then",
            "set selected of t to true",
            "set index of w to 1",
            "return",
            "end if",
            "end repeat",
            "end repeat",
            "end tell",
            "end run",
        ],
        &[&device],
    );
}

/// Runs an AppleScript with `osascript`, without waiting for it (the first
/// run asks the user to let AccRoster control Terminal).
pub(super) fn osascript(lines: &[&str], args: &[&str]) {
    let mut command = std::process::Command::new("/usr/bin/osascript");
    for line in lines {
        command.arg("-e").arg(line);
    }
    command.args(args);
    if let Ok(mut child) = command.spawn() {
        std::thread::spawn(move || child.wait());
    }
}

/// The Claude desktop app's bundle id.
pub(super) const DESKTOP_BUNDLE_ID: &str = "com.anthropic.claudefordesktop";

/// Opens Claude, or brings it forward when it runs:
/// `openApplicationAtURL` activates a running app in place (like `open -b`).
pub(super) fn bring_desktop_app_forward() {
    use objc2_app_kit::NSWorkspaceOpenConfiguration;
    let workspace = NSWorkspace::sharedWorkspace();
    let Some(url) =
        workspace.URLForApplicationWithBundleIdentifier(&NSString::from_str(DESKTOP_BUNDLE_ID))
    else {
        return;
    };
    let configuration = NSWorkspaceOpenConfiguration::configuration();
    configuration.setActivates(true);
    workspace.openApplicationAtURL_configuration_completionHandler(&url, &configuration, None);
}

/// Asks Claude to quit the way ⌘Q does (`terminate` sends it the quit Apple
/// event; Claude may still ask first), never force-quits it. Returns at
/// once: the core watches for Claude to exit and says so when it doesn't
/// (a refused request included). Only Reopen Claude sends this.
pub(super) fn quit_desktop_app() {
    use objc2_app_kit::NSRunningApplication;
    let running = NSRunningApplication::runningApplicationsWithBundleIdentifier(
        &NSString::from_str(DESKTOP_BUNDLE_ID),
    );
    for app in running.iter() {
        app.terminate();
    }
}

/// Makes AccRoster the active app, whichever app is. A menu bar app isn't
/// active when its menu is clicked, and macOS can refuse the cooperative
/// `activate()`, which leaves a new window or alert behind the active
/// app's windows.
pub(super) fn come_forward(app: &NSApplication) {
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    #[allow(deprecated)]
    NSRunningApplication::currentApplication().activateWithOptions(
        NSApplicationActivationOptions::ActivateAllWindows
            | NSApplicationActivationOptions::ActivateIgnoringOtherApps,
    );
}

/// Shows `window` in front of every app's windows and makes it key.
pub(super) fn show_in_front(window: &NSWindow) {
    come_forward(&NSApplication::sharedApplication(window.mtm()));
    window.makeKeyAndOrderFront(None);
    // In front even when macOS didn't activate the app.
    window.orderFrontRegardless();
}

/// Asks for a nickname; `None` on Cancel. An empty answer clears it.
pub(super) fn ask_nickname(
    mtm: MainThreadMarker,
    name: &str,
    current: Option<&str>,
) -> Option<String> {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(&format!("Nickname for {name}")));
    alert.setInformativeText(ns_string!("Leave it empty to use the full name."));
    alert.addButtonWithTitle(ns_string!("Save"));
    alert.addButtonWithTitle(ns_string!("Cancel"));
    let field = NSTextField::textFieldWithString(&NSString::from_str(current.unwrap_or("")), mtm);
    field.setFrame(NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(260.0, 24.0),
    ));
    alert.setAccessoryView(Some(&field));
    alert.window().setInitialFirstResponder(Some(&field));
    come_forward(&NSApplication::sharedApplication(mtm));
    (alert.runModal() == NSAlertFirstButtonReturn).then(|| field.stringValue().to_string())
}

/// Moves the running app's bundle to the Trash (the uninstall's last step).
/// Outside a bundle (`cargo run`) there is nothing to trash. If it fails,
/// says so: everything else is already removed.
pub(super) fn trash_app(mtm: MainThreadMarker) {
    let bundle = NSBundle::mainBundle();
    if !bundle.bundlePath().to_string().ends_with(".app") {
        return;
    }
    let url = bundle.bundleURL();
    if let Err(error) =
        NSFileManager::defaultManager().trashItemAtURL_resultingItemURL_error(&url, None)
    {
        let alert = NSAlert::new(mtm);
        alert.setMessageText(ns_string!("Move AccRoster to the Trash yourself"));
        alert.setInformativeText(&NSString::from_str(&format!(
            "Everything else is removed, but the app couldn't be moved to the Trash: {}",
            error.localizedDescription()
        )));
        come_forward(&NSApplication::sharedApplication(mtm));
        alert.runModal();
    }
}

/// Shows a launch failure before the app exits.
pub fn show_fatal_error(mtm: MainThreadMarker, message: &str) {
    let alert = objc2_app_kit::NSAlert::new(mtm);
    alert.setMessageText(ns_string!("AccRoster couldn't start"));
    alert.setInformativeText(&NSString::from_str(message));
    come_forward(&NSApplication::sharedApplication(mtm));
    alert.runModal();
}

/// Watches the Mac's network path (`NWPathMonitor`) and tells the core when
/// it is gained or lost, starting with the current state.
pub(super) struct NetworkMonitor {
    monitor: *mut c_void,
}

#[link(name = "Network", kind = "framework")]
unsafe extern "C" {
    fn nw_path_monitor_create() -> *mut c_void;
    fn nw_path_monitor_set_update_handler(
        monitor: *mut c_void,
        handler: &block2::Block<dyn Fn(*mut c_void)>,
    );
    fn nw_path_monitor_set_queue(monitor: *mut c_void, queue: &DispatchQueue);
    fn nw_path_monitor_start(monitor: *mut c_void);
    fn nw_path_monitor_cancel(monitor: *mut c_void);
    fn nw_path_get_status(path: *mut c_void) -> c_int;
    fn nw_release(object: *mut c_void);
}

/// `nw_path_status_satisfied`.
pub(super) const NW_PATH_STATUS_SATISFIED: c_int = 1;

impl NetworkMonitor {
    pub(super) fn start() -> NetworkMonitor {
        let last = std::cell::Cell::new(None::<bool>);
        let handler = RcBlock::new(move |path: *mut c_void| {
            // SAFETY: `path` is the nw_path_t the monitor passes in.
            let online = unsafe { nw_path_get_status(path) } == NW_PATH_STATUS_SATISFIED;
            if last.replace(Some(online)) == Some(online) {
                return;
            }
            UI.with(|cell| {
                if let Some(ui) = cell.get() {
                    ui.event(Event::NetworkChanged { online });
                }
            });
        });
        // SAFETY: the monitor is created, given its handler and the main
        // queue (the handler touches the UI's thread-local), then started;
        // it is cancelled and released on drop.
        unsafe {
            let monitor = nw_path_monitor_create();
            nw_path_monitor_set_update_handler(monitor, &handler);
            nw_path_monitor_set_queue(monitor, DispatchQueue::main());
            nw_path_monitor_start(monitor);
            NetworkMonitor { monitor }
        }
    }
}

impl Drop for NetworkMonitor {
    fn drop(&mut self) {
        // SAFETY: `monitor` came from nw_path_monitor_create and is used
        // nowhere else.
        unsafe {
            nw_path_monitor_cancel(self.monitor);
            nw_release(self.monitor);
        }
    }
}
