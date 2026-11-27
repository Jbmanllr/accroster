//! The views the windows and the menu are made of: each is built from one
//! of the core's view models and knows nothing of the [`super::Ui`].

use accroster_core::{
    Badge, Command, DiscoveredRow, LimitBar, MOST_HEADROOM, MenuItem, ProfileRow, RunningDot,
    SessionRow, StatusTone,
};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject};
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSBezierPath, NSBox, NSBoxType, NSColor,
    NSControlSize, NSEvent, NSFont, NSFontAttributeName, NSFontWeightBold, NSFontWeightRegular,
    NSForegroundColorAttributeName, NSImage, NSLayoutAttribute, NSLayoutConstraintOrientation,
    NSLevelIndicator, NSLevelIndicatorStyle, NSLineBreakMode, NSMenuItem, NSResponder, NSStackView,
    NSTextField, NSTitlePosition, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{
    NSAttributedString, NSAttributedStringKey, NSDictionary, NSEdgeInsets, NSPoint, NSRect, NSSize,
    NSString, ns_string,
};

/// The least width of a Profile row in the menu; rows widen to fit.
pub(super) const ROW_WIDTH: f64 = 300.0;

/// A bold group heading, like Settings > Profiles.
pub(super) fn heading(mtm: MainThreadMarker, text: &str) -> Retained<NSTextField> {
    let heading = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    heading.setFont(Some(
        &NSFont::boldSystemFontOfSize(NSFont::systemFontSize()),
    ));
    heading
}

/// A thin horizontal rule `width` wide, between detail pane sections.
pub(super) fn rule(mtm: MainThreadMarker, width: f64) -> Retained<NSBox> {
    let rule = NSBox::new(mtm);
    rule.setBoxType(NSBoxType::Separator);
    fixed_width(&rule, width);
    rule
}

define_class!(
    /// A custom-view menu row. Custom views get no highlight from NSMenu, so
    /// it draws its own when its item is highlighted (hovered).
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AccRosterRowView"]
    pub(super) struct RowView;

    impl RowView {
        /// A custom view gets the click instead of the menu: close the menu,
        /// then send the item's action as a plain item would. Only the
        /// rows' own action is sent: a row with a submenu and no command
        /// reports AppKit's `submenuAction:`, aimed at the submenu, which
        /// NSMenu doesn't answer through `sendAction:` (it throws).
        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            let Some(item) = self.enclosingMenuItem() else {
                return;
            };
            let action = sel!(menuItemClicked:);
            if item.action() != Some(action) {
                return;
            }
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
    /// A row `width` wide holding `content`, as tall as it needs at that
    /// width; it widens with the menu when a plain item is wider.
    pub(super) fn wrap(mtm: MainThreadMarker, content: &NSView, width: f64) -> Retained<RowView> {
        content.setTranslatesAutoresizingMaskIntoConstraints(false);
        let sizing = content.widthAnchor().constraintEqualToConstant(width);
        sizing.setActive(true);
        let height = content.fittingSize().height;
        sizing.setActive(false);
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height));
        let this = RowView::alloc(mtm).set_ivars(());
        let this: Retained<RowView> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
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

/// The one width every custom row in `items` gets, so none is clipped: the
/// widest row's, with room on each Profile row for the most-headroom marker
/// and the "default login" note, which can appear while the menu is open
/// (an open menu keeps its width).
pub(super) fn rows_width(mtm: MainThreadMarker, items: &[MenuItem]) -> f64 {
    items
        .iter()
        .filter_map(|item| match item {
            MenuItem::Profile(profile) => {
                let widest = ProfileRow {
                    marker: Some(MOST_HEADROOM.to_owned()),
                    default_login: true,
                    ..profile.clone()
                };
                Some(profile_row_content(mtm, &widest))
            }
            MenuItem::Discovered(discovered) => Some(discovered_row_content(mtm, discovered)),
            _ => None,
        })
        .map(|content| content.fittingSize().width.ceil())
        .fold(ROW_WIDTH, f64::max)
}

/// A Profile row: the name and a submenu arrow, one bar per limit, the Banked
/// reset / Session refill lines, the note.
pub(super) fn profile_row_content(mtm: MainThreadMarker, row: &ProfileRow) -> Retained<NSView> {
    let name = label(mtm, &row.title, NSFont::boldSystemFontOfSize(13.0), false);
    let mut badges: Vec<Retained<NSView>> =
        row.badges.iter().map(|badge| pill(mtm, *badge)).collect();
    if row.default_login {
        let note = label(mtm, "default login", NSFont::systemFontOfSize(10.0), true);
        badges.push(note.into_super().into_super());
    }
    if let Some(marker) = &row.marker {
        let marker = label(mtm, marker, NSFont::systemFontOfSize(10.0), true);
        badges.push(marker.into_super().into_super());
    }
    let filler = NSView::new(mtm);
    filler.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    let arrow = label(mtm, "›", NSFont::systemFontOfSize(13.0), true);
    let mut name_line: Vec<&NSView> = vec![&name];
    name_line.extend(badges.iter().map(|badge| &**badge));
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

/// A Discovered Profile row: greyed `Sign in to …`, a submenu arrow, and the
/// Unknown Login line.
pub(super) fn discovered_row_content(
    mtm: MainThreadMarker,
    row: &DiscoveredRow,
) -> Retained<NSView> {
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

/// `Session ▓▓▓░░ 62%`: an `NSLevelIndicator`, continuous capacity, warning at
/// 70 and critical at 90.
/// An All Sessions row: the running dot (green for Terminal, orange for a
/// desktop turn, invisible otherwise), the title, and the subtitle.
pub(super) fn session_row_view(mtm: MainThreadMarker, row: &SessionRow) -> Retained<NSView> {
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

pub(super) fn limit_line(mtm: MainThreadMarker, bar: &LimitBar) -> Retained<NSView> {
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
    let bold = unsafe { NSFont::monospacedDigitSystemFontOfSize_weight(11.0, NSFontWeightBold) };
    let percent = label(mtm, &text, bold, false);
    fixed_width(&percent, 36.0);
    let resets = label(mtm, bar.resets.as_deref().unwrap_or(""), regular(), true);
    stack(mtm, false, 6.0, &[&name, &level, &percent, &resets]).into_super()
}

/// A client badge: a coloured pill (CLI blue, Desktop purple) with white
/// text, like the prototype's.
pub(super) fn pill(mtm: MainThreadMarker, badge: Badge) -> Retained<NSView> {
    let text = label(
        mtm,
        badge.title(),
        NSFont::boldSystemFontOfSize(10.0),
        false,
    );
    text.setTextColor(Some(&NSColor::whiteColor()));
    let pill = NSBox::new(mtm);
    pill.setBoxType(NSBoxType::Custom);
    pill.setTitlePosition(NSTitlePosition::NoTitle);
    pill.setBorderWidth(0.0);
    pill.setCornerRadius(4.0);
    pill.setFillColor(&*match badge {
        Badge::Cli => NSColor::systemBlueColor(),
        Badge::Desktop => NSColor::systemPurpleColor(),
    });
    pill.setContentViewMargins(NSSize::new(4.0, 1.0));
    let size = text.fittingSize();
    pill.setContentView(Some(&text));
    // A box has no size of its own: as wide and tall as its text.
    fixed_width(&pill, size.width.ceil() + 8.0);
    pill.heightAnchor()
        .constraintEqualToConstant(size.height.ceil() + 2.0)
        .setActive(true);
    pill.into_super()
}

/// The menu's key equivalents (with ⌘), as in the prototype.
pub(super) fn key_equivalent(command: &Command) -> Option<&'static str> {
    match command {
        Command::OpenAllSessions => Some("s"),
        Command::AddProfile => Some("n"),
        Command::OpenSettings => Some(","),
        Command::Quit => Some("q"),
        _ => None,
    }
}

/// The menu bar's icon when there is no Profile to show: the app icon's
/// three rows (a dot, then a bar partly filled) as a template image, so
/// macOS tints it for the menu bar.
pub(super) fn menu_bar_icon() -> Retained<NSImage> {
    let draw = RcBlock::new(|_rect: NSRect| -> Bool {
        // In a flipped 18 × 16 canvas, top row first.
        for (index, filled) in [0.74, 0.36, 0.9].into_iter().enumerate() {
            let y = 2.5 + 5.0 * index as f64;
            let dot = NSRect::new(NSPoint::new(1.5, y), NSSize::new(3.0, 3.0));
            NSColor::blackColor().set();
            NSBezierPath::bezierPathWithOvalInRect(dot).fill();
            let track = NSRect::new(NSPoint::new(6.0, y + 0.25), NSSize::new(11.0, 2.5));
            NSColor::blackColor().colorWithAlphaComponent(0.35).set();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(track, 1.25, 1.25).fill();
            let fill = NSRect::new(track.origin, NSSize::new(11.0 * filled, 2.5));
            NSColor::blackColor().set();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(fill, 1.25, 1.25).fill();
        }
        Bool::YES
    });
    let image = NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(18.0, 16.0), true, &draw);
    image.setTemplate(true);
    image.setAccessibilityDescription(Some(ns_string!("AccRoster")));
    image
}

pub(super) fn label(
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

pub(super) fn fixed_width(view: &NSView, width: f64) {
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    view.widthAnchor()
        .constraintEqualToConstant(width)
        .setActive(true);
}

pub(super) fn stack(
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

/// The status line at the top of the menu: disabled, with a sign and a
/// colour by tone. A line that waits on the user is bold orange, so the
/// choice under it can't be missed.
pub(super) fn status_item(
    mtm: MainThreadMarker,
    text: &str,
    tone: StatusTone,
) -> Retained<NSMenuItem> {
    let (sign, font, color) = match tone {
        StatusTone::Working => (
            "⟳",
            NSFont::menuFontOfSize(0.0),
            NSColor::secondaryLabelColor(),
        ),
        StatusTone::NeedsYou => (
            "⚠︎",
            NSFont::boldSystemFontOfSize(NSFont::systemFontSize()),
            NSColor::systemOrangeColor(),
        ),
        StatusTone::Done => ("✓", NSFont::menuFontOfSize(0.0), NSColor::labelColor()),
        StatusTone::Failed => (
            "✕",
            NSFont::boldSystemFontOfSize(NSFont::systemFontSize()),
            NSColor::systemRedColor(),
        ),
    };
    let title = format!("{sign} {text}");
    let item = disabled_item(mtm, &title);
    let attributes = unsafe {
        NSDictionary::<NSAttributedStringKey, AnyObject>::from_slices(
            &[NSFontAttributeName, NSForegroundColorAttributeName],
            &[&*font, &*color],
        )
    };
    let attributed = unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(&title),
            Some(&attributes),
        )
    };
    item.setAttributedTitle(Some(&attributed));
    item
}

pub(super) fn disabled_item(mtm: MainThreadMarker, title: &str) -> Retained<NSMenuItem> {
    let item = menu_item(mtm, title);
    item.setEnabled(false);
    item
}

pub(super) fn menu_item(mtm: MainThreadMarker, title: &str) -> Retained<NSMenuItem> {
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            None,
            ns_string!(""),
        )
    }
}
