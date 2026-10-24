//! PROTOTYPE, throwaway. Answers wayfinder ticket "Prototype the menu bar panel":
//! three structurally different menu bar panels over fake data, switchable from the
//! panel's bottom row (or `--variant a|b|c`). Nothing is read from or written to disk;
//! every action is a stub that mutates in-memory state and prints it.
//!
//!   A  Native menu       NSMenu, custom-view Profile rows, click = switch CLI, hover = submenu
//!   B  Popover cards     NSPopover, one card per Profile, explicit CLI / Desktop buttons
//!   C  Popover by client NSPopover, CLI and Desktop slots first, compact table, Sessions inline

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::*;
use objc2_foundation::*;

// ---------------------------------------------------------------- fake data

struct Profile {
    nick: Option<&'static str>,
    email: &'static str,
    org: &'static str,
    plan: &'static str,
    signed_in: bool,
    session: f64,
    session_reset: &'static str,
    weekly: f64,
    weekly_reset: &'static str,
    model_weekly: Option<(&'static str, f64)>,
    note: Option<&'static str>,
}

impl Profile {
    fn name(&self) -> String {
        match self.nick {
            Some(n) => n.to_string(),
            None => format!("{} · {}", self.email, self.org),
        }
    }
    fn full(&self) -> String {
        format!("{} · {}", self.email, self.org)
    }
    /// The tightest limit: what the menu bar label shows.
    fn tightest(&self) -> (&'static str, f64) {
        let mut t = ("5h", self.session);
        if self.weekly > t.1 {
            t = ("7d", self.weekly);
        }
        if let Some((m, v)) = self.model_weekly {
            if v > t.1 {
                t = (m, v);
            }
        }
        t
    }
}

fn profiles() -> Vec<Profile> {
    vec![
        Profile {
            nick: Some("Perso"),
            email: "jb@gmail.com",
            org: "Personal",
            plan: "Max 20×",
            signed_in: true,
            session: 62.0,
            session_reset: "resets in 1 h 40 min",
            weekly: 41.0,
            weekly_reset: "resets Thu 09:00",
            model_weekly: None,
            note: None,
        },
        Profile {
            nick: Some("Work"),
            email: "jb@acme.io",
            org: "Acme Inc",
            plan: "Team",
            signed_in: true,
            session: 18.0,
            session_reset: "resets in 4 h 10 min",
            weekly: 77.0,
            weekly_reset: "resets Mon 14:00",
            model_weekly: Some(("Opus 7d", 92.0)),
            note: Some("Updated 12 min ago · rate-limited, retrying"),
        },
        Profile {
            nick: None,
            email: "jb@gmail.com",
            org: "Side Lab",
            plan: "Pro",
            signed_in: true,
            session: 0.0,
            session_reset: "starts on first use",
            weekly: 5.0,
            weekly_reset: "resets Sat 18:00",
            model_weekly: None,
            note: None,
        },
        Profile {
            nick: Some("Freelance"),
            email: "jb@studio.fr",
            org: "Studio",
            plan: "Pro",
            signed_in: false,
            session: 0.0,
            session_reset: "",
            weekly: 0.0,
            weekly_reset: "",
            model_weekly: None,
            note: None,
        },
    ]
}

/// (title, project, index of the Profile that created it, last activity)
const SESSIONS: [(&str, &str, usize, &str); 5] = [
    ("Fix Keychain prompt on rebuild", "claude-account-manager", 0, "4 min ago"),
    ("Q3 billing export", "acme-api", 1, "1 h ago"),
    ("Refactor auth middleware", "acme-api", 1, "yesterday"),
    ("Blog post draft", "notes", 2, "2 days ago"),
    ("Sidebar sync spike", "claude-account-manager", 0, "3 days ago"),
];

const VARIANTS: [&str; 3] = ["A: Native menu", "B: Popover cards", "C: Popover by client"];

// Action tags: kind * 100 + index.
const USE_CLI: isize = 100;
const SWITCH_DESKTOP: isize = 200;
const SIGN_IN: isize = 300;
const OPEN_IN_CLI: isize = 400;
const OPEN_IN_DESKTOP: isize = 500;
const ALL_SESSIONS: isize = 600;
const ADD_PROFILE: isize = 601;
const SETTINGS: isize = 602;
const REFRESH: isize = 603;
const PREV_VARIANT: isize = 700;
const NEXT_VARIANT: isize = 701;
const QUIT: isize = 800;
const TOGGLE_POPOVER: isize = 900;
const AUTO_OPEN: isize = 950;
const POPUP_CLI: isize = 1000;
const POPUP_DESKTOP: isize = 1100;
const SEARCH: isize = 1200;

struct State {
    profiles: Vec<Profile>,
    variant: usize,
    cli: usize,
    desktop: usize,
    status: Option<Retained<NSStatusItem>>,
    target: Option<Retained<Target>>,
    popover: Option<Retained<NSPopover>>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        profiles: profiles(),
        variant: 0,
        cli: 0,
        desktop: 1,
        status: None,
        target: None,
        popover: None,
    });
}

fn print_state(event: &str) {
    STATE.with(|s| {
        let s = s.borrow();
        println!(
            "[{event}] variant={} cli={} desktop={}",
            VARIANTS[s.variant],
            s.profiles[s.cli].name(),
            s.profiles[s.desktop].name()
        );
    });
}

// ---------------------------------------------------------------- ObjC classes

define_class!(
    // Single target for every control; dispatches on the sender's tag.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ProtoTarget"]
    struct Target;

    impl Target {
        #[unsafe(method(act:))]
        fn act(&self, sender: &AnyObject) {
            let tag: isize = unsafe { msg_send![sender, tag] };
            handle(tag, sender, self.mtm());
        }
    }
);

impl Target {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    // A menu row that draws its own hover highlight and fires on click,
    // which custom-view NSMenuItems do not do by themselves.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ProtoRowView"]
    struct RowView;

    impl RowView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            if let Some(item) = self.enclosingMenuItem() {
                if item.isHighlighted() {
                    let r = self.bounds();
                    let inset = NSRect::new(
                        NSPoint::new(r.origin.x + 5.0, r.origin.y + 1.0),
                        NSSize::new(r.size.width - 10.0, r.size.height - 2.0),
                    );
                    NSColor::controlAccentColor().colorWithAlphaComponent(0.18).set();
                    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(inset, 5.0, 5.0).fill();
                }
            }
        }

        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _p: NSPoint) -> *mut NSView {
            self as *const Self as *mut NSView
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _e: &NSEvent) {
            if let Some(item) = self.enclosingMenuItem() {
                if let Some(menu) = unsafe { item.menu() } {
                    menu.cancelTracking();
                }
                // Fire after menu tracking ends.
                STATE.with(|s| {
                    let s = s.borrow();
                    let t = s.target.as_ref().unwrap();
                    let _: () = unsafe {
                        msg_send![&**t, performSelector: sel!(act:), withObject: &*item, afterDelay: 0.0f64]
                    };
                });
            }
        }
    }
);

impl RowView {
    fn wrap(mtm: MainThreadMarker, content: &NSView, width: f64) -> Retained<Self> {
        content.setTranslatesAutoresizingMaskIntoConstraints(false);
        content.widthAnchor().constraintEqualToConstant(width).setActive(true);
        let h = content.fittingSize().height;
        let this = Self::alloc(mtm).set_ivars(());
        let this: Retained<Self> = unsafe {
            msg_send![super(this), initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, h))]
        };
        this.addSubview(content);
        pin(content, &this);
        this
    }
}

// ---------------------------------------------------------------- view helpers

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

fn pin(child: &NSView, parent: &NSView) {
    child.leadingAnchor().constraintEqualToAnchor(&parent.leadingAnchor()).setActive(true);
    child.trailingAnchor().constraintEqualToAnchor(&parent.trailingAnchor()).setActive(true);
    child.topAnchor().constraintEqualToAnchor(&parent.topAnchor()).setActive(true);
    child.bottomAnchor().constraintEqualToAnchor(&parent.bottomAnchor()).setActive(true);
}

enum Tone {
    Primary,
    Secondary,
    Warning,
}

fn label(mtm: MainThreadMarker, text: &str, size: f64, bold: bool, tone: Tone) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&ns(text), mtm);
    let font = if bold {
        NSFont::boldSystemFontOfSize(size)
    } else {
        NSFont::monospacedDigitSystemFontOfSize_weight(size, unsafe { NSFontWeightRegular })
    };
    l.setFont(Some(&font));
    match tone {
        Tone::Primary => {}
        Tone::Secondary => l.setTextColor(Some(&NSColor::secondaryLabelColor())),
        Tone::Warning => l.setTextColor(Some(&NSColor::systemOrangeColor())),
    }
    l
}

fn fixed_width(v: &NSView, w: f64) {
    v.setTranslatesAutoresizingMaskIntoConstraints(false);
    v.widthAnchor().constraintEqualToConstant(w).setActive(true);
}

fn badge(mtm: MainThreadMarker, text: &str, color: &NSColor) -> Retained<NSTextField> {
    let l = label(mtm, &format!(" {text} "), 10.0, true, Tone::Primary);
    l.setDrawsBackground(true);
    l.setBackgroundColor(Some(color));
    l.setTextColor(Some(&NSColor::whiteColor()));
    l
}

fn spacer(mtm: MainThreadMarker) -> Retained<NSView> {
    let v = NSView::new(mtm);
    v.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    v
}

fn stack(mtm: MainThreadMarker, vertical: bool, spacing: f64, views: &[&NSView]) -> Retained<NSStackView> {
    let s = NSStackView::new(mtm);
    s.setOrientation(if vertical {
        NSUserInterfaceLayoutOrientation::Vertical
    } else {
        NSUserInterfaceLayoutOrientation::Horizontal
    });
    s.setAlignment(if vertical { NSLayoutAttribute::Leading } else { NSLayoutAttribute::CenterY });
    s.setSpacing(spacing);
    for v in views {
        s.addArrangedSubview(v);
    }
    s
}

fn fill_width(s: &NSStackView) {
    // Make every arranged subview of a vertical stack as wide as the stack's content area.
    let i = s.edgeInsets();
    for v in s.arrangedSubviews().iter() {
        v.widthAnchor()
            .constraintEqualToAnchor_constant(&s.widthAnchor(), -(i.left + i.right))
            .setActive(true);
    }
}

fn button(mtm: MainThreadMarker, title: &str, tag: isize) -> Retained<NSButton> {
    let t = STATE.with(|s| s.borrow().target.clone().unwrap());
    let b = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), Some(&t), Some(sel!(act:)), mtm) };
    b.setTag(tag);
    b.setControlSize(NSControlSize::Small);
    b.setFont(Some(&NSFont::systemFontOfSize(11.0)));
    b
}

fn progress_bar(mtm: MainThreadMarker, pct: f64, width: f64) -> Retained<NSView> {
    let p = NSProgressIndicator::new(mtm);
    p.setIndeterminate(false);
    p.setStyle(NSProgressIndicatorStyle::Bar);
    p.setControlSize(NSControlSize::Small);
    p.setMinValue(0.0);
    p.setMaxValue(100.0);
    p.setDoubleValue(pct);
    fixed_width(&p, width);
    p.into_super()
}

fn level_bar(mtm: MainThreadMarker, pct: f64, width: f64) -> Retained<NSView> {
    let l = NSLevelIndicator::new(mtm);
    l.setLevelIndicatorStyle(NSLevelIndicatorStyle::ContinuousCapacity);
    l.setMinValue(0.0);
    l.setMaxValue(100.0);
    l.setWarningValue(70.0);
    l.setCriticalValue(90.0);
    l.setDoubleValue(pct);
    l.setControlSize(NSControlSize::Small);
    fixed_width(&l, width);
    l.heightAnchor().constraintEqualToConstant(10.0).setActive(true);
    l.into_super().into_super()
}

fn separator(mtm: MainThreadMarker) -> Retained<NSView> {
    let b = NSBox::new(mtm);
    b.setBoxType(NSBoxType::Separator);
    b.into_super()
}

fn card(mtm: MainThreadMarker, content: &NSView) -> Retained<NSView> {
    let b = NSBox::new(mtm);
    b.setBoxType(NSBoxType::Custom);
    b.setTitlePosition(NSTitlePosition::NoTitle);
    b.setCornerRadius(8.0);
    b.setBorderColor(&NSColor::separatorColor());
    b.setFillColor(&NSColor::controlBackgroundColor());
    b.setContentViewMargins(NSSize::new(10.0, 8.0));
    b.setContentView(Some(content));
    b.into_super()
}

fn switcher(mtm: MainThreadMarker, variant: usize) -> Retained<NSView> {
    let title = label(
        mtm,
        &format!("PROTOTYPE  {}  ({}/3)", VARIANTS[variant], variant + 1),
        10.0,
        true,
        Tone::Warning,
    );
    let row = stack(
        mtm,
        false,
        6.0,
        &[&button(mtm, "◀", PREV_VARIANT), &spacer(mtm), &title, &spacer(mtm), &button(mtm, "▶", NEXT_VARIANT), &button(mtm, "Quit", QUIT)],
    );
    row.into_super()
}

/// "5h ▓▓▓░░ 62% · resets in 1 h 40 min"
fn limit_line(mtm: MainThreadMarker, name: &str, pct: f64, detail: &str, level: bool, name_w: f64, bar_w: f64) -> Retained<NSView> {
    let n = label(mtm, name, 11.0, false, Tone::Secondary);
    fixed_width(&n, name_w);
    let bar = if level { level_bar(mtm, pct, bar_w) } else { progress_bar(mtm, pct, bar_w) };
    let v = label(mtm, &format!("{pct:.0}%"), 11.0, true, if pct >= 90.0 { Tone::Warning } else { Tone::Primary });
    fixed_width(&v, 34.0);
    let d = label(mtm, detail, 11.0, false, Tone::Secondary);
    stack(mtm, false, 6.0, &[&n, &bar, &v, &d]).into_super()
}

fn headroom_index(st: &State) -> usize {
    (0..st.profiles.len())
        .filter(|&i| st.profiles[i].signed_in)
        .min_by(|&a, &b| st.profiles[a].tightest().1.total_cmp(&st.profiles[b].tightest().1))
        .unwrap()
}

// ---------------------------------------------------------------- variant A: native menu

fn menu_item(mtm: MainThreadMarker, title: &str, tag: isize, key: &str) -> Retained<NSMenuItem> {
    let t = STATE.with(|s| s.borrow().target.clone().unwrap());
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &ns(title), Some(sel!(act:)), &ns(key))
    };
    unsafe { item.setTarget(Some(&t)) };
    item.setTag(tag);
    item
}

fn disabled_item(mtm: MainThreadMarker, title: &str) -> Retained<NSMenuItem> {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &ns(title), None, &ns(""))
    };
    item.setEnabled(false);
    item
}

fn build_menu(mtm: MainThreadMarker, st: &State) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    let headroom = headroom_index(st);
    menu.addItem(&disabled_item(
        mtm,
        &format!("CLI: {}    Desktop: {}", st.profiles[st.cli].name(), st.profiles[st.desktop].name()),
    ));
    menu.addItem(&NSMenuItem::separatorItem(mtm));

    for (i, p) in st.profiles.iter().enumerate() {
        let i = i as isize;
        if !p.signed_in {
            let item = menu_item(mtm, &format!("Sign in to {} ({})…", p.name(), p.full()), SIGN_IN + i, "");
            menu.addItem(&item);
            continue;
        }
        let mut head: Vec<Retained<NSView>> = vec![label(mtm, &p.name(), 13.0, true, Tone::Primary).into_super().into_super()];
        if st.cli == i as usize {
            head.push(badge(mtm, "CLI", &NSColor::systemBlueColor()).into_super().into_super());
        }
        if st.desktop == i as usize {
            head.push(badge(mtm, "Desktop", &NSColor::systemPurpleColor()).into_super().into_super());
        }
        if headroom == i as usize {
            head.push(label(mtm, "most headroom", 10.0, false, Tone::Secondary).into_super().into_super());
        }
        head.push(spacer(mtm));
        head.push(label(mtm, "›", 13.0, false, Tone::Secondary).into_super().into_super());
        let head_refs: Vec<&NSView> = head.iter().map(|v| &**v).collect();
        let head_row = stack(mtm, false, 5.0, &head_refs);

        let mut rows: Vec<Retained<NSView>> = vec![
            head_row.into_super(),
            limit_line(mtm, "5h", p.session, p.session_reset, true, 46.0, 120.0),
            limit_line(mtm, "7d", p.weekly, p.weekly_reset, true, 46.0, 120.0),
        ];
        if let Some((m, v)) = p.model_weekly {
            rows.push(limit_line(mtm, m, v, "", true, 46.0, 120.0));
        }
        if let Some(n) = p.note {
            rows.push(label(mtm, n, 10.0, false, Tone::Warning).into_super().into_super());
        }
        let row_refs: Vec<&NSView> = rows.iter().map(|v| &**v).collect();
        let body = stack(mtm, true, 3.0, &row_refs);
        body.setEdgeInsets(NSEdgeInsets { top: 5.0, left: 16.0, bottom: 5.0, right: 14.0 });
        fill_width(&body);

        let item = menu_item(mtm, &p.name(), USE_CLI + i, "");
        let row = RowView::wrap(mtm, &body, 370.0);
        item.setView(Some(&row));

        let sub = NSMenu::new(mtm);
        sub.setAutoenablesItems(false);
        let use_cli = menu_item(mtm, "Use for new CLI runs", USE_CLI + i, "");
        use_cli.setState(if st.cli == i as usize { NSControlStateValueOn } else { NSControlStateValueOff });
        sub.addItem(&use_cli);
        let desk = menu_item(mtm, "Switch desktop app to this Profile…", SWITCH_DESKTOP + i, "");
        desk.setState(if st.desktop == i as usize { NSControlStateValueOn } else { NSControlStateValueOff });
        sub.addItem(&desk);
        sub.addItem(&NSMenuItem::separatorItem(mtm));
        sub.addItem(&disabled_item(mtm, &format!("{} · {}", p.full(), p.plan)));
        sub.addItem(&menu_item(mtm, "Rename…", SETTINGS, ""));
        item.setSubmenu(Some(&sub));
        menu.addItem(&item);
    }

    menu.addItem(&NSMenuItem::separatorItem(mtm));
    let recent = menu_item(mtm, "Recent Sessions", 0, "");
    let rsub = NSMenu::new(mtm);
    rsub.setAutoenablesItems(false);
    for (i, (title, project, by, when)) in SESSIONS.iter().enumerate() {
        let i = i as isize;
        let it = disabled_item(mtm, &format!("{title}    {project} · {} · {when}", st.profiles[*by].name()));
        it.setEnabled(true);
        let s2 = NSMenu::new(mtm);
        s2.addItem(&menu_item(mtm, "Open in Terminal", OPEN_IN_CLI + i, ""));
        s2.addItem(&menu_item(mtm, "Open in Claude desktop", OPEN_IN_DESKTOP + i, ""));
        it.setSubmenu(Some(&s2));
        rsub.addItem(&it);
    }
    recent.setSubmenu(Some(&rsub));
    menu.addItem(&recent);
    menu.addItem(&menu_item(mtm, "All Sessions…", ALL_SESSIONS, "s"));
    menu.addItem(&menu_item(mtm, "Add Profile…", ADD_PROFILE, "n"));
    menu.addItem(&menu_item(mtm, "Settings…", SETTINGS, ","));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&menu_item(mtm, "PROTOTYPE  A: Native menu (1/3)   next variant ▶", NEXT_VARIANT, ""));
    menu.addItem(&menu_item(mtm, "Quit", QUIT, "q"));
    menu
}

// ---------------------------------------------------------------- variant B: popover cards

fn build_cards(mtm: MainThreadMarker, st: &State) -> Retained<NSView> {
    let headroom = headroom_index(st);
    let mut views: Vec<Retained<NSView>> = vec![stack(
        mtm,
        false,
        6.0,
        &[
            &label(mtm, "Claude Profiles", 13.0, true, Tone::Primary),
            &spacer(mtm),
            &label(mtm, "updated 1 min ago", 11.0, false, Tone::Secondary),
            &button(mtm, "↻", REFRESH),
        ],
    )
    .into_super()];

    for (i, p) in st.profiles.iter().enumerate() {
        let i = i as isize;
        let mut lines: Vec<Retained<NSView>> = Vec::new();
        let mut head: Vec<Retained<NSView>> = vec![label(mtm, &p.name(), 13.0, true, Tone::Primary).into_super().into_super()];
        if p.signed_in && st.cli == i as usize {
            head.push(badge(mtm, "CLI", &NSColor::systemBlueColor()).into_super().into_super());
        }
        if p.signed_in && st.desktop == i as usize {
            head.push(badge(mtm, "Desktop", &NSColor::systemPurpleColor()).into_super().into_super());
        }
        head.push(spacer(mtm));
        if p.signed_in && headroom == i as usize {
            head.push(label(mtm, "most headroom", 10.0, false, Tone::Secondary).into_super().into_super());
        }
        let head_refs: Vec<&NSView> = head.iter().map(|v| &**v).collect();
        lines.push(stack(mtm, false, 5.0, &head_refs).into_super());
        lines.push(label(mtm, &format!("{} · {}", p.full(), p.plan), 11.0, false, Tone::Secondary).into_super().into_super());

        if p.signed_in {
            lines.push(limit_line(mtm, "Session", p.session, p.session_reset, true, 52.0, 110.0));
            lines.push(limit_line(mtm, "Weekly", p.weekly, p.weekly_reset, true, 52.0, 110.0));
            if let Some((m, v)) = p.model_weekly {
                lines.push(limit_line(mtm, m, v, "", true, 52.0, 110.0));
            }
            if let Some(n) = p.note {
                lines.push(label(mtm, n, 10.0, false, Tone::Warning).into_super().into_super());
            }
            let cli = button(mtm, if st.cli == i as usize { "✓ Used by CLI" } else { "Use in CLI" }, USE_CLI + i);
            cli.setEnabled(st.cli != i as usize);
            let desk = button(mtm, if st.desktop == i as usize { "✓ In desktop app" } else { "Switch desktop…" }, SWITCH_DESKTOP + i);
            desk.setEnabled(st.desktop != i as usize);
            lines.push(stack(mtm, false, 6.0, &[&cli, &desk]).into_super());
        } else {
            lines.push(label(mtm, "Not signed in: limits unavailable", 11.0, false, Tone::Secondary).into_super().into_super());
            lines.push(button(mtm, "Sign in…", SIGN_IN + i).into_super().into_super());
        }
        let refs: Vec<&NSView> = lines.iter().map(|v| &**v).collect();
        let inner = stack(mtm, true, 4.0, &refs);
        if !p.signed_in {
            for v in inner.arrangedSubviews().iter().take(2) {
                v.setAlphaValue(0.5);
            }
        }
        views.push(card(mtm, &inner));
    }

    let search = NSSearchField::new(mtm);
    search.setPlaceholderString(Some(&ns("Search all Sessions…")));
    let t = STATE.with(|s| s.borrow().target.clone().unwrap());
    unsafe {
        search.setTarget(Some(&t));
        search.setAction(Some(sel!(act:)));
    }
    search.setTag(SEARCH);
    views.push(separator(mtm));
    views.push(search.into_super().into_super().into_super());
    views.push(
        stack(
            mtm,
            false,
            6.0,
            &[&button(mtm, "All Sessions…", ALL_SESSIONS), &button(mtm, "Add Profile…", ADD_PROFILE), &spacer(mtm), &button(mtm, "Settings…", SETTINGS)],
        )
        .into_super(),
    );
    views.push(separator(mtm));
    views.push(switcher(mtm, st.variant));
    finish_panel(mtm, views)
}

fn finish_panel(mtm: MainThreadMarker, views: Vec<Retained<NSView>>) -> Retained<NSView> {
    let refs: Vec<&NSView> = views.iter().map(|v| &**v).collect();
    let s = stack(mtm, true, 8.0, &refs);
    s.setEdgeInsets(NSEdgeInsets { top: 12.0, left: 12.0, bottom: 10.0, right: 12.0 });
    s.setTranslatesAutoresizingMaskIntoConstraints(false);
    s.widthAnchor().constraintEqualToConstant(360.0).setActive(true);
    for v in s.arrangedSubviews().iter() {
        v.widthAnchor().constraintEqualToConstant(336.0).setActive(true);
    }
    let size = s.fittingSize();
    s.setFrameSize(size);
    s.into_super()
}

// ---------------------------------------------------------------- variant C: popover by client

fn popup(mtm: MainThreadMarker, st: &State, selected: usize, tag: isize) -> Retained<NSPopUpButton> {
    let p = NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(200.0, 24.0)), false);
    for pr in st.profiles.iter().filter(|p| p.signed_in) {
        p.addItemWithTitle(&ns(&format!("{}  ({})", pr.name(), pr.full())));
    }
    p.selectItemAtIndex(selected as isize);
    let t = STATE.with(|s| s.borrow().target.clone().unwrap());
    unsafe {
        p.setTarget(Some(&t));
        p.setAction(Some(sel!(act:)));
    }
    p.setTag(tag);
    p.setControlSize(NSControlSize::Small);
    p
}

fn slot(mtm: MainThreadMarker, st: &State, client: &str, idx: usize, tag: isize, hint: &str) -> Retained<NSView> {
    let p = &st.profiles[idx];
    let (tname, tval) = p.tightest();
    let client_label = label(mtm, client, 12.0, true, Tone::Primary);
    fixed_width(&client_label, 56.0);
    let pop = popup(mtm, st, idx, tag);
    fixed_width(&pop, 240.0);
    let head = stack(
        mtm,
        false,
        8.0,
        &[&client_label, &pop],
    );
    let mut lines: Vec<Retained<NSView>> = vec![
        head.into_super(),
        limit_line(mtm, "Session", p.session, p.session_reset, true, 52.0, 110.0),
        limit_line(mtm, "Weekly", p.weekly, p.weekly_reset, true, 52.0, 110.0),
    ];
    if let Some((m, v)) = p.model_weekly {
        lines.push(limit_line(mtm, m, v, "", true, 52.0, 110.0));
    }
    if tval >= 90.0 {
        lines.push(label(mtm, &format!("{tname} at {tval:.0}%: consider switching"), 10.0, false, Tone::Warning).into_super().into_super());
    }
    lines.push(label(mtm, hint, 10.0, false, Tone::Secondary).into_super().into_super());
    let refs: Vec<&NSView> = lines.iter().map(|v| &**v).collect();
    card(mtm, &stack(mtm, true, 4.0, &refs))
}

fn build_by_client(mtm: MainThreadMarker, st: &State) -> Retained<NSView> {
    let headroom = headroom_index(st);
    let mut views: Vec<Retained<NSView>> = vec![
        slot(mtm, st, "CLI", st.cli, POPUP_CLI, "New `claude` runs use this Profile. Running Sessions keep theirs."),
        slot(mtm, st, "Desktop", st.desktop, POPUP_DESKTOP, "Changing it guides you through signing in again in Claude."),
        label(mtm, "All Profiles                                   5h                   7d", 11.0, true, Tone::Secondary).into_super().into_super(),
    ];
    for (i, p) in st.profiles.iter().enumerate() {
        let name = label(mtm, &p.name(), 12.0, false, if p.signed_in { Tone::Primary } else { Tone::Secondary });
        fixed_width(&name, 140.0);
        let row = if p.signed_in {
            let s = label(mtm, &format!("{:.0}%", p.session), 11.0, false, Tone::Primary);
            fixed_width(&s, 30.0);
            let w = label(mtm, &format!("{:.0}%", p.model_weekly.map_or(p.weekly, |(_, v)| v.max(p.weekly))), 11.0, false, Tone::Primary);
            fixed_width(&w, 30.0);
            let mark = label(mtm, if headroom == i { "◆" } else { "" }, 10.0, false, Tone::Secondary);
            stack(
                mtm,
                false,
                5.0,
                &[&name, &level_bar(mtm, p.session, 40.0), &s, &level_bar(mtm, p.model_weekly.map_or(p.weekly, |(_, v)| v.max(p.weekly)), 40.0), &w, &mark, &spacer(mtm)],
            )
        } else {
            stack(mtm, false, 5.0, &[&name, &label(mtm, "not signed in", 10.0, false, Tone::Secondary), &spacer(mtm), &button(mtm, "Sign in…", SIGN_IN + i as isize)])
        };
        views.push(row.into_super());
    }
    views.push(separator(mtm));
    views.push(stack(mtm, false, 6.0, &[&label(mtm, "Recent Sessions", 11.0, true, Tone::Secondary), &spacer(mtm), &button(mtm, "All Sessions…", ALL_SESSIONS)]).into_super());
    for (i, (title, project, by, when)) in SESSIONS.iter().enumerate() {
        let i = i as isize;
        let text = stack(
            mtm,
            true,
            1.0,
            &[&label(mtm, title, 12.0, false, Tone::Primary), &label(mtm, &format!("{project} · {} · {when}", st.profiles[*by].name()), 10.0, false, Tone::Secondary)],
        );
        views.push(stack(mtm, false, 4.0, &[&text, &spacer(mtm), &button(mtm, "Terminal", OPEN_IN_CLI + i), &button(mtm, "Desktop", OPEN_IN_DESKTOP + i)]).into_super());
    }
    views.push(separator(mtm));
    views.push(stack(mtm, false, 6.0, &[&button(mtm, "Add Profile…", ADD_PROFILE), &spacer(mtm), &button(mtm, "Settings…", SETTINGS)]).into_super());
    views.push(switcher(mtm, st.variant));
    finish_panel(mtm, views)
}

// ---------------------------------------------------------------- wiring

fn label_text(st: &State) -> String {
    let p = &st.profiles[st.cli];
    let (name, pct) = p.tightest();
    match st.variant {
        0 => format!("{} {pct:.0}%", p.name()),
        1 => format!("✳ {} · {name} {pct:.0}%", p.name()),
        _ => format!("{pct:.0}% · {:.0}%", st.profiles[st.desktop].tightest().1),
    }
}

fn panel_view(mtm: MainThreadMarker, st: &State) -> Retained<NSView> {
    if st.variant == 1 {
        build_cards(mtm, st)
    } else {
        build_by_client(mtm, st)
    }
}

fn rebuild(mtm: MainThreadMarker) {
    STATE.with(|s| {
        let st = s.borrow();
        let status = st.status.as_ref().unwrap();
        let button = status.button(mtm).unwrap();
        button.setTitle(&ns(&label_text(&st)));
        if st.variant == 0 {
            status.setMenu(Some(&build_menu(mtm, &st)));
        } else {
            status.setMenu(None);
            unsafe {
                button.setTarget(Some(st.target.as_ref().unwrap()));
                button.setAction(Some(sel!(act:)));
            }
            button.setTag(TOGGLE_POPOVER);
            let pop = st.popover.as_ref().unwrap();
            if pop.isShown() {
                let v = panel_view(mtm, &st);
                pop.setContentSize(v.frame().size);
                pop.contentViewController().unwrap().setView(&v);
            }
        }
    });
}

fn show_popover(mtm: MainThreadMarker) {
    STATE.with(|s| {
        let st = s.borrow();
        let pop = st.popover.as_ref().unwrap();
        let button = st.status.as_ref().unwrap().button(mtm).unwrap();
        let v = panel_view(mtm, &st);
        let vc = NSViewController::new(mtm);
        vc.setView(&v);
        pop.setContentViewController(Some(&vc));
        pop.setContentSize(v.frame().size);
        NSApplication::sharedApplication(mtm).activate();
        pop.showRelativeToRect_ofView_preferredEdge(button.bounds(), &button, NSRectEdge::MinY);
    });
}

fn alert(mtm: MainThreadMarker, title: &str, info: &str) {
    println!("  stub: {title}: {info}");
    let a = NSAlert::new(mtm);
    a.setMessageText(&ns(title));
    a.setInformativeText(&ns(info));
    NSApplication::sharedApplication(mtm).activate();
    a.runModal();
}

fn signed_in_index(n: isize) -> usize {
    STATE.with(|s| {
        let st = s.borrow();
        (0..st.profiles.len()).filter(|&i| st.profiles[i].signed_in).nth(n as usize).unwrap()
    })
}

fn handle(tag: isize, sender: &AnyObject, mtm: MainThreadMarker) {
    let name = |i: usize| STATE.with(|s| {
        let st = s.borrow();
        format!("{} ({})", st.profiles[i].name(), st.profiles[i].full())
    });
    match tag {
        TOGGLE_POPOVER => {
            let shown = STATE.with(|s| s.borrow().popover.as_ref().unwrap().isShown());
            if shown {
                STATE.with(|s| unsafe { s.borrow().popover.as_ref().unwrap().performClose(None) });
            } else {
                show_popover(mtm);
            }
            return;
        }
        AUTO_OPEN => {
            let variant = STATE.with(|s| s.borrow().variant);
            if variant == 0 {
                let b = STATE.with(|s| s.borrow().status.as_ref().unwrap().button(mtm).unwrap());
                unsafe { b.performClick(None) };
            } else {
                show_popover(mtm);
            }
            return;
        }
        PREV_VARIANT | NEXT_VARIANT => {
            let reopen = STATE.with(|s| {
                let mut st = s.borrow_mut();
                st.variant = (st.variant + if tag == NEXT_VARIANT { 1 } else { 2 }) % 3;
                let pop = st.popover.as_ref().unwrap();
                if pop.isShown() {
                    pop.close();
                }
                st.variant != 0
            });
            rebuild(mtm);
            print_state("variant");
            if reopen {
                show_popover(mtm);
            }
            return;
        }
        QUIT => NSApplication::sharedApplication(mtm).terminate(None),
        REFRESH => alert(mtm, "Refresh", "Would re-poll every signed-in Profile's limits now (with backoff)."),
        ALL_SESSIONS => alert(mtm, "Unified session view", "Would open a separate window listing every Session on the Mac, searchable, grouped by project."),
        ADD_PROFILE => alert(mtm, "Add Profile", "Would create a fresh config dir, run `claude auth login` and open the browser sign-in."),
        SETTINGS => alert(mtm, "Settings", "Would open settings: nicknames, thresholds, launch at login."),
        SEARCH => {
            let q: Retained<NSString> = unsafe { msg_send![sender, stringValue] };
            alert(mtm, "Unified session view", &format!("Would open the Session window filtered by \"{q}\"."));
        }
        POPUP_CLI => {
            let n: isize = unsafe { msg_send![sender, indexOfSelectedItem] };
            let i = signed_in_index(n);
            STATE.with(|s| s.borrow_mut().cli = i);
        }
        POPUP_DESKTOP => {
            let n: isize = unsafe { msg_send![sender, indexOfSelectedItem] };
            let i = signed_in_index(n);
            alert(mtm, "Guided desktop switch", &format!("Claude will come forward. Sign out, then sign in as {}.", name(i)));
            STATE.with(|s| s.borrow_mut().desktop = i);
        }
        t if (USE_CLI..USE_CLI + 100).contains(&t) => {
            STATE.with(|s| s.borrow_mut().cli = (t - USE_CLI) as usize);
        }
        t if (SWITCH_DESKTOP..SWITCH_DESKTOP + 100).contains(&t) => {
            let i = (t - SWITCH_DESKTOP) as usize;
            alert(mtm, "Guided desktop switch", &format!("Claude will come forward. Sign out, then sign in as {}.", name(i)));
            STATE.with(|s| s.borrow_mut().desktop = i);
        }
        t if (SIGN_IN..SIGN_IN + 100).contains(&t) => {
            let i = (t - SIGN_IN) as usize;
            alert(mtm, "Sign in", &format!("Would run `claude auth login` for {} and open the browser.", name(i)));
        }
        t if (OPEN_IN_CLI..OPEN_IN_CLI + 100).contains(&t) => {
            alert(mtm, "Open in Terminal", &format!("Would run `claude --resume <id>` for \"{}\" in Terminal.", SESSIONS[(t - OPEN_IN_CLI) as usize].0));
        }
        t if (OPEN_IN_DESKTOP..OPEN_IN_DESKTOP + 100).contains(&t) => {
            alert(mtm, "Open in Claude desktop", &format!("Would open \"{}\" via claude://resume, filed under the desktop's Active Profile.", SESSIONS[(t - OPEN_IN_DESKTOP) as usize].0));
        }
        _ => {}
    }
    rebuild(mtm);
    print_state(&format!("tag {tag}"));
}

fn main() {
    let mtm = MainThreadMarker::new().unwrap();
    let args: Vec<String> = std::env::args().collect();
    let variant = match args.iter().position(|a| a == "--variant").and_then(|i| args.get(i + 1)).map(|s| s.as_str()) {
        Some("b") => 1,
        Some("c") => 2,
        _ => 0,
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let status = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    let target = Target::new(mtm);
    let popover = NSPopover::new(mtm);
    popover.setBehavior(NSPopoverBehavior::Transient);
    STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.variant = variant;
        st.status = Some(status);
        st.target = Some(target.clone());
        st.popover = Some(popover);
    });
    rebuild(mtm);
    print_state("start");

    if args.iter().any(|a| a == "--open") {
        // Open the panel shortly after launch, for screenshots.
        let trigger = disabled_item(mtm, "");
        trigger.setTag(AUTO_OPEN);
        let _: () = unsafe { msg_send![&*target, performSelector: sel!(act:), withObject: &*trigger, afterDelay: 0.8f64] };
        std::mem::forget(trigger);
    }
    app.run();
}
