//! The first-run window, which offers Launch at login once.

use accroster_core::View;
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, sel};
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSControlStateValueOff, NSControlStateValueOn, NSTextField,
    NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, ns_string};

use super::Ui;
use super::mac::show_in_front;

pub(super) struct FirstRunWindow {
    pub(super) window: Retained<NSWindow>,
    pub(super) launch_at_login: Retained<NSButton>,
}

impl Ui {
    /// Opens the first-run window when the view has an offer, closes it once
    /// the view no longer does.
    pub(super) fn render_first_run(&self, view: &View) {
        let mut open = self.ivars().first_run.borrow_mut();
        match (&view.first_run, open.is_some()) {
            (Some(offer), false) => {
                let first_run = self.make_first_run_window(offer.launch_at_login);
                first_run.window.center();
                show_in_front(&first_run.window);
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

    /// A small window with the launch-at-login checkbox and Done. It has no
    /// close button, so the offer is always answered.
    pub(super) fn make_first_run_window(&self, launch_at_login: bool) -> FirstRunWindow {
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
}
