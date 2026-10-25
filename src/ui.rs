//! The UI layer: AppKit through `objc2`. It renders the core's view models,
//! forwards clicks as commands and performs effects. It decides nothing.
//!
//! Threading: AppKit lives on the main thread. The core runs on the driver's
//! thread; each [`Update`] hops to the main queue and is applied there.

use std::cell::{OnceCell, RefCell};

use accroster_core::driver::{self, CoreHandle, Update};
use accroster_core::{Command, Core, Effect, MenuItem, SettingsView, View};
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAboutPanelOptionApplicationName, NSAboutPanelOptionApplicationVersion,
    NSAboutPanelOptionCredits, NSApplication, NSBackingStoreType, NSButton, NSControlStateValueOff,
    NSControlStateValueOn, NSFont, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSTextField,
    NSVariableStatusItemLength, NSWindow, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSDictionary, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    NSURL, ns_string,
};

thread_local! {
    /// The one [`Ui`], reachable from main-queue blocks.
    static UI: OnceCell<Retained<Ui>> = const { OnceCell::new() };
}

/// Starts the UI: the status item, then the core on its driver thread.
pub fn start(core: Core, mtm: MainThreadMarker) {
    let ui = Ui::new(mtm);
    UI.with(|cell| {
        cell.set(ui.clone()).ok().expect("the UI starts once");
    });
    let handle = driver::start(core, |update| {
        DispatchQueue::main().exec_async(move || {
            UI.with(|cell| {
                if let Some(ui) = cell.get() {
                    ui.apply(update);
                }
            });
        });
    });
    ui.ivars().core.set(handle).expect("the core starts once");
}

pub struct Ivars {
    core: OnceCell<CoreHandle>,
    status_item: Retained<NSStatusItem>,
    /// The latest view, to map clicks back to the commands it attached.
    view: RefCell<Option<View>>,
    settings: OnceCell<SettingsWindow>,
}

struct SettingsWindow {
    window: Retained<NSWindow>,
    verbose_logging: Retained<NSButton>,
    verbose_logging_note: Retained<NSTextField>,
}

define_class!(
    /// Owns the AppKit objects and is the target of every click.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AccRosterUi"]
    #[ivars = Ivars]
    pub struct Ui;

    unsafe impl NSObjectProtocol for Ui {}

    impl Ui {
        /// A menu item was clicked; its tag is its index in the view's menu.
        #[unsafe(method(menuItemClicked:))]
        fn menu_item_clicked(&self, sender: &NSMenuItem) {
            let command = self.ivars().view.borrow().as_ref().and_then(|view| {
                match view.menu.items.get(usize::try_from(sender.tag()).ok()?)? {
                    MenuItem::Action { command, .. } => Some(command.clone()),
                    _ => None,
                }
            });
            if let Some(command) = command {
                self.send(command);
            }
        }

        #[unsafe(method(verboseLoggingClicked:))]
        fn verbose_logging_clicked(&self, sender: &NSButton) {
            self.send(Command::SetVerboseLogging(sender.state() == NSControlStateValueOn));
        }

        #[unsafe(method(revealLogsClicked:))]
        fn reveal_logs_clicked(&self, _sender: &NSButton) {
            self.send(Command::RevealLogs);
        }
    }
);

impl Ui {
    fn new(mtm: MainThreadMarker) -> Retained<Ui> {
        let status_item =
            NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        let this = Ui::alloc(mtm).set_ivars(Ivars {
            core: OnceCell::new(),
            status_item,
            view: RefCell::new(None),
            settings: OnceCell::new(),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn mtm(&self) -> MainThreadMarker {
        MainThreadMarker::from(self)
    }

    fn send(&self, command: Command) {
        if let Some(core) = self.ivars().core.get() {
            core.command(command);
        }
    }

    /// Renders the update's view, then performs its effects.
    fn apply(&self, update: Update) {
        self.render(&update.view);
        *self.ivars().view.borrow_mut() = Some(update.view.clone());
        for effect in update.effects {
            self.perform(effect, &update.view);
        }
    }

    fn render(&self, view: &View) {
        let mtm = self.mtm();
        let item = &self.ivars().status_item;
        if let Some(button) = item.button(mtm) {
            button.setTitle(&NSString::from_str(&view.menu_bar.label));
        }
        item.setMenu(Some(&self.build_menu(view)));
        if let Some(settings) = self.ivars().settings.get() {
            render_settings(settings, &view.settings);
        }
    }

    fn build_menu(&self, view: &View) -> Retained<NSMenu> {
        let mtm = self.mtm();
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        for (index, item) in view.menu.items.iter().enumerate() {
            let row = match item {
                MenuItem::Separator => NSMenuItem::separatorItem(mtm),
                MenuItem::Warning(text) => {
                    let row = menu_item(mtm, &format!("⚠︎ {text}"));
                    row.setEnabled(false);
                    row
                }
                MenuItem::Action { title, .. } => {
                    let row = menu_item(mtm, title);
                    unsafe {
                        row.setAction(Some(sel!(menuItemClicked:)));
                        row.setTarget(Some(self));
                    }
                    row.setTag(index as isize);
                    row
                }
            };
            menu.addItem(&row);
        }
        menu
    }

    fn perform(&self, effect: Effect, view: &View) {
        let mtm = self.mtm();
        let app = NSApplication::sharedApplication(mtm);
        match effect {
            Effect::ShowSettings => {
                let settings = self
                    .ivars()
                    .settings
                    .get_or_init(|| self.make_settings_window());
                render_settings(settings, &view.settings);
                settings.window.center();
                settings.window.makeKeyAndOrderFront(None);
                app.activate();
            }
            Effect::ShowAbout => {
                let about = &view.about;
                let credits = NSAttributedString::from_nsstring(&NSString::from_str(&format!(
                    "{}\n\n{}",
                    about.tagline, about.disclaimer
                )));
                let name = NSString::from_str(&about.name);
                let version = NSString::from_str(&about.version);
                let options = unsafe {
                    NSDictionary::<NSString, AnyObject>::from_slices(
                        &[
                            NSAboutPanelOptionApplicationName,
                            NSAboutPanelOptionApplicationVersion,
                            NSAboutPanelOptionCredits,
                        ],
                        &[&*name, &*version, &*credits],
                    )
                };
                app.activate();
                unsafe { app.orderFrontStandardAboutPanelWithOptions(&options) };
            }
            Effect::RevealInFinder(path) => {
                let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
                NSWorkspace::sharedWorkspace()
                    .activateFileViewerSelectingURLs(&NSArray::from_slice(&[&*url]));
            }
            Effect::Terminate => app.terminate(None),
        }
    }

    fn make_settings_window(&self) -> SettingsWindow {
        let mtm = self.mtm();
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(380.0, 140.0));
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // Keep the window when it is closed, so it can be shown again.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(ns_string!("AccRoster Settings"));
        let content = window.contentView().expect("a window has a content view");

        let verbose_logging = unsafe {
            NSButton::checkboxWithTitle_target_action(
                ns_string!("Verbose logging"),
                Some(self),
                Some(sel!(verboseLoggingClicked:)),
                mtm,
            )
        };
        verbose_logging.setFrameOrigin(NSPoint::new(20.0, 98.0));
        content.addSubview(&verbose_logging);

        let verbose_logging_note = NSTextField::labelWithString(ns_string!(""), mtm);
        verbose_logging_note.setFont(Some(&NSFont::systemFontOfSize(
            NSFont::smallSystemFontSize(),
        )));
        verbose_logging_note.setFrame(NSRect::new(
            NSPoint::new(40.0, 76.0),
            NSSize::new(320.0, 18.0),
        ));
        content.addSubview(&verbose_logging_note);

        let reveal = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Reveal logs in Finder"),
                Some(self),
                Some(sel!(revealLogsClicked:)),
                mtm,
            )
        };
        reveal.setFrameOrigin(NSPoint::new(14.0, 24.0));
        content.addSubview(&reveal);

        SettingsWindow {
            window,
            verbose_logging,
            verbose_logging_note,
        }
    }
}

fn menu_item(mtm: MainThreadMarker, title: &str) -> Retained<NSMenuItem> {
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            None,
            ns_string!(""),
        )
    }
}

fn render_settings(settings: &SettingsWindow, view: &SettingsView) {
    settings.verbose_logging.setState(if view.verbose_logging {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    settings
        .verbose_logging_note
        .setStringValue(&NSString::from_str(&view.verbose_logging_note));
}

/// Shows a launch failure before the app exits.
pub fn show_fatal_error(mtm: MainThreadMarker, message: &str) {
    let alert = objc2_app_kit::NSAlert::new(mtm);
    alert.setMessageText(ns_string!("AccRoster couldn't start"));
    alert.setInformativeText(&NSString::from_str(message));
    NSApplication::sharedApplication(mtm).activate();
    alert.runModal();
}
