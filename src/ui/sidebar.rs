//! The All Sessions sidebar: its outline view's data source and delegate.

use super::UI;
use super::widgets::label;
use accroster_core::{Command, SessionFilter, SidebarGroup};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSControlTextEditingDelegate, NSFont, NSLayoutConstraintOrientation, NSLineBreakMode,
    NSOutlineView, NSOutlineViewDataSource, NSOutlineViewDelegate, NSTableColumn, NSView,
};
use objc2_foundation::{
    NSIndexSet, NSInteger, NSNotification, NSNumber, NSObjectProtocol, NSString,
};
use std::cell::RefCell;

/// One line of the All Sessions sidebar.
pub(super) enum SidebarLine {
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
    pub(super) struct SidebarSource;

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
    pub(super) fn new(mtm: MainThreadMarker) -> Retained<SidebarSource> {
        let this = SidebarSource::alloc(mtm).set_ivars(SidebarIvars {
            lines: RefCell::new(Vec::new()),
            items: RefCell::new(Vec::new()),
            shown: RefCell::new(Vec::new()),
        });
        unsafe { msg_send![super(this), init] }
    }

    pub(super) fn line_view(&self, item: &AnyObject) -> Option<Retained<NSView>> {
        let mtm = MainThreadMarker::from(self);
        let lines = self.ivars().lines.borrow();
        let view = match lines.get(line_index(item)?)? {
            SidebarLine::Heading(title) => {
                label(mtm, title, NSFont::boldSystemFontOfSize(11.0), true)
            }
            SidebarLine::Item(title, _) => {
                let line = label(mtm, title, NSFont::systemFontOfSize(12.0), false);
                line.setToolTip(Some(&NSString::from_str(title)));
                line
            }
        };
        view.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        view.setContentCompressionResistancePriority_forOrientation(
            1.0,
            NSLayoutConstraintOrientation::Horizontal,
        );
        Some(view.into_super().into_super())
    }

    pub(super) fn is_heading(&self, item: &AnyObject) -> bool {
        line_index(item).is_some_and(|index| {
            matches!(
                self.ivars().lines.borrow().get(index),
                Some(SidebarLine::Heading(_))
            )
        })
    }

    /// Redraws the groups when they changed and selects `filter`'s item.
    pub(super) fn render(
        &self,
        outline: &NSOutlineView,
        groups: &[SidebarGroup],
        filter: &SessionFilter,
    ) {
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
pub(super) fn line_index(item: &AnyObject) -> Option<usize> {
    let number = item.downcast_ref::<NSNumber>()?;
    usize::try_from(number.as_isize()).ok()
}
