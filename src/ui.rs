//! The UI layer: AppKit through `objc2`. It renders the core's view models,
//! forwards clicks as commands and performs effects. It decides nothing.
//!
//! Threading: AppKit lives on the main thread. The core runs on the driver's
//! thread; each [`Update`] hops to the main queue and is applied there.
//!
//! This file holds the [`Ui`] object (the target of every click and the
//! owner of every window) and the windows it builds. Its leaves are apart:
//! [`widgets`] builds views from view models, [`sidebar`] is the All
//! Sessions sidebar's data source, and [`mac`] does what the core asks of
//! the rest of the Mac.

mod all_sessions_window;
mod diagnostics_window;
mod first_run_window;
mod mac;
mod settings_window;
mod sidebar;
mod widgets;

use all_sessions_window::AllSessionsWindow;
use diagnostics_window::DiagnosticsWindow;
use first_run_window::FirstRunWindow;
pub use mac::show_fatal_error;
use mac::{
    NetworkMonitor, ask_nickname, bring_desktop_app_forward, come_forward, quit_desktop_app,
    run_in_terminal, show_in_front, show_in_terminal, trash_app,
};
use settings_window::{SettingsWindow, render_settings};
use widgets::{
    ROW_WIDTH, RowView, disabled_item, discovered_row_content, key_equivalent, menu_bar_icon,
    menu_item, profile_row_content, rows_width, session_row_view, status_item,
};

use std::cell::{Cell, OnceCell, RefCell};

use accroster_core::driver::{self, CoreHandle, Update};
use accroster_core::watcher::{self, FolderWatch};

use crate::notifications;
use accroster_core::{
    Command, Core, Effect, Event, MenuBarView, MenuItem, ProfileSettings, SessionDetail,
    SessionRow, SidebarSyncMode, View,
};
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::NSAlertStyle;
use objc2_app_kit::{
    NSAboutPanelOptionApplicationName, NSAboutPanelOptionApplicationVersion,
    NSAboutPanelOptionCredits, NSAlert, NSAlertFirstButtonReturn, NSApplication, NSButton, NSColor,
    NSControlStateValueOff, NSControlStateValueOn, NSControlTextEditingDelegate, NSFont,
    NSFontAttributeName, NSForegroundColorAttributeName, NSMenu, NSMenuDelegate, NSMenuItem,
    NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSPasteboard, NSPasteboardTypeString,
    NSSearchField, NSStatusBar, NSStatusItem, NSTableColumn, NSTableView, NSTableViewDataSource,
    NSTableViewDelegate, NSTextAlignment, NSTextField, NSVariableStatusItemLength, NSView,
    NSWindowDelegate, NSWorkspace, NSWorkspaceDidWakeNotification,
    NSWorkspaceWillSleepNotification,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSAttributedStringKey, NSDictionary,
    NSDistributedNotificationCenter, NSInteger, NSNotification, NSObjectProtocol, NSPoint,
    NSString, NSURL, ns_string,
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
    notifications::start(|action| {
        UI.with(|cell| {
            if let Some(ui) = cell.get() {
                ui.event(Event::NotificationActionClicked(action));
            }
        });
    });
    ui.observe_the_mac();
}

pub struct Ivars {
    core: OnceCell<CoreHandle>,
    status_item: Retained<NSStatusItem>,
    /// Keeps the network path monitor alive.
    network: OnceCell<NetworkMonitor>,
    /// What the status item shows now: it is redrawn only when that changes.
    shown_menu_bar: RefCell<Option<MenuBarView>>,
    /// The commands of the menu's clickable items (submenus included), by tag.
    actions: RefCell<Vec<Command>>,
    settings: OnceCell<SettingsWindow>,
    diagnostics: OnceCell<DiagnosticsWindow>,
    /// The Diagnostics window's Resume buttons' commands, by tag.
    resume_commands: RefCell<Vec<Command>>,
    /// The first-run offer, open while the view has one.
    first_run: RefCell<Option<FirstRunWindow>>,
    /// Whether the status item's menu is open. While it is, its tag table is
    /// left alone: rows whose content changed are swapped in place, and a
    /// menu whose clickable items changed waits in `pending_menu`.
    menu_open: Cell<bool>,
    pending_menu: RefCell<Option<Vec<MenuItem>>>,
    /// The items the status item's menu shows now.
    shown_menu: RefCell<Vec<MenuItem>>,
    /// The width its custom rows were given.
    shown_width: Cell<f64>,
    /// Settings > Hidden (n)…: each entry's title and Restore command, by tag.
    hidden: RefCell<Vec<(String, Command)>>,
    /// Settings > Profiles as last drawn, and its buttons' commands by tag.
    profile_settings: RefCell<Option<Vec<ProfileSettings>>>,
    profile_commands: RefCell<Vec<Command>>,
    all_sessions: OnceCell<AllSessionsWindow>,
    /// The rows the All Sessions table shows (its data source).
    session_rows: RefCell<Vec<SessionRow>>,
    /// The detail pane as last drawn.
    session_detail: RefCell<Option<SessionDetail>>,
    /// The detail pane's Open buttons' commands, by tag.
    open_commands: RefCell<Vec<Command>>,
    /// Set while the UI changes a selection itself, so the selection
    /// callbacks don't echo it back to the core.
    rendering: Cell<bool>,
    /// The FSEvents watch the core asked for while All Sessions is open.
    watch: RefCell<Option<FolderWatch>>,
    /// The FSEvents watch on the sidebars while Sidebar sync is on.
    sidebar_watch: RefCell<Option<FolderWatch>>,
}

define_class!(
    /// Owns the AppKit objects and is the target of every click.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AccRosterUi"]
    #[ivars = Ivars]
    pub struct Ui;

    unsafe impl NSObjectProtocol for Ui {}

    unsafe impl NSControlTextEditingDelegate for Ui {}

    unsafe impl NSTableViewDataSource for Ui {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, _table: &NSTableView) -> NSInteger {
            self.ivars().session_rows.borrow().len() as NSInteger
        }
    }

    unsafe impl NSTableViewDelegate for Ui {
        /// One two-line Session row: a running dot, the title, then
        /// `project · last activity`.
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn view_for_row(
            &self,
            _table: &NSTableView,
            _column: Option<&NSTableColumn>,
            row: NSInteger,
        ) -> Option<Retained<NSView>> {
            let rows = self.ivars().session_rows.borrow();
            usize::try_from(row)
                .ok()
                .and_then(|row| rows.get(row))
                .map(|row| session_row_view(self.mtm(), row))
        }

        /// A row was selected (or none): the detail pane follows.
        #[unsafe(method(tableViewSelectionDidChange:))]
        fn selection_did_change(&self, notification: &NSNotification) {
            if self.ivars().rendering.get() {
                return;
            }
            let Some(table) = notification
                .object()
                .and_then(|object| object.downcast::<NSTableView>().ok())
            else {
                return;
            };
            let id = usize::try_from(table.selectedRow())
                .ok()
                .and_then(|row| self.ivars().session_rows.borrow().get(row).map(|r| r.id.clone()));
            self.send(Command::SelectSession(id));
        }
    }

    unsafe impl NSWindowDelegate for Ui {
        /// Only the All Sessions window has this delegate: closing it stops
        /// the watch.
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            self.send(Command::CloseAllSessions);
        }
    }

    unsafe impl NSMenuDelegate for Ui {
        /// The status item's menu is opening: the core rescans what the menu
        /// shows (Discovered Profiles) and refreshes old Limit readings in
        /// the background.
        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, _menu: &NSMenu) {
            self.ivars().menu_open.set(true);
            self.event(Event::MenuOpened);
        }

        /// Applies the menu that arrived while it was open, after the clicked
        /// item's action (sent once tracking ends) has read the old tags.
        #[unsafe(method(menuDidClose:))]
        fn menu_did_close(&self, _menu: &NSMenu) {
            self.ivars().menu_open.set(false);
            DispatchQueue::main().exec_async(|| {
                UI.with(|cell| {
                    if let Some(ui) = cell.get() {
                        ui.apply_pending_menu();
                    }
                });
            });
        }
    }

    impl Ui {
        #[unsafe(method(willSleep:))]
        fn will_sleep(&self, _notification: &NSNotification) {
            self.event(Event::WillSleep);
        }

        #[unsafe(method(didWake:))]
        fn did_wake(&self, _notification: &NSNotification) {
            self.event(Event::DidWake);
        }

        #[unsafe(method(screenLocked:))]
        fn screen_locked(&self, _notification: &NSNotification) {
            self.event(Event::ScreenLocked);
        }

        #[unsafe(method(screenUnlocked:))]
        fn screen_unlocked(&self, _notification: &NSNotification) {
            self.event(Event::ScreenUnlocked);
        }

        /// A menu item was clicked; its tag indexes the menu's commands.
        #[unsafe(method(menuItemClicked:))]
        fn menu_item_clicked(&self, sender: &NSMenuItem) {
            let command = usize::try_from(sender.tag())
                .ok()
                .and_then(|tag| self.ivars().actions.borrow().get(tag).cloned());
            if let Some(command) = command {
                self.send(command);
            }
        }

        #[unsafe(method(verboseLoggingClicked:))]
        fn verbose_logging_clicked(&self, sender: &NSButton) {
            self.send(Command::SetVerboseLogging(sender.state() == NSControlStateValueOn));
        }

        #[unsafe(method(launchAtLoginClicked:))]
        fn launch_at_login_clicked(&self, sender: &NSButton) {
            self.send(Command::SetLaunchAtLogin(sender.state() == NSControlStateValueOn));
        }

        /// Done in the first-run window: answers with the checkbox's state.
        #[unsafe(method(firstRunDoneClicked:))]
        fn first_run_done_clicked(&self, _sender: &NSButton) {
            let on = self
                .ivars()
                .first_run
                .borrow()
                .as_ref()
                .map(|first_run| first_run.launch_at_login.state() == NSControlStateValueOn);
            if let Some(on) = on {
                self.send(Command::SetLaunchAtLogin(on));
            }
        }

        /// The Sidebar sync checkbox: on is Manual ("Sync now").
        #[unsafe(method(sidebarSyncClicked:))]
        fn sidebar_sync_clicked(&self, sender: &NSButton) {
            let mode = if sender.state() == NSControlStateValueOn {
                SidebarSyncMode::Manual
            } else {
                SidebarSyncMode::Off
            };
            self.send(Command::SetSidebarSync(mode));
        }

        /// The "automatically" checkbox under it: on is Automatic, off is
        /// back to Manual.
        #[unsafe(method(sidebarSyncAutomaticClicked:))]
        fn sidebar_sync_automatic_clicked(&self, sender: &NSButton) {
            let mode = if sender.state() == NSControlStateValueOn {
                SidebarSyncMode::Automatic
            } else {
                SidebarSyncMode::Manual
            };
            self.send(Command::SetSidebarSync(mode));
        }

        #[unsafe(method(uninstallClicked:))]
        fn uninstall_clicked(&self, _sender: &NSButton) {
            self.send(Command::AskUninstall);
        }

        /// Retry after a stopped uninstall (already confirmed once).
        #[unsafe(method(retryUninstallClicked:))]
        fn retry_uninstall_clicked(&self, _sender: &NSButton) {
            self.send(Command::Uninstall);
        }

        /// "Include SSH and No-folder Sessions".
        #[unsafe(method(syncSshSessionsClicked:))]
        fn sync_ssh_sessions_clicked(&self, sender: &NSButton) {
            self.send(Command::SetIncludeSshAndNoFolderSessions(
                sender.state() == NSControlStateValueOn,
            ));
        }

        #[unsafe(method(syncNowClicked:))]
        fn sync_now_clicked(&self, _sender: &NSButton) {
            self.send(Command::SyncSidebarNow);
        }

        #[unsafe(method(reopenClaudeClicked:))]
        fn reopen_claude_clicked(&self, _sender: &NSButton) {
            self.send(Command::ReopenDesktopApp);
        }

        /// The thresholds field was committed: whole percents separated by
        /// commas or spaces ("80, 95").
        #[unsafe(method(thresholdsChanged:))]
        fn thresholds_changed(&self, sender: &NSTextField) {
            let text = sender.stringValue().to_string();
            let values = text
                .split(|c: char| !c.is_ascii_digit())
                .filter_map(|part| part.parse().ok())
                .collect();
            self.send(Command::SetThresholds(values));
        }

        #[unsafe(method(resetNotificationClicked:))]
        fn reset_notification_clicked(&self, sender: &NSButton) {
            self.send(Command::SetResetNotification(
                sender.state() == NSControlStateValueOn,
            ));
        }

        #[unsafe(method(cliSwitchingClicked:))]
        fn cli_switching_clicked(&self, sender: &NSButton) {
            self.send(Command::SetCliSwitching(sender.state() == NSControlStateValueOn));
        }

        /// Settings > Hidden (n)…: pops up one Restore item per hidden
        /// Discovered Profile.
        #[unsafe(method(hiddenClicked:))]
        fn hidden_clicked(&self, sender: &NSButton) {
            let mtm = self.mtm();
            let menu = NSMenu::new(mtm);
            menu.setAutoenablesItems(false);
            let titles: Vec<String> =
                self.ivars().hidden.borrow().iter().map(|(title, _)| title.clone()).collect();
            for (tag, title) in titles.iter().enumerate() {
                let item = menu_item(mtm, &format!("Restore {title}"));
                unsafe {
                    item.setAction(Some(sel!(restoreHiddenClicked:)));
                    item.setTarget(Some(self));
                }
                item.setTag(tag as isize);
                menu.addItem(&item);
            }
            let origin = NSPoint::new(0.0, sender.bounds().size.height + 4.0);
            menu.popUpMenuPositioningItem_atLocation_inView(None, origin, Some(sender));
        }

        /// A Settings > Profiles button (↑, ↓, Rename…, Mute).
        #[unsafe(method(profileSettingClicked:))]
        fn profile_setting_clicked(&self, sender: &NSButton) {
            let command = usize::try_from(sender.tag())
                .ok()
                .and_then(|tag| self.ivars().profile_commands.borrow().get(tag).cloned());
            if let Some(command) = command {
                self.send(command);
            }
        }

        #[unsafe(method(restoreHiddenClicked:))]
        fn restore_hidden_clicked(&self, sender: &NSMenuItem) {
            let command = usize::try_from(sender.tag())
                .ok()
                .and_then(|tag| self.ivars().hidden.borrow().get(tag).map(|(_, c)| c.clone()));
            if let Some(command) = command {
                self.send(command);
            }
        }

        /// The All Sessions search field changed.
        #[unsafe(method(searchChanged:))]
        fn search_changed(&self, sender: &NSSearchField) {
            self.send(Command::SearchSessions(sender.stringValue().to_string()));
        }

        #[unsafe(method(revealLogsClicked:))]
        fn reveal_logs_clicked(&self, _sender: &NSButton) {
            self.send(Command::RevealLogs);
        }

        #[unsafe(method(copyReportClicked:))]
        fn copy_report_clicked(&self, _sender: &NSButton) {
            self.send(Command::CopyDiagnosticsReport);
        }

        /// An Open button in the All Sessions detail pane.
        #[unsafe(method(openChoiceClicked:))]
        fn open_choice_clicked(&self, sender: &NSButton) {
            let command = usize::try_from(sender.tag())
                .ok()
                .and_then(|tag| self.ivars().open_commands.borrow().get(tag).cloned());
            if let Some(command) = command {
                self.send(command);
            }
        }

        /// A double-click on an All Sessions row: open where it was last
        /// used.
        #[unsafe(method(sessionDoubleClicked:))]
        fn session_double_clicked(&self, sender: &NSTableView) {
            let id = usize::try_from(sender.clickedRow())
                .ok()
                .and_then(|row| self.ivars().session_rows.borrow().get(row).map(|r| r.id.clone()));
            if let Some(id) = id {
                self.send(Command::OpenSession(id));
            }
        }

        #[unsafe(method(resumeClicked:))]
        fn resume_clicked(&self, sender: &NSButton) {
            let command = usize::try_from(sender.tag())
                .ok()
                .and_then(|tag| self.ivars().resume_commands.borrow().get(tag).cloned());
            if let Some(command) = command {
                self.send(command);
            }
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
            network: OnceCell::new(),
            shown_menu_bar: RefCell::new(None),
            actions: RefCell::new(Vec::new()),
            settings: OnceCell::new(),
            diagnostics: OnceCell::new(),
            resume_commands: RefCell::new(Vec::new()),
            first_run: RefCell::new(None),
            menu_open: Cell::new(false),
            pending_menu: RefCell::new(None),
            shown_menu: RefCell::new(Vec::new()),
            shown_width: Cell::new(ROW_WIDTH),
            hidden: RefCell::new(Vec::new()),
            profile_settings: RefCell::new(None),
            profile_commands: RefCell::new(Vec::new()),
            all_sessions: OnceCell::new(),
            session_rows: RefCell::new(Vec::new()),
            session_detail: RefCell::new(None),
            open_commands: RefCell::new(Vec::new()),
            rendering: Cell::new(false),
            watch: RefCell::new(None),
            sidebar_watch: RefCell::new(None),
        });
        unsafe { msg_send![super(this), init] }
    }

    /// Forwards sleep and wake, screen lock and unlock, and network path
    /// changes to the core.
    fn observe_the_mac(&self) {
        let workspace = NSWorkspace::sharedWorkspace().notificationCenter();
        let distributed = NSDistributedNotificationCenter::defaultCenter();
        // SAFETY: each selector names a method of `Ui` taking one
        // NSNotification; the observer lives as long as the app.
        unsafe {
            workspace.addObserver_selector_name_object(
                self,
                sel!(willSleep:),
                Some(NSWorkspaceWillSleepNotification),
                None,
            );
            workspace.addObserver_selector_name_object(
                self,
                sel!(didWake:),
                Some(NSWorkspaceDidWakeNotification),
                None,
            );
            distributed.addObserver_selector_name_object(
                self,
                sel!(screenLocked:),
                Some(ns_string!("com.apple.screenIsLocked")),
                None,
            );
            distributed.addObserver_selector_name_object(
                self,
                sel!(screenUnlocked:),
                Some(ns_string!("com.apple.screenIsUnlocked")),
                None,
            );
        }
        let _ = self.ivars().network.set(NetworkMonitor::start());
    }

    fn event(&self, event: Event) {
        if let Some(core) = self.ivars().core.get() {
            core.event(event);
        }
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
        for effect in update.effects {
            self.perform(effect, &update.view);
        }
    }

    fn render(&self, view: &View) {
        let mtm = self.mtm();
        let item = &self.ivars().status_item;
        let mut shown_menu_bar = self.ivars().shown_menu_bar.borrow_mut();
        if shown_menu_bar.as_ref() != Some(&view.menu_bar)
            && let Some(button) = item.button(mtm)
        {
            match &view.menu_bar.label {
                Some(label) => {
                    button.setImage(None);
                    button.setTitle(&NSString::from_str(label));
                }
                None => {
                    button.setTitle(ns_string!(""));
                    button.setImage(Some(&menu_bar_icon()));
                }
            }
            button.setAppearsDisabled(view.menu_bar.dimmed);
            *shown_menu_bar = Some(view.menu_bar.clone());
        }
        drop(shown_menu_bar);
        // Building the menu's rows takes tens of milliseconds on the main
        // thread, and most updates (ticks) leave the menu as it is.
        if self.ivars().menu_open.get() {
            let pending = if self.update_open_menu(&view.menu.items) {
                None
            } else {
                Some(view.menu.items.clone())
            };
            *self.ivars().pending_menu.borrow_mut() = pending;
        } else {
            // A menu left waiting from while it was open is older than this.
            self.ivars().pending_menu.borrow_mut().take();
            if *self.ivars().shown_menu.borrow() != view.menu.items
                || self.ivars().status_item.menu(mtm).is_none()
            {
                self.set_menu(&view.menu.items);
            }
        }
        *self.ivars().hidden.borrow_mut() = view
            .settings
            .hidden_discovered
            .iter()
            .map(|hidden| (hidden.title.clone(), hidden.restore.clone()))
            .collect();
        if let Some(settings) = self.ivars().settings.get() {
            render_settings(settings, &view.settings);
            self.render_profile_settings(settings, &view.settings.profiles);
        }
        if let Some(diagnostics) = self.ivars().diagnostics.get() {
            self.render_diagnostics(diagnostics, &view.diagnostics);
        }
        if let Some(window) = self.ivars().all_sessions.get() {
            self.render_all_sessions(window, &view.all_sessions);
        }
        self.render_first_run(view);
    }

    /// Replaces the status item's menu and its tag table.
    fn set_menu(&self, items: &[MenuItem]) {
        let mut actions = Vec::new();
        let menu = self.build_menu(items, &mut actions);
        menu.setDelegate(Some(ProtocolObject::from_ref(self)));
        self.ivars().status_item.setMenu(Some(&menu));
        *self.ivars().actions.borrow_mut() = actions;
        *self.ivars().shown_menu.borrow_mut() = items.to_vec();
        self.ivars().shown_width.set(rows_width(self.mtm(), items));
    }

    /// Updates the open menu in place (a refreshed Limit reading, the
    /// offline line) when its clickable items, and so its tags, are
    /// unchanged: each changed item is swapped for a fresh one. Returns
    /// `false` when the change has to wait until the menu closes.
    fn update_open_menu(&self, items: &[MenuItem]) -> bool {
        let Some(menu) = self.ivars().status_item.menu(self.mtm()) else {
            return false;
        };
        let shown = self.ivars().shown_menu.borrow().clone();
        if shown == items {
            return true;
        }
        if shown.len() != items.len() || menu.numberOfItems() as usize != items.len() {
            return false;
        }
        // An open menu keeps its width: a wider row would be clipped.
        if rows_width(self.mtm(), items) > self.ivars().shown_width.get() {
            return false;
        }
        let mut actions = Vec::new();
        let fresh = self.build_menu(items, &mut actions);
        if actions != *self.ivars().actions.borrow() {
            return false;
        }
        let fresh_items = fresh.itemArray();
        fresh.removeAllItems();
        for (index, (old, new)) in shown.iter().zip(items).enumerate() {
            if old != new {
                menu.removeItemAtIndex(index as isize);
                menu.insertItem_atIndex(&fresh_items.objectAtIndex(index), index as isize);
            }
        }
        *self.ivars().shown_menu.borrow_mut() = items.to_vec();
        true
    }

    fn apply_pending_menu(&self) {
        if self.ivars().menu_open.get() {
            return;
        }
        let pending = self.ivars().pending_menu.borrow_mut().take();
        if let Some(items) = pending {
            self.set_menu(&items);
        }
    }

    /// Builds a submenu from `items`.
    fn build_menu(&self, items: &[MenuItem], actions: &mut Vec<Command>) -> Retained<NSMenu> {
        let menu = NSMenu::new(self.mtm());
        menu.setAutoenablesItems(false);
        self.fill_menu(&menu, items, actions);
        menu
    }

    /// Adds `items` to `menu`, recording each clickable item's command in
    /// `actions` under the tag it gets.
    fn fill_menu(&self, menu: &NSMenu, items: &[MenuItem], actions: &mut Vec<Command>) {
        let mtm = self.mtm();
        let contents: Vec<Option<Retained<NSView>>> = items
            .iter()
            .map(|item| match item {
                MenuItem::Profile(profile) => Some(profile_row_content(mtm, profile)),
                MenuItem::Discovered(discovered) => Some(discovered_row_content(mtm, discovered)),
                _ => None,
            })
            .collect();
        let width = rows_width(mtm, items);
        for (item, content) in items.iter().zip(&contents) {
            let row = match item {
                MenuItem::Separator => NSMenuItem::separatorItem(mtm),
                MenuItem::Warning(text) => disabled_item(mtm, &format!("⚠︎ {text}")),
                MenuItem::Status { text, tone } => status_item(mtm, text, *tone),
                MenuItem::Detail(text) => disabled_item(mtm, text),
                MenuItem::Submenu { title, items } => {
                    let row = menu_item(mtm, title);
                    row.setSubmenu(Some(&self.build_menu(items, actions)));
                    row
                }
                MenuItem::Action { title, command } => {
                    let row = menu_item(mtm, title);
                    if let Some(key) = key_equivalent(command) {
                        row.setKeyEquivalent(&NSString::from_str(key));
                    }
                    unsafe {
                        row.setAction(Some(sel!(menuItemClicked:)));
                        row.setTarget(Some(self));
                    }
                    row.setTag(actions.len() as isize);
                    actions.push(command.clone());
                    row
                }
                MenuItem::Check {
                    title,
                    checked,
                    command,
                } => {
                    let row = menu_item(mtm, title);
                    unsafe {
                        row.setAction(Some(sel!(menuItemClicked:)));
                        row.setTarget(Some(self));
                    }
                    row.setState(if *checked {
                        NSControlStateValueOn
                    } else {
                        NSControlStateValueOff
                    });
                    row.setTag(actions.len() as isize);
                    actions.push(command.clone());
                    row
                }
                MenuItem::Profile(profile) => {
                    let row = menu_item(mtm, &profile.title);
                    if let Some(content) = content {
                        row.setView(Some(&RowView::wrap(mtm, content, width)));
                    }
                    // The row sends this itself on click (see RowView).
                    if let Some(command) = &profile.command {
                        unsafe {
                            row.setAction(Some(sel!(menuItemClicked:)));
                            row.setTarget(Some(self));
                        }
                        row.setTag(actions.len() as isize);
                        actions.push(command.clone());
                    }
                    if !profile.submenu.is_empty() {
                        row.setSubmenu(Some(&self.build_menu(&profile.submenu, actions)));
                    }
                    row
                }
                MenuItem::Discovered(discovered) => {
                    let row = menu_item(mtm, &discovered.title);
                    if let Some(content) = content {
                        row.setView(Some(&RowView::wrap(mtm, content, width)));
                    }
                    // The row sends this itself on click (see RowView).
                    unsafe {
                        row.setAction(Some(sel!(menuItemClicked:)));
                        row.setTarget(Some(self));
                    }
                    row.setTag(actions.len() as isize);
                    actions.push(discovered.command.clone());
                    if !discovered.submenu.is_empty() {
                        row.setSubmenu(Some(&self.build_menu(&discovered.submenu, actions)));
                    }
                    row
                }
            };
            menu.addItem(&row);
        }
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
                self.render_profile_settings(settings, &view.settings.profiles);
                settings.window.center();
                show_in_front(&settings.window);
            }
            Effect::ShowDiagnostics => {
                let diagnostics = self
                    .ivars()
                    .diagnostics
                    .get_or_init(|| self.make_diagnostics_window());
                self.render_diagnostics(diagnostics, &view.diagnostics);
                diagnostics.window.center();
                show_in_front(&diagnostics.window);
            }
            Effect::ShowAbout => {
                let about = &view.about;
                let centred = NSMutableParagraphStyle::new();
                centred.setAlignment(NSTextAlignment::Center);
                let font = NSFont::systemFontOfSize(NSFont::smallSystemFontSize());
                let color = NSColor::labelColor();
                let attributes = unsafe {
                    NSDictionary::<NSAttributedStringKey, AnyObject>::from_slices(
                        &[
                            NSParagraphStyleAttributeName,
                            NSFontAttributeName,
                            NSForegroundColorAttributeName,
                        ],
                        &[&*centred, &*font, &*color],
                    )
                };
                let credits = unsafe {
                    NSAttributedString::initWithString_attributes(
                        NSAttributedString::alloc(),
                        &NSString::from_str(&format!("{}\n\n{}", about.tagline, about.disclaimer)),
                        Some(&attributes),
                    )
                };
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
                come_forward(&app);
                unsafe { app.orderFrontStandardAboutPanelWithOptions(&options) };
            }
            Effect::RevealInFinder(path) => {
                let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
                NSWorkspace::sharedWorkspace()
                    .activateFileViewerSelectingURLs(&NSArray::from_slice(&[&*url]));
            }
            Effect::OpenUrl(url) => {
                if let Some(url) = NSURL::URLWithString(&NSString::from_str(&url)) {
                    NSWorkspace::sharedWorkspace().openURL(&url);
                }
            }
            Effect::ShowAllSessions => {
                let window = self
                    .ivars()
                    .all_sessions
                    .get_or_init(|| self.make_all_sessions_window());
                self.render_all_sessions(window, &view.all_sessions);
                if !window.window.isVisible() {
                    window.window.center();
                }
                show_in_front(&window.window);
            }
            Effect::WatchSessionFiles(folders) => {
                let Some(core) = self.ivars().core.get().cloned() else {
                    return;
                };
                // A folder that can't be watched is left to the menu-open
                // rescan; the UI has no log of its own.
                let watch = FolderWatch::start(&folders, watcher::DEBOUNCE, move |paths| {
                    core.event(Event::SessionFilesChanged(paths))
                })
                .ok()
                .map(|(watch, _skipped)| watch);
                *self.ivars().watch.borrow_mut() = watch;
            }
            Effect::StopWatchingSessionFiles => {
                self.ivars().watch.borrow_mut().take();
            }
            Effect::WatchSidebarFolders(folders) => {
                let Some(core) = self.ivars().core.get().cloned() else {
                    return;
                };
                // A folder that can't be watched leaves Automatic mode to
                // "Sync now"; the UI has no log of its own.
                let watch = FolderWatch::start(&folders, watcher::DEBOUNCE, move |paths| {
                    core.event(Event::SidebarFilesChanged(paths))
                })
                .ok()
                .map(|(watch, _skipped)| watch);
                *self.ivars().sidebar_watch.borrow_mut() = watch;
            }
            Effect::StopWatchingSidebarFolders => {
                self.ivars().sidebar_watch.borrow_mut().take();
            }
            Effect::OpenLoginItemsSettings => unsafe {
                objc2_service_management::SMAppService::openSystemSettingsLoginItems()
            },
            Effect::CopyToClipboard(text) => {
                let pasteboard = NSPasteboard::generalPasteboard();
                pasteboard.clearContents();
                pasteboard.setString_forType(&NSString::from_str(&text), unsafe {
                    NSPasteboardTypeString
                });
            }
            Effect::BringDesktopAppForward => bring_desktop_app_forward(),
            Effect::QuitDesktopApp => quit_desktop_app(),
            Effect::RunInTerminal(line) => run_in_terminal(&line),
            Effect::ShowInTerminal { tty } => show_in_terminal(tty.as_deref()),
            Effect::Notify { title, body } => notifications::post(&title, &body, None),
            Effect::NotifyWithAction {
                title,
                body,
                action,
                on_click,
            } => notifications::post(&title, &body, Some((&action, &on_click))),
            Effect::Terminate => app.terminate(None),
            Effect::ConfirmUninstall {
                title,
                message,
                confirm,
            } => {
                let alert = NSAlert::new(mtm);
                alert.setAlertStyle(NSAlertStyle::Warning);
                alert.setMessageText(&NSString::from_str(&title));
                alert.setInformativeText(&NSString::from_str(&message));
                alert
                    .addButtonWithTitle(ns_string!("Uninstall"))
                    .setHasDestructiveAction(true);
                alert.addButtonWithTitle(ns_string!("Cancel"));
                come_forward(&app);
                if alert.runModal() == NSAlertFirstButtonReturn {
                    self.send(confirm);
                }
            }
            Effect::TrashAppAndQuit => {
                trash_app(mtm);
                app.terminate(None);
            }
            Effect::AskNickname {
                name,
                current,
                target,
            } => {
                if let Some(nickname) = ask_nickname(mtm, &name, current.as_deref()) {
                    self.send(Command::SetNickname {
                        target,
                        nickname: Some(nickname),
                    });
                }
            }
            Effect::ConfirmRemoval {
                title,
                message,
                confirm,
            } => {
                let alert = NSAlert::new(mtm);
                alert.setAlertStyle(NSAlertStyle::Warning);
                alert.setMessageText(&NSString::from_str(&title));
                alert.setInformativeText(&NSString::from_str(&message));
                alert
                    .addButtonWithTitle(ns_string!("Remove"))
                    .setHasDestructiveAction(true);
                alert.addButtonWithTitle(ns_string!("Cancel"));
                come_forward(&app);
                if alert.runModal() == NSAlertFirstButtonReturn {
                    self.send(confirm);
                }
            }
        }
    }
}
