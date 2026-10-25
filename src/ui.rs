//! The UI layer: AppKit through `objc2`. It renders the core's view models,
//! forwards clicks as commands and performs effects. It decides nothing.
//!
//! Threading: AppKit lives on the main thread. The core runs on the driver's
//! thread; each [`Update`] hops to the main queue and is applied there.

use std::cell::{Cell, OnceCell, RefCell};
use std::ffi::{c_int, c_void};

use accroster_core::driver::{self, CoreHandle, Update};
use accroster_core::watcher::{self, FolderWatch};

use crate::notifications;
use accroster_core::{
    AllSessionsView, CheckStatus, Command, Core, DiagnosticsView, DiscoveredRow, Effect, Event,
    LimitBar, MenuItem, ProfileRow, RunningDot, SessionDetail, SessionFilter, SessionRow,
    SettingsView, SidebarGroup, SidebarSyncMode, View,
};
use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::NSAlertStyle;
use objc2_app_kit::{
    NSAboutPanelOptionApplicationName, NSAboutPanelOptionApplicationVersion,
    NSAboutPanelOptionCredits, NSAlert, NSAlertFirstButtonReturn, NSApplication,
    NSAutoresizingMaskOptions, NSBackingStoreType, NSBezierPath, NSButton, NSColor, NSControlSize,
    NSControlStateValueOff, NSControlStateValueOn, NSControlTextEditingDelegate, NSEvent, NSFont,
    NSFontWeightRegular, NSLayoutAttribute, NSLayoutConstraintOrientation, NSLevelIndicator,
    NSLevelIndicatorStyle, NSLineBreakMode, NSMenu, NSMenuDelegate, NSMenuItem, NSOutlineView,
    NSOutlineViewDataSource, NSOutlineViewDelegate, NSPasteboard, NSPasteboardTypeString,
    NSResponder, NSScrollView, NSSearchField, NSStackView, NSStatusBar, NSStatusItem,
    NSTableColumn, NSTableView, NSTableViewDataSource, NSTableViewDelegate, NSTableViewStyle,
    NSTextField, NSUserInterfaceLayoutOrientation, NSVariableStatusItemLength, NSView, NSWindow,
    NSWindowDelegate, NSWindowStyleMask, NSWorkspace, NSWorkspaceDidWakeNotification,
    NSWorkspaceWillSleepNotification,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSDictionary, NSDistributedNotificationCenter, NSEdgeInsets,
    NSIndexSet, NSInteger, NSNotification, NSNumber, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString, NSURL, ns_string,
};

/// Width of a Profile row in the menu.
const ROW_WIDTH: f64 = 300.0;

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
    notifications::start(|command| {
        UI.with(|cell| {
            if let Some(ui) = cell.get() {
                ui.send(command);
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
    /// Settings > Hidden (n)…: each entry's title and Restore command, by tag.
    hidden: RefCell<Vec<(String, Command)>>,
    all_sessions: OnceCell<AllSessionsWindow>,
    /// The rows the All Sessions table shows (its data source).
    session_rows: RefCell<Vec<SessionRow>>,
    /// The detail pane as last drawn.
    session_detail: RefCell<Option<SessionDetail>>,
    /// Set while the UI changes a selection itself, so the selection
    /// callbacks don't echo it back to the core.
    rendering: Cell<bool>,
    /// The FSEvents watch the core asked for while All Sessions is open.
    watch: RefCell<Option<FolderWatch>>,
    /// The FSEvents watch on the sidebars while Sidebar sync is on.
    sidebar_watch: RefCell<Option<FolderWatch>>,
}

struct AllSessionsWindow {
    window: Retained<NSWindow>,
    search: Retained<NSSearchField>,
    status: Retained<NSTextField>,
    sidebar: Retained<NSOutlineView>,
    sidebar_source: Retained<SidebarSource>,
    table: Retained<NSTableView>,
    detail: Retained<NSStackView>,
}

struct FirstRunWindow {
    window: Retained<NSWindow>,
    launch_at_login: Retained<NSButton>,
}

struct DiagnosticsWindow {
    window: Retained<NSWindow>,
    text: Retained<NSTextField>,
    /// One Resume button per feature that can be paused, hidden when unused.
    resume: Vec<Retained<NSButton>>,
}

struct SettingsWindow {
    window: Retained<NSWindow>,
    sidebar_sync: Retained<NSButton>,
    sidebar_sync_automatic: Retained<NSButton>,
    sync_now: Retained<NSButton>,
    sidebar_sync_note: Retained<NSTextField>,
    cli_switching: Retained<NSButton>,
    cli_switching_note: Retained<NSTextField>,
    launch_at_login: Retained<NSButton>,
    launch_at_login_note: Retained<NSTextField>,
    verbose_logging: Retained<NSButton>,
    verbose_logging_note: Retained<NSTextField>,
    hidden: Retained<NSButton>,
    thresholds: Retained<NSTextField>,
    reset_notification: Retained<NSButton>,
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

        #[unsafe(method(syncNowClicked:))]
        fn sync_now_clicked(&self, _sender: &NSButton) {
            self.send(Command::SyncSidebarNow);
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
            actions: RefCell::new(Vec::new()),
            settings: OnceCell::new(),
            diagnostics: OnceCell::new(),
            resume_commands: RefCell::new(Vec::new()),
            first_run: RefCell::new(None),
            menu_open: Cell::new(false),
            pending_menu: RefCell::new(None),
            shown_menu: RefCell::new(Vec::new()),
            hidden: RefCell::new(Vec::new()),
            all_sessions: OnceCell::new(),
            session_rows: RefCell::new(Vec::new()),
            session_detail: RefCell::new(None),
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
        if let Some(button) = item.button(mtm) {
            button.setTitle(&NSString::from_str(&view.menu_bar.label));
            button.setAppearsDisabled(view.menu_bar.dimmed);
        }
        if self.ivars().menu_open.get() {
            if !self.update_open_menu(&view.menu.items) {
                *self.ivars().pending_menu.borrow_mut() = Some(view.menu.items.clone());
            }
        } else {
            self.set_menu(&view.menu.items);
        }
        *self.ivars().hidden.borrow_mut() = view
            .settings
            .hidden_discovered
            .iter()
            .map(|hidden| (hidden.title.clone(), hidden.restore.clone()))
            .collect();
        if let Some(settings) = self.ivars().settings.get() {
            render_settings(settings, &view.settings);
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
        if shown.len() != items.len() || menu.numberOfItems() as usize != items.len() {
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

    /// Refreshes the All Sessions list. The search field is the user's; it is
    /// only filled from the view while it isn't being edited.
    fn render_all_sessions(&self, window: &AllSessionsWindow, view: &AllSessionsView) {
        self.ivars().rendering.set(true);
        window
            .sidebar_source
            .render(&window.sidebar, &view.sidebar, &view.filter);
        let changed = *self.ivars().session_rows.borrow() != view.rows;
        if changed {
            *self.ivars().session_rows.borrow_mut() = view.rows.clone();
            window.table.reloadData();
            // Keep the selection on the same Session as rows come and go.
            let selected = view
                .detail
                .as_ref()
                .and_then(|detail| view.rows.iter().position(|row| row.id == detail.id));
            match selected {
                Some(row) => window.table.selectRowIndexes_byExtendingSelection(
                    &NSIndexSet::indexSetWithIndex(row),
                    false,
                ),
                // SAFETY: plain deselection of our own table on the main thread.
                None => unsafe { window.table.deselectAll(None) },
            }
        }
        if *self.ivars().session_detail.borrow() != view.detail {
            *self.ivars().session_detail.borrow_mut() = view.detail.clone();
            render_session_detail(self.mtm(), &window.detail, view.detail.as_ref());
        }
        self.ivars().rendering.set(false);
        let count = view.rows.len();
        let status = match (view.loading, count) {
            (true, _) => format!("Indexing Sessions… {count}"),
            (false, 1) => "1 Session".to_owned(),
            (false, _) => format!("{count} Sessions"),
        };
        window.status.setStringValue(&NSString::from_str(&status));
        if window.search.currentEditor().is_none() {
            window
                .search
                .setStringValue(&NSString::from_str(&view.search));
        }
    }

    /// Opens the first-run window when the view has an offer, closes it once
    /// the view no longer does.
    fn render_first_run(&self, view: &View) {
        let mut open = self.ivars().first_run.borrow_mut();
        match (&view.first_run, open.is_some()) {
            (Some(offer), false) => {
                let first_run = self.make_first_run_window(offer.launch_at_login);
                first_run.window.center();
                first_run.window.makeKeyAndOrderFront(None);
                NSApplication::sharedApplication(self.mtm()).activate();
                *open = Some(first_run);
            }
            (None, true) => {
                if let Some(first_run) = open.take() {
                    first_run.window.close();
                }
            }
            _ => {}
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
        for item in items {
            let row = match item {
                MenuItem::Separator => NSMenuItem::separatorItem(mtm),
                MenuItem::Warning(text) => disabled_item(mtm, &format!("⚠︎ {text}")),
                MenuItem::Detail(text) => disabled_item(mtm, text),
                MenuItem::Submenu { title, items } => {
                    let row = menu_item(mtm, title);
                    row.setSubmenu(Some(&self.build_menu(items, actions)));
                    row
                }
                MenuItem::Action { title, command } => {
                    let row = menu_item(mtm, title);
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
                    row.setView(Some(&RowView::wrap(
                        mtm,
                        &profile_row_content(mtm, profile),
                    )));
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
                    row.setView(Some(&RowView::wrap(
                        mtm,
                        &discovered_row_content(mtm, discovered),
                    )));
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
                settings.window.center();
                settings.window.makeKeyAndOrderFront(None);
                app.activate();
            }
            Effect::ShowDiagnostics => {
                let diagnostics = self
                    .ivars()
                    .diagnostics
                    .get_or_init(|| self.make_diagnostics_window());
                self.render_diagnostics(diagnostics, &view.diagnostics);
                diagnostics.window.center();
                diagnostics.window.makeKeyAndOrderFront(None);
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
                window.window.makeKeyAndOrderFront(None);
                app.activate();
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
            Effect::Notify { title, body } => notifications::post(&title, &body, None),
            Effect::NotifyWithAction {
                title,
                body,
                action,
                command,
            } => notifications::post(&title, &body, Some((&action, command))),
            Effect::Terminate => app.terminate(None),
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
                app.activate();
                if alert.runModal() == NSAlertFirstButtonReturn {
                    self.send(confirm);
                }
            }
        }
    }

    /// A small window with the launch-at-login checkbox and Done. It has no
    /// close button, so the offer is always answered.
    fn make_first_run_window(&self, launch_at_login: bool) -> FirstRunWindow {
        let mtm = self.mtm();
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(380.0, 150.0));
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                NSWindowStyleMask::Titled,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(ns_string!("Welcome to AccRoster"));
        let content = window.contentView().expect("a window has a content view");

        let message = NSTextField::wrappingLabelWithString(
            ns_string!("AccRoster lives in the menu bar. You can change this later in Settings."),
            mtm,
        );
        message.setFrame(NSRect::new(
            NSPoint::new(20.0, 90.0),
            NSSize::new(340.0, 40.0),
        ));
        content.addSubview(&message);

        let checkbox = unsafe {
            NSButton::checkboxWithTitle_target_action(
                ns_string!("Open AccRoster at login"),
                None,
                None,
                mtm,
            )
        };
        checkbox.setState(if launch_at_login {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        checkbox.setFrameOrigin(NSPoint::new(20.0, 60.0));
        content.addSubview(&checkbox);

        let done = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Done"),
                Some(self),
                Some(sel!(firstRunDoneClicked:)),
                mtm,
            )
        };
        done.setKeyEquivalent(ns_string!("\r"));
        done.setFrameOrigin(NSPoint::new(290.0, 14.0));
        content.addSubview(&done);

        FirstRunWindow {
            window,
            launch_at_login: checkbox,
        }
    }

    /// A resizable three-pane window under a status line and the search
    /// field: the sidebar (`NSOutlineView`), the Session table, the detail
    /// pane.
    fn make_all_sessions_window(&self) -> AllSessionsWindow {
        let mtm = self.mtm();
        let (width, height) = (1040.0, 600.0);
        let (sidebar_width, detail_width, top) = (200.0, 320.0, 50.0);
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height));
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Resizable
                    | NSWindowStyleMask::Miniaturizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(ns_string!("All Sessions"));
        window.setMinSize(NSSize::new(760.0, 320.0));
        window.setDelegate(Some(ProtocolObject::from_ref(self)));
        let content = window.contentView().expect("a window has a content view");

        let status = NSTextField::labelWithString(ns_string!(""), mtm);
        status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        status.setFrame(NSRect::new(
            NSPoint::new(16.0, height - 34.0),
            NSSize::new(width - 280.0, 18.0),
        ));
        status.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        content.addSubview(&status);

        let search = NSSearchField::new(mtm);
        search.setFrame(NSRect::new(
            NSPoint::new(width - 256.0, height - 38.0),
            NSSize::new(240.0, 24.0),
        ));
        search.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        search.setPlaceholderString(Some(ns_string!("Title, prompt, project, branch")));
        search.setSendsSearchStringImmediately(true);
        unsafe {
            search.setTarget(Some(self));
            search.setAction(Some(sel!(searchChanged:)));
        }
        content.addSubview(&search);

        // Sidebar.
        let sidebar_frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(sidebar_width, height - top),
        );
        let sidebar = NSOutlineView::initWithFrame(NSOutlineView::alloc(mtm), sidebar_frame);
        let column =
            NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), ns_string!("sidebar"));
        column.setWidth(sidebar_width);
        sidebar.addTableColumn(&column);
        // SAFETY: the column was just added to this outline view.
        unsafe { sidebar.setOutlineTableColumn(Some(&column)) };
        sidebar.setHeaderView(None);
        sidebar.setStyle(NSTableViewStyle::SourceList);
        sidebar.setRowHeight(22.0);
        let sidebar_source = SidebarSource::new(mtm);
        unsafe {
            sidebar.setDataSource(Some(ProtocolObject::from_ref(&*sidebar_source)));
            sidebar.setDelegate(Some(ProtocolObject::from_ref(&*sidebar_source)));
        }
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), sidebar_frame);
        scroll.setHasVerticalScroller(true);
        scroll.setDocumentView(Some(&sidebar));
        scroll.setAutoresizingMask(NSAutoresizingMaskOptions::ViewHeightSizable);
        content.addSubview(&scroll);

        // Session list.
        let list_frame = NSRect::new(
            NSPoint::new(sidebar_width, 0.0),
            NSSize::new(width - sidebar_width - detail_width, height - top),
        );
        let table = NSTableView::initWithFrame(NSTableView::alloc(mtm), list_frame);
        let column =
            NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), ns_string!("session"));
        column.setWidth(list_frame.size.width);
        table.addTableColumn(&column);
        table.setHeaderView(None);
        table.setStyle(NSTableViewStyle::FullWidth);
        table.setRowHeight(40.0);
        unsafe {
            table.setDataSource(Some(ProtocolObject::from_ref(self)));
            table.setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), list_frame);
        scroll.setHasVerticalScroller(true);
        scroll.setDocumentView(Some(&table));
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        content.addSubview(&scroll);
        table.sizeLastColumnToFit();

        // Detail pane: a top-aligned stack, refilled on every change.
        let detail = stack(mtm, true, 8.0, &[]);
        detail.setEdgeInsets(NSEdgeInsets {
            top: 12.0,
            left: 14.0,
            bottom: 12.0,
            right: 14.0,
        });
        let detail_frame = NSRect::new(
            NSPoint::new(width - detail_width, 0.0),
            NSSize::new(detail_width, height - top),
        );
        let holder = NSView::initWithFrame(NSView::alloc(mtm), detail_frame);
        holder.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        holder.addSubview(&detail);
        detail.setTranslatesAutoresizingMaskIntoConstraints(false);
        for (a, b) in [
            (detail.leadingAnchor(), holder.leadingAnchor()),
            (detail.trailingAnchor(), holder.trailingAnchor()),
        ] {
            a.constraintEqualToAnchor(&b).setActive(true);
        }
        detail
            .topAnchor()
            .constraintEqualToAnchor(&holder.topAnchor())
            .setActive(true);
        content.addSubview(&holder);
        render_session_detail(mtm, &detail, None);

        AllSessionsWindow {
            window,
            search,
            status,
            sidebar,
            sidebar_source,
            table,
            detail,
        }
    }

    /// The Diagnostics window: the checks and versions as selectable text,
    /// a Resume button per resumable pause, and Copy report.
    fn make_diagnostics_window(&self) -> DiagnosticsWindow {
        let mtm = self.mtm();
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(720.0, 440.0));
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(ns_string!("AccRoster Diagnostics"));
        let content = window.contentView().expect("a window has a content view");

        let text = NSTextField::wrappingLabelWithString(ns_string!(""), mtm);
        text.setSelectable(true);
        text.setFont(Some(&NSFont::systemFontOfSize(
            NSFont::smallSystemFontSize(),
        )));
        text.setFrame(NSRect::new(
            NSPoint::new(20.0, 64.0),
            NSSize::new(680.0, 360.0),
        ));
        content.addSubview(&text);

        let copy = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Copy report"),
                Some(self),
                Some(sel!(copyReportClicked:)),
                mtm,
            )
        };
        copy.setFrameOrigin(NSPoint::new(14.0, 20.0));
        content.addSubview(&copy);

        let resume = [140.0, 330.0, 530.0]
            .into_iter()
            .map(|x| {
                let button = unsafe {
                    NSButton::buttonWithTitle_target_action(
                        ns_string!("Resume"),
                        Some(self),
                        Some(sel!(resumeClicked:)),
                        mtm,
                    )
                };
                button.setFrameOrigin(NSPoint::new(x, 20.0));
                button.setHidden(true);
                content.addSubview(&button);
                button
            })
            .collect();

        DiagnosticsWindow {
            window,
            text,
            resume,
        }
    }

    fn render_diagnostics(&self, diagnostics: &DiagnosticsWindow, view: &DiagnosticsView) {
        let mut lines = Vec::new();
        for paused in &view.paused {
            let state = if paused.resume.is_some() {
                "checks pass again: Resume when ready"
            } else {
                "waiting for the checks to pass"
            };
            lines.push(format!("{}: {} ({state})", paused.title, paused.reason));
        }
        if !view.paused.is_empty() {
            lines.push(String::new());
        }
        let versions = &view.versions;
        let unknown = || "unknown".to_owned();
        lines.push(format!(
            "Claude Code: {}",
            versions.claude_code.clone().unwrap_or_else(unknown)
        ));
        lines.push(format!(
            "Desktop's Claude Code: {}",
            if versions.desktop_claude_code.is_empty() {
                unknown()
            } else {
                versions.desktop_claude_code.join(", ")
            }
        ));
        lines.push(format!(
            "Claude desktop app: {}",
            versions.desktop.clone().unwrap_or_else(unknown)
        ));
        lines.push(String::new());
        for check in &view.checks {
            let mark = match check.status {
                CheckStatus::Pass => "✓",
                CheckStatus::Fail => "✗",
                CheckStatus::NeverSeen => "–",
            };
            let status = match check.status {
                CheckStatus::Pass => "pass",
                CheckStatus::Fail => "fail",
                CheckStatus::NeverSeen => "never seen",
            };
            match &check.detail {
                Some(detail) => lines.push(format!("{mark} {} · {status} · {detail}", check.name)),
                None => lines.push(format!("{mark} {} · {status}", check.name)),
            }
        }
        diagnostics
            .text
            .setStringValue(&NSString::from_str(&lines.join("\n")));

        let resumable: Vec<(String, Command)> = view
            .paused
            .iter()
            .filter_map(|paused| {
                let command = paused.resume.clone()?;
                let feature = paused.title.trim_end_matches(" paused");
                Some((format!("Resume {feature}"), command))
            })
            .collect();
        for (index, button) in diagnostics.resume.iter().enumerate() {
            match resumable.get(index) {
                Some((title, _)) => {
                    button.setTitle(&NSString::from_str(title));
                    button.sizeToFit();
                    button.setTag(index as isize);
                    button.setHidden(false);
                }
                None => button.setHidden(true),
            }
        }
        *self.ivars().resume_commands.borrow_mut() =
            resumable.into_iter().map(|(_, command)| command).collect();
    }

    fn make_settings_window(&self) -> SettingsWindow {
        let mtm = self.mtm();
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(420.0, 488.0));
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

        let sidebar_sync = unsafe {
            NSButton::checkboxWithTitle_target_action(
                ns_string!("Sidebar sync: every Session in every Profile's desktop sidebar"),
                Some(self),
                Some(sel!(sidebarSyncClicked:)),
                mtm,
            )
        };
        sidebar_sync.setFrameOrigin(NSPoint::new(20.0, 456.0));
        content.addSubview(&sidebar_sync);

        let sidebar_sync_automatic = unsafe {
            NSButton::checkboxWithTitle_target_action(
                ns_string!("Automatically, a few seconds after each change"),
                Some(self),
                Some(sel!(sidebarSyncAutomaticClicked:)),
                mtm,
            )
        };
        sidebar_sync_automatic.setFrameOrigin(NSPoint::new(34.0, 434.0));
        content.addSubview(&sidebar_sync_automatic);

        let sync_now = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Sync now"),
                Some(self),
                Some(sel!(syncNowClicked:)),
                mtm,
            )
        };
        sync_now.setFrameOrigin(NSPoint::new(34.0, 400.0));
        content.addSubview(&sync_now);

        let sidebar_sync_note = NSTextField::wrappingLabelWithString(ns_string!(""), mtm);
        sidebar_sync_note.setFont(Some(&NSFont::systemFontOfSize(
            NSFont::smallSystemFontSize(),
        )));
        sidebar_sync_note.setFrame(NSRect::new(
            NSPoint::new(140.0, 396.0),
            NSSize::new(260.0, 34.0),
        ));
        content.addSubview(&sidebar_sync_note);

        let thresholds_label =
            NSTextField::labelWithString(ns_string!("Notify when a limit reaches (%):"), mtm);
        thresholds_label.setFrameOrigin(NSPoint::new(20.0, 370.0));
        thresholds_label.sizeToFit();
        content.addSubview(&thresholds_label);
        let thresholds = NSTextField::textFieldWithString(ns_string!(""), mtm);
        thresholds.setFrame(NSRect::new(
            NSPoint::new(240.0, 367.0),
            NSSize::new(80.0, 22.0),
        ));
        unsafe {
            thresholds.setTarget(Some(self));
            thresholds.setAction(Some(sel!(thresholdsChanged:)));
        }
        content.addSubview(&thresholds);

        let reset_notification = unsafe {
            NSButton::checkboxWithTitle_target_action(
                ns_string!("Also notify when a limit that warned resets"),
                Some(self),
                Some(sel!(resetNotificationClicked:)),
                mtm,
            )
        };
        reset_notification.setFrameOrigin(NSPoint::new(20.0, 338.0));
        content.addSubview(&reset_notification);

        let cli_switching = unsafe {
            NSButton::checkboxWithTitle_target_action(
                ns_string!("CLI switching: clicking a Profile picks it for new claude runs"),
                Some(self),
                Some(sel!(cliSwitchingClicked:)),
                mtm,
            )
        };
        cli_switching.setFrameOrigin(NSPoint::new(20.0, 302.0));
        content.addSubview(&cli_switching);

        let cli_switching_note = NSTextField::wrappingLabelWithString(ns_string!(""), mtm);
        cli_switching_note.setSelectable(true);
        cli_switching_note.setFont(Some(&NSFont::systemFontOfSize(
            NSFont::smallSystemFontSize(),
        )));
        cli_switching_note.setFrame(NSRect::new(
            NSPoint::new(40.0, 196.0),
            NSSize::new(360.0, 102.0),
        ));
        content.addSubview(&cli_switching_note);

        let launch_at_login = unsafe {
            NSButton::checkboxWithTitle_target_action(
                ns_string!("Open AccRoster at login"),
                Some(self),
                Some(sel!(launchAtLoginClicked:)),
                mtm,
            )
        };
        launch_at_login.setFrameOrigin(NSPoint::new(20.0, 162.0));
        content.addSubview(&launch_at_login);

        let launch_at_login_note = NSTextField::wrappingLabelWithString(ns_string!(""), mtm);
        launch_at_login_note.setFont(Some(&NSFont::systemFontOfSize(
            NSFont::smallSystemFontSize(),
        )));
        launch_at_login_note.setFrame(NSRect::new(
            NSPoint::new(40.0, 126.0),
            NSSize::new(360.0, 32.0),
        ));
        content.addSubview(&launch_at_login_note);

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

        let hidden = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Hidden (0)…"),
                Some(self),
                Some(sel!(hiddenClicked:)),
                mtm,
            )
        };
        hidden.setFrameOrigin(NSPoint::new(230.0, 24.0));
        content.addSubview(&hidden);

        SettingsWindow {
            window,
            sidebar_sync,
            sidebar_sync_automatic,
            sync_now,
            sidebar_sync_note,
            cli_switching,
            cli_switching_note,
            launch_at_login,
            launch_at_login_note,
            verbose_logging,
            verbose_logging_note,
            hidden,
            thresholds,
            reset_notification,
        }
    }
}

/// One line of the All Sessions sidebar.
enum SidebarLine {
    Heading(String),
    Item(String, SessionFilter),
}

pub struct SidebarIvars {
    lines: RefCell<Vec<SidebarLine>>,
    /// One item object per line: the outline view doesn't retain its items.
    items: RefCell<Vec<Retained<NSNumber>>>,
    /// The groups as last drawn.
    shown: RefCell<Vec<SidebarGroup>>,
}

define_class!(
    /// The All Sessions sidebar's data source and delegate: a flat list of
    /// headings (group rows) and pickable items.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AccRosterSidebarSource"]
    #[ivars = SidebarIvars]
    struct SidebarSource;

    unsafe impl NSObjectProtocol for SidebarSource {}

    unsafe impl NSControlTextEditingDelegate for SidebarSource {}

    unsafe impl NSOutlineViewDataSource for SidebarSource {
        #[unsafe(method(outlineView:numberOfChildrenOfItem:))]
        fn number_of_children(
            &self,
            _outline: &NSOutlineView,
            item: Option<&AnyObject>,
        ) -> NSInteger {
            match item {
                None => self.ivars().items.borrow().len() as NSInteger,
                Some(_) => 0,
            }
        }

        #[unsafe(method_id(outlineView:child:ofItem:))]
        fn child(
            &self,
            _outline: &NSOutlineView,
            index: NSInteger,
            _item: Option<&AnyObject>,
        ) -> Retained<AnyObject> {
            let items = self.ivars().items.borrow();
            let item = usize::try_from(index)
                .ok()
                .and_then(|index| items.get(index).cloned())
                .unwrap_or_else(|| NSNumber::new_isize(-1));
            item.into()
        }

        #[unsafe(method(outlineView:isItemExpandable:))]
        fn is_expandable(&self, _outline: &NSOutlineView, _item: &AnyObject) -> bool {
            false
        }
    }

    unsafe impl NSOutlineViewDelegate for SidebarSource {
        #[unsafe(method_id(outlineView:viewForTableColumn:item:))]
        fn view_for_item(
            &self,
            _outline: &NSOutlineView,
            _column: Option<&NSTableColumn>,
            item: &AnyObject,
        ) -> Option<Retained<NSView>> {
            self.line_view(item)
        }

        #[unsafe(method(outlineView:isGroupItem:))]
        fn is_group(&self, _outline: &NSOutlineView, item: &AnyObject) -> bool {
            self.is_heading(item)
        }

        #[unsafe(method(outlineView:shouldSelectItem:))]
        fn should_select(&self, _outline: &NSOutlineView, item: &AnyObject) -> bool {
            !self.is_heading(item)
        }

        /// An item was picked: the list shows its Sessions.
        #[unsafe(method(outlineViewSelectionDidChange:))]
        fn selection_did_change(&self, notification: &NSNotification) {
            let picked = UI.with(|cell| {
                let ui = cell.get()?;
                if ui.ivars().rendering.get() {
                    return None;
                }
                let outline = notification.object()?.downcast::<NSOutlineView>().ok()?;
                let row = usize::try_from(outline.selectedRow()).ok()?;
                match self.ivars().lines.borrow().get(row)? {
                    SidebarLine::Item(_, filter) => Some((ui.clone(), filter.clone())),
                    SidebarLine::Heading(_) => None,
                }
            });
            if let Some((ui, filter)) = picked {
                ui.send(Command::FilterSessions(filter));
            }
        }
    }
);

impl SidebarSource {
    fn new(mtm: MainThreadMarker) -> Retained<SidebarSource> {
        let this = SidebarSource::alloc(mtm).set_ivars(SidebarIvars {
            lines: RefCell::new(Vec::new()),
            items: RefCell::new(Vec::new()),
            shown: RefCell::new(Vec::new()),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn line_view(&self, item: &AnyObject) -> Option<Retained<NSView>> {
        let mtm = MainThreadMarker::from(self);
        let lines = self.ivars().lines.borrow();
        let view = match lines.get(line_index(item)?)? {
            SidebarLine::Heading(title) => {
                label(mtm, title, NSFont::boldSystemFontOfSize(11.0), true)
            }
            SidebarLine::Item(title, _) => label(mtm, title, NSFont::systemFontOfSize(12.0), false),
        };
        view.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        Some(view.into_super().into_super())
    }

    fn is_heading(&self, item: &AnyObject) -> bool {
        line_index(item).is_some_and(|index| {
            matches!(
                self.ivars().lines.borrow().get(index),
                Some(SidebarLine::Heading(_))
            )
        })
    }

    /// Redraws the groups when they changed and selects `filter`'s item.
    fn render(&self, outline: &NSOutlineView, groups: &[SidebarGroup], filter: &SessionFilter) {
        if *self.ivars().shown.borrow() != groups {
            *self.ivars().shown.borrow_mut() = groups.to_vec();
            let mut lines = Vec::new();
            for group in groups {
                lines.extend(group.title.clone().map(SidebarLine::Heading));
                lines.extend(
                    group
                        .items
                        .iter()
                        .map(|item| SidebarLine::Item(item.title.clone(), item.filter.clone())),
                );
            }
            *self.ivars().items.borrow_mut() = (0..lines.len())
                .map(|index| NSNumber::new_isize(index as isize))
                .collect();
            *self.ivars().lines.borrow_mut() = lines;
            outline.reloadData();
        }
        let selected = self
            .ivars()
            .lines
            .borrow()
            .iter()
            .position(|line| matches!(line, SidebarLine::Item(_, f) if f == filter));
        if let Some(row) = selected
            && usize::try_from(outline.selectedRow()).ok() != Some(row)
        {
            outline
                .selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(row), false);
        }
    }
}

/// The sidebar line an outline item stands for.
fn line_index(item: &AnyObject) -> Option<usize> {
    let number = item.downcast_ref::<NSNumber>()?;
    usize::try_from(number.as_isize()).ok()
}

/// Refills the detail pane: title, folder and branch, last client and age,
/// turns, first prompt, the running state, one ✓/– line per Profile's
/// Sidebar, and why nothing opens when the Transcript is gone.
fn render_session_detail(
    mtm: MainThreadMarker,
    pane: &NSStackView,
    detail: Option<&SessionDetail>,
) {
    const WIDTH: f64 = 290.0;
    for view in pane.arrangedSubviews() {
        view.removeFromSuperview();
    }
    let wrapping = |text: &str, font: Retained<NSFont>, color: Retained<NSColor>| {
        let field = NSTextField::wrappingLabelWithString(&NSString::from_str(text), mtm);
        field.setFont(Some(&font));
        field.setTextColor(Some(&color));
        field.setPreferredMaxLayoutWidth(WIDTH);
        fixed_width(&field, WIDTH);
        field
    };
    let Some(detail) = detail else {
        pane.addArrangedSubview(&wrapping(
            "Select a Session.",
            NSFont::systemFontOfSize(13.0),
            NSColor::secondaryLabelColor(),
        ));
        return;
    };
    let small = || NSFont::systemFontOfSize(11.0);
    pane.addArrangedSubview(&wrapping(
        &detail.title,
        NSFont::systemFontOfSize(15.0),
        if detail.deleted.is_some() {
            NSColor::tertiaryLabelColor()
        } else {
            NSColor::labelColor()
        },
    ));
    let place: Vec<&str> = [detail.folder.as_deref(), detail.branch.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    let mut facts = vec![place.join("  ·  ")];
    facts.push(match detail.turns {
        Some(1) => format!("{}  ·  1 turn", detail.last_used),
        Some(turns) => format!("{}  ·  {turns} turns", detail.last_used),
        None => detail.last_used.clone(),
    });
    if let Some(prompt) = &detail.first_prompt {
        let mut shown: String = prompt.chars().take(200).collect();
        if prompt.chars().count() > 200 {
            shown.push('…');
        }
        facts.push(format!("First prompt: “{shown}”"));
    }
    facts.retain(|line| !line.is_empty());
    pane.addArrangedSubview(&wrapping(
        &facts.join("\n"),
        small(),
        NSColor::secondaryLabelColor(),
    ));
    match detail.dot {
        Some(RunningDot::Terminal) => pane.addArrangedSubview(&wrapping(
            "● Running in Terminal now",
            NSFont::boldSystemFontOfSize(11.0),
            NSColor::systemGreenColor(),
        )),
        Some(RunningDot::Desktop) => pane.addArrangedSubview(&wrapping(
            "● A desktop turn is running now",
            NSFont::boldSystemFontOfSize(11.0),
            NSColor::systemOrangeColor(),
        )),
        None => {}
    }
    pane.addArrangedSubview(&wrapping(
        "Sidebar entries",
        NSFont::boldSystemFontOfSize(11.0),
        NSColor::secondaryLabelColor(),
    ));
    for sidebar in &detail.sidebars {
        let mut line = format!(
            "{}  {}",
            if sidebar.has_entry { "✓" } else { "–" },
            sidebar.profile
        );
        if sidebar.archived {
            line.push_str("  (archived)");
        }
        if sidebar.desktop_now {
            line.push_str("  (desktop now)");
        }
        let color = if sidebar.has_entry {
            NSColor::labelColor()
        } else {
            NSColor::tertiaryLabelColor()
        };
        pane.addArrangedSubview(&wrapping(&line, small(), color));
    }
    if let Some(why) = &detail.deleted {
        pane.addArrangedSubview(&wrapping(why, small(), NSColor::systemOrangeColor()));
    }
}

define_class!(
    /// A custom-view menu row. Custom views get no highlight from NSMenu, so
    /// it draws its own when its item is highlighted (hovered).
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AccRosterRowView"]
    struct RowView;

    impl RowView {
        /// A custom view gets the click instead of the menu: close the menu,
        /// then send the item's action as a plain item would.
        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            let Some(item) = self.enclosingMenuItem() else {
                return;
            };
            let Some(action) = item.action() else {
                return;
            };
            // SAFETY: the item belongs to the open menu; reading its menu
            // has no other precondition.
            if let Some(menu) = unsafe { item.menu() } {
                menu.cancelTracking();
            }
            let target = item.target();
            unsafe {
                NSApplication::sharedApplication(MainThreadMarker::from(self)).sendAction_to_from(
                    action,
                    target.as_deref(),
                    Some(&item),
                );
            }
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            if self.enclosingMenuItem().is_some_and(|item| item.isHighlighted()) {
                let bounds = self.bounds();
                let inset = NSRect::new(
                    NSPoint::new(bounds.origin.x + 5.0, bounds.origin.y + 1.0),
                    NSSize::new(bounds.size.width - 10.0, bounds.size.height - 2.0),
                );
                NSColor::controlAccentColor().colorWithAlphaComponent(0.18).set();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(inset, 5.0, 5.0).fill();
            }
        }
    }
);

impl RowView {
    /// A row of [`ROW_WIDTH`] holding `content`, as tall as it needs.
    fn wrap(mtm: MainThreadMarker, content: &NSView) -> Retained<RowView> {
        content.setTranslatesAutoresizingMaskIntoConstraints(false);
        content
            .widthAnchor()
            .constraintEqualToConstant(ROW_WIDTH)
            .setActive(true);
        let height = content.fittingSize().height;
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(ROW_WIDTH, height));
        let this = RowView::alloc(mtm).set_ivars(());
        let this: Retained<RowView> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.addSubview(content);
        for (a, b) in [
            (content.leadingAnchor(), this.leadingAnchor()),
            (content.trailingAnchor(), this.trailingAnchor()),
        ] {
            a.constraintEqualToAnchor(&b).setActive(true);
        }
        content
            .topAnchor()
            .constraintEqualToAnchor(&this.topAnchor())
            .setActive(true);
        content
            .bottomAnchor()
            .constraintEqualToAnchor(&this.bottomAnchor())
            .setActive(true);
        this
    }
}

/// A Profile row: the name and a submenu arrow, one bar per limit, the Banked
/// reset / Session refill lines, the note.
fn profile_row_content(mtm: MainThreadMarker, row: &ProfileRow) -> Retained<NSView> {
    let name = label(mtm, &row.title, NSFont::boldSystemFontOfSize(13.0), false);
    let badges: Vec<Retained<NSTextField>> = row
        .badges
        .iter()
        .map(String::as_str)
        .chain(row.most_headroom.then_some("Most headroom"))
        .map(|badge| label(mtm, badge, NSFont::boldSystemFontOfSize(9.0), true))
        .collect();
    let filler = NSView::new(mtm);
    filler.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    let arrow = label(mtm, "›", NSFont::systemFontOfSize(13.0), true);
    let mut name_line: Vec<&NSView> = vec![&name];
    name_line.extend(badges.iter().map(|badge| -> &NSView { badge }));
    name_line.extend([&*filler, &arrow]);
    let mut lines: Vec<Retained<NSView>> = vec![stack(mtm, false, 5.0, &name_line).into_super()];
    lines.extend(row.limits.iter().map(|bar| {
        let line = limit_line(mtm, bar);
        if row.dimmed {
            line.setAlphaValue(0.45);
        }
        line
    }));
    lines.extend(row.offers.iter().map(|offer| {
        label(mtm, offer, NSFont::systemFontOfSize(10.0), false)
            .into_super()
            .into_super()
    }));
    if let Some(note) = &row.note {
        lines.push(
            label(mtm, note, NSFont::systemFontOfSize(10.0), true)
                .into_super()
                .into_super(),
        );
    }
    let refs: Vec<&NSView> = lines.iter().map(|view| &**view).collect();
    let body = stack(mtm, true, 3.0, &refs);
    if row.dimmed {
        body.setAlphaValue(0.5);
    }
    body.setEdgeInsets(NSEdgeInsets {
        top: 5.0,
        left: 16.0,
        bottom: 5.0,
        right: 14.0,
    });
    // Every line as wide as the row's content area.
    for line in body.arrangedSubviews().iter() {
        line.widthAnchor()
            .constraintEqualToAnchor_constant(&body.widthAnchor(), -30.0)
            .setActive(true);
    }
    body.into_super()
}

/// The Claude desktop app's bundle id.
const DESKTOP_BUNDLE_ID: &str = "com.anthropic.claudefordesktop";

/// Opens Claude, or brings it forward when it runs. Never quits or relaunches
/// it: `openApplicationAtURL` activates a running app in place.
fn bring_desktop_app_forward() {
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

/// A Discovered Profile row: greyed `Sign in to …`, a submenu arrow, and the
/// Unknown Login line.
fn discovered_row_content(mtm: MainThreadMarker, row: &DiscoveredRow) -> Retained<NSView> {
    let name = label(mtm, &row.title, NSFont::systemFontOfSize(13.0), true);
    let filler = NSView::new(mtm);
    filler.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    let arrow = label(mtm, "›", NSFont::systemFontOfSize(13.0), true);
    let mut lines: Vec<Retained<NSView>> =
        vec![stack(mtm, false, 5.0, &[&name, &filler, &arrow]).into_super()];
    if let Some(subtitle) = &row.subtitle {
        let line = label(mtm, subtitle, NSFont::systemFontOfSize(10.0), true);
        line.setTextColor(Some(&NSColor::tertiaryLabelColor()));
        lines.push(line.into_super().into_super());
    }
    let refs: Vec<&NSView> = lines.iter().map(|view| &**view).collect();
    let body = stack(mtm, true, 2.0, &refs);
    body.setEdgeInsets(NSEdgeInsets {
        top: 4.0,
        left: 16.0,
        bottom: 4.0,
        right: 14.0,
    });
    for line in body.arrangedSubviews().iter() {
        line.widthAnchor()
            .constraintEqualToAnchor_constant(&body.widthAnchor(), -30.0)
            .setActive(true);
    }
    body.into_super()
}

/// Asks for a nickname; `None` on Cancel. An empty answer clears it.
fn ask_nickname(mtm: MainThreadMarker, name: &str, current: Option<&str>) -> Option<String> {
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
    NSApplication::sharedApplication(mtm).activate();
    (alert.runModal() == NSAlertFirstButtonReturn).then(|| field.stringValue().to_string())
}

/// `Session ▓▓▓░░ 62%`: an `NSLevelIndicator`, continuous capacity, warning at
/// 70 and critical at 90.
/// An All Sessions row: the running dot (green for Terminal, orange for a
/// desktop turn, invisible otherwise), the title, and the subtitle.
fn session_row_view(mtm: MainThreadMarker, row: &SessionRow) -> Retained<NSView> {
    let dot = label(mtm, "●", NSFont::systemFontOfSize(10.0), false);
    dot.setTextColor(Some(&*match row.dot {
        Some(RunningDot::Terminal) => NSColor::systemGreenColor(),
        Some(RunningDot::Desktop) => NSColor::systemOrangeColor(),
        None => NSColor::clearColor(),
    }));
    let title = label(mtm, &row.title, NSFont::systemFontOfSize(13.0), false);
    if row.deleted {
        title.setTextColor(Some(&NSColor::tertiaryLabelColor()));
    }
    let subtitle = label(mtm, &row.subtitle, NSFont::systemFontOfSize(11.0), true);
    for line in [&title, &subtitle] {
        line.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        line.setContentCompressionResistancePriority_forOrientation(
            1.0,
            NSLayoutConstraintOrientation::Horizontal,
        );
    }
    let lines = stack(mtm, true, 1.0, &[&title, &subtitle]);
    let row = stack(mtm, false, 6.0, &[&dot, &lines]);
    row.setEdgeInsets(NSEdgeInsets {
        top: 0.0,
        left: 8.0,
        bottom: 0.0,
        right: 8.0,
    });
    row.into_super()
}

fn limit_line(mtm: MainThreadMarker, bar: &LimitBar) -> Retained<NSView> {
    let regular =
        || unsafe { NSFont::monospacedDigitSystemFontOfSize_weight(11.0, NSFontWeightRegular) };
    let name = label(mtm, &bar.label, regular(), true);
    fixed_width(&name, 60.0);
    let level = NSLevelIndicator::new(mtm);
    level.setLevelIndicatorStyle(NSLevelIndicatorStyle::ContinuousCapacity);
    level.setMinValue(0.0);
    level.setMaxValue(100.0);
    level.setWarningValue(70.0);
    level.setCriticalValue(90.0);
    level.setDoubleValue(f64::from(bar.percent));
    level.setControlSize(NSControlSize::Small);
    fixed_width(&level, 150.0);
    level
        .heightAnchor()
        .constraintEqualToConstant(10.0)
        .setActive(true);
    let text = if bar.reset {
        "Reset".to_owned()
    } else {
        format!("{}%", bar.percent)
    };
    let percent = label(mtm, &text, regular(), false);
    stack(mtm, false, 6.0, &[&name, &level, &percent]).into_super()
}

fn label(
    mtm: MainThreadMarker,
    text: &str,
    font: Retained<NSFont>,
    secondary: bool,
) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    label.setFont(Some(&font));
    if secondary {
        label.setTextColor(Some(&NSColor::secondaryLabelColor()));
    }
    label
}

fn fixed_width(view: &NSView, width: f64) {
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    view.widthAnchor()
        .constraintEqualToConstant(width)
        .setActive(true);
}

fn stack(
    mtm: MainThreadMarker,
    vertical: bool,
    spacing: f64,
    views: &[&NSView],
) -> Retained<NSStackView> {
    let stack = NSStackView::new(mtm);
    stack.setOrientation(if vertical {
        NSUserInterfaceLayoutOrientation::Vertical
    } else {
        NSUserInterfaceLayoutOrientation::Horizontal
    });
    stack.setAlignment(if vertical {
        NSLayoutAttribute::Leading
    } else {
        NSLayoutAttribute::CenterY
    });
    stack.setSpacing(spacing);
    for view in views {
        stack.addArrangedSubview(view);
    }
    stack
}

fn disabled_item(mtm: MainThreadMarker, title: &str) -> Retained<NSMenuItem> {
    let item = menu_item(mtm, title);
    item.setEnabled(false);
    item
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
    let sync = &view.sidebar_sync;
    settings
        .sidebar_sync
        .setState(if sync.mode == SidebarSyncMode::Off {
            NSControlStateValueOff
        } else {
            NSControlStateValueOn
        });
    settings
        .sidebar_sync_automatic
        .setState(if sync.mode == SidebarSyncMode::Automatic {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
    settings
        .sidebar_sync_automatic
        .setEnabled(sync.mode != SidebarSyncMode::Off);
    settings.sync_now.setEnabled(sync.sync_now.is_some());
    let sync_text = match (&sync.note, sync.mode) {
        (Some(note), _) => note.clone(),
        (None, SidebarSyncMode::Off) => "Off: sidebars are left as they are.".to_owned(),
        (None, SidebarSyncMode::Manual | SidebarSyncMode::Automatic) => {
            "Copies entries, never deletes. While Claude runs, its own Profile is left alone."
                .to_owned()
        }
    };
    settings
        .sidebar_sync_note
        .setStringValue(&NSString::from_str(&sync_text));
    let notifications = &view.notifications;
    let thresholds: Vec<String> = notifications
        .thresholds
        .iter()
        .map(u32::to_string)
        .collect();
    // Leave the field alone while the user is typing in it.
    if settings.thresholds.currentEditor().is_none() {
        settings
            .thresholds
            .setStringValue(&NSString::from_str(&thresholds.join(", ")));
    }
    settings
        .reset_notification
        .setState(if notifications.reset_notification {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
    let cli = &view.cli_switching;
    settings.cli_switching.setState(if cli.on {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    let cli_text = match &cli.note {
        Some(note) => note.clone(),
        None => format!(
            "Turning it on backs up ~/.zshrc, then adds this line at its end, between # >>> accroster >>> markers:\n{}\nOpen a new terminal afterwards. Turning it off removes it.",
            cli.line
        ),
    };
    settings
        .cli_switching_note
        .setStringValue(&NSString::from_str(&cli_text));
    let hidden = view.hidden_discovered.len();
    settings
        .hidden
        .setTitle(&NSString::from_str(&format!("Hidden ({hidden})…")));
    settings.hidden.setEnabled(hidden > 0);
    let launch_at_login = &view.launch_at_login;
    settings.launch_at_login.setState(if launch_at_login.on {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    settings.launch_at_login.setEnabled(launch_at_login.enabled);
    settings
        .launch_at_login_note
        .setStringValue(&NSString::from_str(
            launch_at_login.note.as_deref().unwrap_or(""),
        ));
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

/// Watches the Mac's network path (`NWPathMonitor`) and tells the core when
/// it is gained or lost, starting with the current state.
struct NetworkMonitor {
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
const NW_PATH_STATUS_SATISFIED: c_int = 1;

impl NetworkMonitor {
    fn start() -> NetworkMonitor {
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
