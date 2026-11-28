//! The All Sessions window: the sidebar of projects and Profiles, the
//! table of Sessions and the selected Session's detail pane.

use accroster_core::{AllSessionsView, RunningDot, SessionDetail};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadOnly, sel};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSButton, NSColor, NSFont, NSOutlineView,
    NSScrollView, NSSearchField, NSStackView, NSTableColumn, NSTableView, NSTableViewStyle,
    NSTextField, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSEdgeInsets, NSIndexSet, NSPoint, NSRect, NSSize, NSString, ns_string};

use super::Ui;
use super::sidebar::SidebarSource;
use super::widgets::{fixed_width, rule, stack};

pub(super) struct AllSessionsWindow {
    pub(super) window: Retained<NSWindow>,
    pub(super) search: Retained<NSSearchField>,
    pub(super) status: Retained<NSTextField>,
    pub(super) sidebar: Retained<NSOutlineView>,
    pub(super) sidebar_source: Retained<SidebarSource>,
    pub(super) table: Retained<NSTableView>,
    pub(super) detail: Retained<NSStackView>,
}

impl Ui {
    /// Refreshes the All Sessions list. The search field is the user's; it is
    /// only filled from the view while it isn't being edited.
    pub(super) fn render_all_sessions(&self, window: &AllSessionsWindow, view: &AllSessionsView) {
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
            self.render_session_detail(&window.detail, view.detail.as_ref());
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

    /// A resizable three-pane window under a status line and the search
    /// field: the sidebar (`NSOutlineView`), the Session table, the detail
    /// pane.
    pub(super) fn make_all_sessions_window(&self) -> AllSessionsWindow {
        let mtm = self.mtm();
        let (width, height) = (1040.0, 600.0);
        let (sidebar_width, detail_width, top) = (240.0, 320.0, 50.0);
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
        // The column as wide as what the scroll view shows, so long lines
        // end in "…" instead of running under its edge.
        sidebar.sizeLastColumnToFit();

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
            table.setTarget(Some(self));
            table.setDoubleAction(Some(sel!(sessionDoubleClicked:)));
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
        self.render_session_detail(&detail, None);

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

    /// Refills the detail pane: title, folder and branch, last client and age,
    /// turns, first prompt, the running state, one ✓/– line per Profile's
    /// Sidebar, and why nothing opens when the Transcript is gone.
    pub(super) fn render_session_detail(&self, pane: &NSStackView, detail: Option<&SessionDetail>) {
        const WIDTH: f64 = 290.0;
        let mtm = self.mtm();
        self.ivars().open_commands.borrow_mut().clear();
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
        pane.addArrangedSubview(&rule(mtm, WIDTH));
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
        if detail.open.is_empty() {
            return;
        }
        pane.addArrangedSubview(&rule(mtm, WIDTH));
        pane.addArrangedSubview(&wrapping(
            "Open",
            NSFont::boldSystemFontOfSize(11.0),
            NSColor::secondaryLabelColor(),
        ));
        let mut commands = self.ivars().open_commands.borrow_mut();
        for choice in &detail.open {
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(&choice.title),
                    Some(self),
                    Some(sel!(openChoiceClicked:)),
                    mtm,
                )
            };
            button.setEnabled(choice.enabled);
            button.setTag(commands.len() as isize);
            commands.push(choice.command.clone());
            pane.addArrangedSubview(&button);
            if let Some(note) = &choice.note {
                pane.addArrangedSubview(&wrapping(note, small(), NSColor::secondaryLabelColor()));
            }
        }
    }
}
