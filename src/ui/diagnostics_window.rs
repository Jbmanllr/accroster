//! The Diagnostics window: the self-check's results and a Resume button
//! per paused feature.

use accroster_core::{CheckStatus, Command, DiagnosticsView, SafetyFeature};
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, sel};
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSFont, NSTextField, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, ns_string};

use super::Ui;

pub(super) struct DiagnosticsWindow {
    pub(super) window: Retained<NSWindow>,
    pub(super) text: Retained<NSTextField>,
    /// One Resume button per feature that can be paused, hidden when unused.
    pub(super) resume: Vec<Retained<NSButton>>,
}

impl Ui {
    /// The Diagnostics window: the checks and versions as selectable text,
    /// a Resume button per resumable pause, and Copy report.
    pub(super) fn make_diagnostics_window(&self) -> DiagnosticsWindow {
        let mtm = self.mtm();
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(860.0, 440.0));
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
            NSSize::new(820.0, 360.0),
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

        // Placed side by side as they are shown (`render_diagnostics`).
        let resume = SafetyFeature::ALL
            .iter()
            .map(|_| {
                let button = unsafe {
                    NSButton::buttonWithTitle_target_action(
                        ns_string!("Resume"),
                        Some(self),
                        Some(sel!(resumeClicked:)),
                        mtm,
                    )
                };
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

    pub(super) fn render_diagnostics(
        &self,
        diagnostics: &DiagnosticsWindow,
        view: &DiagnosticsView,
    ) {
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
                Some((format!("Resume {}", paused.feature.name()), command))
            })
            .collect();
        // Right of Copy report, side by side.
        let mut x = 140.0;
        for (index, button) in diagnostics.resume.iter().enumerate() {
            match resumable.get(index) {
                Some((title, _)) => {
                    button.setTitle(&NSString::from_str(title));
                    button.sizeToFit();
                    button.setFrameOrigin(NSPoint::new(x, 20.0));
                    x += button.frame().size.width + 8.0;
                    button.setTag(index as isize);
                    button.setHidden(false);
                }
                None => button.setHidden(true),
            }
        }
        *self.ivars().resume_commands.borrow_mut() =
            resumable.into_iter().map(|(_, command)| command).collect();
    }
}
