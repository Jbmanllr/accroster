//! The Settings window: its controls, how they are laid out, and how a
//! [`SettingsView`] fills them.

use accroster_core::{Command, ProfileSettings, SettingsView, SidebarSyncMode};
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, sel};
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSColor, NSControlSize, NSControlStateValueOff,
    NSControlStateValueOn, NSFont, NSLineBreakMode, NSTextField, NSView, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, ns_string};

use super::Ui;
use super::widgets::heading;

pub(super) struct SettingsWindow {
    pub(super) window: Retained<NSWindow>,
    pub(super) sidebar_sync: Retained<NSButton>,
    pub(super) sidebar_sync_automatic: Retained<NSButton>,
    pub(super) sync_now: Retained<NSButton>,
    /// Shown while the core offers Reopen Claude.
    pub(super) reopen_claude: Retained<NSButton>,
    pub(super) sidebar_sync_note: Retained<NSTextField>,
    pub(super) sync_ssh_sessions: Retained<NSButton>,
    pub(super) cli_switching: Retained<NSButton>,
    pub(super) cli_switching_note: Retained<NSTextField>,
    pub(super) launch_at_login: Retained<NSButton>,
    pub(super) launch_at_login_note: Retained<NSTextField>,
    pub(super) verbose_logging: Retained<NSButton>,
    pub(super) verbose_logging_note: Retained<NSTextField>,
    pub(super) hidden: Retained<NSButton>,
    /// Settings > Profiles, above everything else; rebuilt when the
    /// Profiles change.
    pub(super) profiles: Retained<NSView>,
    pub(super) thresholds: Retained<NSTextField>,
    pub(super) reset_notification: Retained<NSButton>,
    pub(super) retry_uninstall: Retained<NSButton>,
    pub(super) uninstall_note: Retained<NSTextField>,
}

/// Places views top-down in the bottom [`SETTINGS_HEIGHT`] points of a
/// (bottom-up) window content view.
pub(super) struct Layout<'a> {
    pub(super) content: &'a NSView,
    /// Distance from the top of the fixed part to the next view.
    pub(super) top: f64,
}

impl Ui {
    pub(super) fn make_settings_window(&self) -> SettingsWindow {
        let mtm = self.mtm();
        let frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(SETTINGS_WIDTH, SETTINGS_HEIGHT),
        );
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
        // The fixed part, laid out top-down: a bold heading per group, each
        // control's note under it in one column.
        let mut layout = Layout::new(&content);

        let small = || NSFont::systemFontOfSize(NSFont::smallSystemFontSize());
        let note = |text: &str| {
            let note = NSTextField::wrappingLabelWithString(&NSString::from_str(text), mtm);
            note.setFont(Some(&small()));
            note.setTextColor(Some(&NSColor::secondaryLabelColor()));
            note
        };
        let checkbox = |title: &str, action| unsafe {
            NSButton::checkboxWithTitle_target_action(
                &NSString::from_str(title),
                Some(self),
                Some(action),
                mtm,
            )
        };
        let push_button = |title: &str, action| unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(title),
                Some(self),
                Some(action),
                mtm,
            )
        };
        const NOTE_X: f64 = 40.0;
        const NOTE_WIDTH: f64 = SETTINGS_WIDTH - NOTE_X - 20.0;

        layout.place(&heading(mtm, "Sidebar sync"), 20.0, None, 6.0);
        let sidebar_sync = checkbox(
            "Show every Session in every Profile's desktop sidebar",
            sel!(sidebarSyncClicked:),
        );
        layout.place(&sidebar_sync, 20.0, None, 4.0);
        let sidebar_sync_automatic = checkbox(
            "Automatically, a few seconds after each change",
            sel!(sidebarSyncAutomaticClicked:),
        );
        layout.place(&sidebar_sync_automatic, NOTE_X, None, 4.0);
        let sync_ssh_sessions = checkbox(
            "Include SSH and No-folder Sessions",
            sel!(syncSshSessionsClicked:),
        );
        layout.place(&sync_ssh_sessions, NOTE_X, None, 4.0);
        let sync_now = push_button("Sync now", sel!(syncNowClicked:));
        let reopen_claude = push_button("Reopen Claude", sel!(reopenClaudeClicked:));
        layout.place_row(&[&sync_now, &reopen_claude], NOTE_X - 6.0, 2.0);
        let sidebar_sync_note = note("");
        // Room for a result that lists what was left out.
        layout.place(&sidebar_sync_note, NOTE_X, Some((NOTE_WIDTH, 44.0)), 4.0);

        layout.place(&heading(mtm, "Notifications"), 20.0, None, 6.0);
        let thresholds_label =
            NSTextField::labelWithString(ns_string!("Notify when a limit reaches (%):"), mtm);
        let thresholds = NSTextField::textFieldWithString(ns_string!(""), mtm);
        unsafe {
            thresholds.setTarget(Some(self));
            thresholds.setAction(Some(sel!(thresholdsChanged:)));
        }
        let label_width = thresholds_label.fittingSize().width.ceil();
        layout.place_beside(
            &thresholds_label,
            20.0,
            &thresholds,
            20.0 + label_width + 8.0,
            (80.0, 22.0),
            6.0,
        );
        let reset_notification = checkbox(
            "Also notify when a limit that warned resets",
            sel!(resetNotificationClicked:),
        );
        layout.place(&reset_notification, 20.0, None, 14.0);

        layout.place(&heading(mtm, "CLI switching"), 20.0, None, 6.0);
        let cli_switching = checkbox(
            "Clicking a Profile picks it for new claude runs",
            sel!(cliSwitchingClicked:),
        );
        layout.place(&cli_switching, 20.0, None, 4.0);
        let cli_switching_note = note("");
        cli_switching_note.setSelectable(true);
        layout.place(&cli_switching_note, NOTE_X, Some((NOTE_WIDTH, 84.0)), 14.0);

        layout.place(&heading(mtm, "General"), 20.0, None, 6.0);
        let launch_at_login = checkbox("Open AccRoster at login", sel!(launchAtLoginClicked:));
        layout.place(&launch_at_login, 20.0, None, 2.0);
        let launch_at_login_note = note("");
        layout.place(&launch_at_login_note, NOTE_X, Some((NOTE_WIDTH, 30.0)), 6.0);
        let verbose_logging = checkbox("Verbose logging", sel!(verboseLoggingClicked:));
        layout.place(&verbose_logging, 20.0, None, 2.0);
        let verbose_logging_note = note("");
        layout.place(
            &verbose_logging_note,
            NOTE_X,
            Some((NOTE_WIDTH, 16.0)),
            10.0,
        );
        let reveal = push_button("Reveal logs in Finder", sel!(revealLogsClicked:));
        let uninstall = push_button("Uninstall…", sel!(uninstallClicked:));
        let retry_uninstall = push_button("Retry", sel!(retryUninstallClicked:));
        layout.place_row(&[&reveal, &uninstall, &retry_uninstall], 14.0, 2.0);
        let uninstall_note = note("");
        layout.place(
            &uninstall_note,
            20.0,
            Some((SETTINGS_WIDTH - 40.0, 32.0)),
            0.0,
        );
        layout.finish();

        let hidden = push_button("Hidden (0)…", sel!(hiddenClicked:));
        // Placed in the Profiles section by `render_profile_settings`.
        let profiles = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, SETTINGS_HEIGHT),
                NSSize::new(SETTINGS_WIDTH, 0.0),
            ),
        );
        content.addSubview(&profiles);

        SettingsWindow {
            window,
            sidebar_sync,
            sidebar_sync_automatic,
            sync_now,
            reopen_claude,
            sidebar_sync_note,
            sync_ssh_sessions,
            cli_switching,
            cli_switching_note,
            launch_at_login,
            launch_at_login_note,
            verbose_logging,
            verbose_logging_note,
            hidden,
            profiles,
            thresholds,
            reset_notification,
            retry_uninstall,
            uninstall_note,
        }
    }

    /// Settings > Profiles: a heading, one line per Profile (its name, ↑ and
    /// ↓, Rename…, Mute) and Hidden (n)…. Rebuilt only when the Profiles
    /// changed; the window grows by the section's height.
    pub(super) fn render_profile_settings(
        &self,
        settings: &SettingsWindow,
        profiles: &[ProfileSettings],
    ) {
        if self.ivars().profile_settings.borrow().as_deref() == Some(profiles) {
            return;
        }
        *self.ivars().profile_settings.borrow_mut() = Some(profiles.to_vec());
        let mtm = self.mtm();
        let section = &settings.profiles;
        for view in section.subviews() {
            view.removeFromSuperview();
        }
        let mut commands = Vec::new();
        let height = 28.0 + PROFILE_LINE * profiles.len() as f64 + 36.0;
        section.setFrame(NSRect::new(
            NSPoint::new(0.0, SETTINGS_HEIGHT),
            NSSize::new(SETTINGS_WIDTH, height),
        ));
        let heading = NSTextField::labelWithString(ns_string!("Profiles"), mtm);
        heading.setFont(Some(
            &NSFont::boldSystemFontOfSize(NSFont::systemFontSize()),
        ));
        heading.setFrameOrigin(NSPoint::new(20.0, height - 24.0));
        heading.sizeToFit();
        section.addSubview(&heading);
        let mut button =
            |title: &str, command: Option<&Command>, x: f64, y: f64, check: Option<bool>| {
                let title = NSString::from_str(title);
                let button = unsafe {
                    match check {
                        Some(_) => NSButton::checkboxWithTitle_target_action(
                            &title,
                            Some(self),
                            Some(sel!(profileSettingClicked:)),
                            mtm,
                        ),
                        None => NSButton::buttonWithTitle_target_action(
                            &title,
                            Some(self),
                            Some(sel!(profileSettingClicked:)),
                            mtm,
                        ),
                    }
                };
                button.setControlSize(NSControlSize::Small);
                button.sizeToFit();
                button.setFrameOrigin(NSPoint::new(x, y));
                if let Some(on) = check {
                    button.setState(if on {
                        NSControlStateValueOn
                    } else {
                        NSControlStateValueOff
                    });
                }
                match command {
                    Some(command) => {
                        button.setTag(commands.len() as isize);
                        commands.push(command.clone());
                    }
                    None => button.setEnabled(false),
                }
                section.addSubview(&button);
            };
        for (index, profile) in profiles.iter().enumerate() {
            let y = height - 28.0 - PROFILE_LINE * (index + 1) as f64;
            let name = NSTextField::labelWithString(&NSString::from_str(&profile.title), mtm);
            // The middle goes first: the Organization at the end tells two
            // Profiles of one email apart.
            name.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
            name.setFrame(NSRect::new(
                NSPoint::new(20.0, y + 3.0),
                NSSize::new(280.0, 18.0),
            ));
            let tooltip = match &profile.detail {
                Some(detail) => format!("{}\n{detail}", profile.title),
                None => profile.title.clone(),
            };
            name.setToolTip(Some(&NSString::from_str(&tooltip)));
            section.addSubview(&name);
            button("↑", profile.move_up.as_ref(), 306.0, y, None);
            button("↓", profile.move_down.as_ref(), 338.0, y, None);
            button("Rename…", Some(&profile.rename), 370.0, y, None);
            if let Some(mute) = &profile.mute {
                button("Mute", Some(&mute.command), 456.0, y + 3.0, Some(mute.on));
            }
        }
        *self.ivars().profile_commands.borrow_mut() = commands;
        settings.hidden.setFrameOrigin(NSPoint::new(14.0, 4.0));
        section.addSubview(&settings.hidden);
        settings
            .window
            .setContentSize(NSSize::new(SETTINGS_WIDTH, SETTINGS_HEIGHT + height));
    }
}

/// The Settings window without its Profiles section, which sits on top and
/// grows the window by its own height.
pub(super) const SETTINGS_WIDTH: f64 = 540.0;
pub(super) const SETTINGS_HEIGHT: f64 = 536.0;
/// One line of Settings > Profiles.
pub(super) const PROFILE_LINE: f64 = 26.0;

impl<'a> Layout<'a> {
    fn new(content: &'a NSView) -> Layout<'a> {
        Layout { content, top: 6.0 }
    }

    /// Adds `view` at `x`, sized `size` or to fit, then leaves `gap`.
    fn place(&mut self, view: &NSView, x: f64, size: Option<(f64, f64)>, gap: f64) {
        let (width, height) = size.unwrap_or_else(|| {
            let fit = view.fittingSize();
            (fit.width.ceil(), fit.height.ceil())
        });
        self.put(view, x, width, height, height);
        self.top += height + gap;
    }

    /// Adds `label` and `field` on one line, centred on each other.
    fn place_beside(
        &mut self,
        label: &NSView,
        label_x: f64,
        field: &NSView,
        field_x: f64,
        (width, height): (f64, f64),
        gap: f64,
    ) {
        let fit = label.fittingSize();
        self.put(label, label_x, fit.width.ceil(), fit.height.ceil(), height);
        self.put(field, field_x, width, height, height);
        self.top += height + gap;
    }

    /// Adds buttons side by side from `x`.
    fn place_row(&mut self, views: &[&NSView], mut x: f64, gap: f64) {
        let height = views
            .iter()
            .map(|view| view.fittingSize().height.ceil())
            .fold(0.0, f64::max);
        for view in views {
            let fit = view.fittingSize();
            self.put(view, x, fit.width.ceil(), fit.height.ceil(), height);
            x += fit.width.ceil() + 4.0;
        }
        self.top += height + gap;
    }

    /// Frames `view` in a line `line` tall at the current top, centred.
    fn put(&self, view: &NSView, x: f64, width: f64, height: f64, line: f64) {
        let y = SETTINGS_HEIGHT - self.top - line + (line - height) / 2.0;
        view.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(width, height)));
        self.content.addSubview(view);
    }

    /// Checks everything fit in [`SETTINGS_HEIGHT`].
    fn finish(self) {
        debug_assert!(
            self.top <= SETTINGS_HEIGHT,
            "Settings needs {} points, has {SETTINGS_HEIGHT}",
            self.top
        );
    }
}

pub(super) fn render_settings(settings: &SettingsWindow, view: &SettingsView) {
    let uninstall = &view.uninstall;
    settings
        .retry_uninstall
        .setHidden(uninstall.retry.is_none());
    settings.uninstall_note.setStringValue(&NSString::from_str(
        uninstall.note.as_deref().unwrap_or_default(),
    ));
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
    settings.reopen_claude.setHidden(sync.reopen.is_none());
    settings
        .sync_ssh_sessions
        .setState(if sync.ssh_and_no_folder_sessions.on {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
    settings
        .sync_ssh_sessions
        .setEnabled(sync.mode != SidebarSyncMode::Off);
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
