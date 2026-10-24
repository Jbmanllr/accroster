//! PROTOTYPE, throwaway. Answers wayfinder ticket "Prototype the Unified session view window":
//! three structurally different Session windows over fake data, switchable from the bar at the
//! bottom of the window (or `--variant a|b|c`). Nothing is read from or written to disk; every
//! action is a stub that shows what it would do and prints the in-memory state.
//!
//!   A  Table      one dense list, search + Group by, per-row Terminal / Claude buttons
//!   B  Browser    sidebar (Running, Projects, Profiles) | list | detail pane with explicit open choices
//!   C  Launcher   Spotlight-like: one search field, flat recency list, keyboard first (↩ / ⌘↩)

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::*;
use objc2_foundation::*;

// ---------------------------------------------------------------- fake data

struct Profile {
    nick: &'static str,
    full: &'static str,
    signed_in: bool,
}

const PROFILES: [Profile; 4] = [
    Profile { nick: "Perso", full: "jb@gmail.com · Personal", signed_in: true },
    Profile { nick: "Work", full: "jb@acme.io · Acme Inc", signed_in: true },
    Profile { nick: "jb@gmail.com · Side Lab", full: "jb@gmail.com · Side Lab", signed_in: true },
    Profile { nick: "Freelance", full: "jb@studio.fr · Studio", signed_in: false },
];

#[derive(Clone, Copy, PartialEq)]
enum Client {
    Cli,
    Desktop,
}

#[derive(Clone, Copy, PartialEq)]
enum Running {
    No,
    Terminal,
    DesktopBusy,
}

struct Session {
    /// Sidebar title, else transcript `custom-title`, else None (fall back to the first prompt).
    title: Option<&'static str>,
    first_prompt: &'static str,
    project: &'static str,
    branch: &'static str,
    age_min: u32,
    /// Client the Session was last used in (transcript `entrypoint` of the last turn).
    last_client: Client,
    /// Profiles whose sidebar holds a Sidebar entry for it (indices into PROFILES).
    entries: &'static [usize],
    /// Archived in every Profile that has an entry.
    archived: bool,
    transcript: bool,
    running: Running,
    turns: u32,
}

const SESSIONS: [Session; 13] = [
    Session { title: Some("Fix Keychain prompt on rebuild"), first_prompt: "every cargo build asks for the keychain password again", project: "claude-account-manager", branch: "main", age_min: 4, last_client: Client::Cli, entries: &[], archived: false, transcript: true, running: Running::Terminal, turns: 38 },
    Session { title: Some("Investigate flaky checkout test"), first_prompt: "checkout.spec.ts fails 1 in 5 on CI", project: "acme-web", branch: "fix/flaky-checkout", age_min: 20, last_client: Client::Desktop, entries: &[1], archived: false, transcript: true, running: Running::DesktopBusy, turns: 12 },
    Session { title: Some("Q3 billing export"), first_prompt: "add a CSV export of invoices per quarter", project: "acme-api", branch: "feat/billing-export", age_min: 65, last_client: Client::Desktop, entries: &[1], archived: false, transcript: true, running: Running::No, turns: 54 },
    Session { title: None, first_prompt: "why does cargo build take 40s after touching one file", project: "rust-playground", branch: "main", age_min: 300, last_client: Client::Cli, entries: &[], archived: false, transcript: true, running: Running::No, turns: 6 },
    Session { title: Some("Refactor auth middleware"), first_prompt: "split the auth middleware into session and token parts", project: "acme-api", branch: "refactor/auth", age_min: 1500, last_client: Client::Desktop, entries: &[1, 0], archived: false, transcript: true, running: Running::No, turns: 91 },
    Session { title: Some("Blog post draft"), first_prompt: "outline a post about running several Claude accounts", project: "notes", branch: "-", age_min: 2900, last_client: Client::Desktop, entries: &[2], archived: false, transcript: true, running: Running::No, turns: 17 },
    Session { title: Some("Sidebar sync spike"), first_prompt: "copy one local_*.json into the other account folder and see", project: "claude-account-manager", branch: "spike/sidebar", age_min: 4300, last_client: Client::Desktop, entries: &[0], archived: false, transcript: true, running: Running::No, turns: 23 },
    Session { title: Some("Rewrite README"), first_prompt: "the README still talks about clauth, rewrite it", project: "claude-account-manager", branch: "main", age_min: 9000, last_client: Client::Cli, entries: &[], archived: false, transcript: true, running: Running::No, turns: 9 },
    Session { title: Some("Dependency audit"), first_prompt: "list every dependency with a known CVE", project: "acme-web", branch: "chore/audit", age_min: 14000, last_client: Client::Desktop, entries: &[1], archived: true, transcript: true, running: Running::No, turns: 30 },
    Session { title: Some("Landing page copy"), first_prompt: "tighten the hero copy for the studio site", project: "studio-site", branch: "main", age_min: 20000, last_client: Client::Desktop, entries: &[3], archived: false, transcript: true, running: Running::No, turns: 14 },
    Session { title: Some("Plan holiday budget"), first_prompt: "make a spreadsheet for the december trip", project: "notes", branch: "-", age_min: 30000, last_client: Client::Desktop, entries: &[0], archived: false, transcript: true, running: Running::No, turns: 8 },
    Session { title: Some("Old migration plan"), first_prompt: "plan the postgres 14 to 16 migration", project: "acme-api", branch: "chore/pg16", age_min: 60000, last_client: Client::Desktop, entries: &[1], archived: false, transcript: false, running: Running::No, turns: 0 },
    Session { title: None, first_prompt: "what does this regex do: ^(?=.*\\d)", project: "rust-playground", branch: "main", age_min: 61000, last_client: Client::Cli, entries: &[], archived: false, transcript: false, running: Running::No, turns: 0 },
];

impl Session {
    fn name(&self) -> String {
        match self.title {
            Some(t) => t.to_string(),
            None => format!("“{}”", trunc(self.first_prompt, 48)),
        }
    }
    fn age(&self) -> String {
        let m = self.age_min;
        if m < 60 {
            format!("{m} min ago")
        } else if m < 1440 {
            format!("{} h ago", m / 60)
        } else if m < 2880 {
            "yesterday".into()
        } else {
            format!("{} days ago", m / 1440)
        }
    }
    fn day_bucket(&self) -> &'static str {
        match self.age_min {
            0..=1439 => "Today",
            1440..=2879 => "Yesterday",
            2880..=10079 => "This week",
            _ => "Earlier",
        }
    }
    /// Where it shows: Profiles whose sidebar has it, or CLI-only.
    fn where_text(&self) -> String {
        if self.entries.is_empty() {
            "CLI only".into()
        } else {
            self.entries.iter().map(|&p| short(p)).collect::<Vec<_>>().join(", ")
        }
    }
}

fn short(p: usize) -> String {
    match p {
        2 => "Side Lab".into(),
        _ => PROFILES[p].nick.into(),
    }
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.into()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

const VARIANTS: [&str; 3] = ["A: Table", "B: Browser", "C: Launcher"];

// Action tags: kind * 1000 + index.
const OPEN_TERMINAL: isize = 1000;
const OPEN_CLAUDE: isize = 2000;
const OPEN_CLAUDE_HERE: isize = 3000; // open under the desktop's current Profile (new entry)
const GUIDED_SWITCH: isize = 4000; // + Profile index
const FILTER: isize = 5000; // B sidebar: + filter index
const GROUP: isize = 6000;
const SHOW_ARCHIVED: isize = 6001;
const PREV_VARIANT: isize = 7000;
const NEXT_VARIANT: isize = 7001;

// ---------------------------------------------------------------- state

#[derive(Clone, Copy, PartialEq)]
enum Filter {
    All,
    Running,
    Project(usize),
    Profile(usize),
    CliOnly,
}

fn projects() -> Vec<&'static str> {
    let mut v: Vec<&str> = SESSIONS.iter().map(|s| s.project).collect();
    v.sort();
    v.dedup();
    v
}

fn filters() -> Vec<(Filter, String)> {
    let mut v = vec![(Filter::All, "All Sessions".to_string()), (Filter::Running, "Running".to_string())];
    for (i, p) in projects().iter().enumerate() {
        v.push((Filter::Project(i), p.to_string()));
    }
    for i in 0..PROFILES.len() {
        let suffix = if PROFILES[i].signed_in { "" } else { "  (not signed in)" };
        v.push((Filter::Profile(i), format!("{}{suffix}", short(i))));
    }
    v.push((Filter::CliOnly, "CLI only (no Sidebar entry)".to_string()));
    v
}

#[derive(Default)]
struct State {
    variant: usize,
    query: String,
    group: usize, // A: 0 none, 1 project, 2 Profile, 3 day
    show_archived: bool,
    filter: usize, // B: index into filters()
    selected: Option<usize>,
    cli: usize,
    desktop: usize,
    window: Option<Retained<NSWindow>>,
    body: Option<Retained<NSStackView>>,
    target: Option<Retained<Target>>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State { cli: 0, desktop: 1, ..Default::default() });
}

fn visible() -> Vec<usize> {
    STATE.with(|s| {
        let st = s.borrow();
        let q = st.query.to_lowercase();
        let f = filters()[st.filter].0;
        (0..SESSIONS.len())
            .filter(|&i| {
                let x = &SESSIONS[i];
                if x.archived && !st.show_archived && st.variant != 1 {
                    return false;
                }
                let hay = format!("{} {} {} {}", x.name(), x.first_prompt, x.project, x.branch).to_lowercase();
                if !q.is_empty() && !hay.contains(&q) {
                    return false;
                }
                if st.variant == 1 {
                    return match f {
                        Filter::All => !x.archived,
                        Filter::Running => x.running != Running::No,
                        Filter::Project(p) => x.project == projects()[p],
                        Filter::Profile(p) => x.entries.contains(&p),
                        Filter::CliOnly => x.entries.is_empty(),
                    };
                }
                true
            })
            .collect()
    })
}

fn print_state(why: &str) {
    let vis = visible();
    STATE.with(|s| {
        let st = s.borrow();
        println!(
            "[{why}] variant={} query={:?} group={} archived={} filter={} selected={:?} | CLI={} Desktop={} | visible={:?}",
            VARIANTS[st.variant],
            st.query,
            st.group,
            st.show_archived,
            filters()[st.filter].1,
            st.selected.map(|i| SESSIONS[i].name()),
            short(st.cli),
            short(st.desktop),
            vis
        );
    });
}

// ---------------------------------------------------------------- ObjC classes

define_class!(
    // Single target for every control (dispatches on the sender's tag) and delegate of the search field.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "USVTarget"]
    struct Target;

    impl Target {
        #[unsafe(method(act:))]
        fn act(&self, sender: &AnyObject) {
            let tag: isize = unsafe { msg_send![sender, tag] };
            handle(tag, sender, self.mtm());
        }

        #[unsafe(method(rebuild:))]
        fn rebuild_later(&self, _sender: Option<&AnyObject>) {
            rebuild_body(self.mtm());
        }

        #[unsafe(method(rebuildWindow:))]
        fn rebuild_window(&self, _sender: Option<&AnyObject>) {
            build_window(self.mtm());
        }
    }

    unsafe impl NSObjectProtocol for Target {}

    unsafe impl NSControlTextEditingDelegate for Target {
        #[unsafe(method(controlTextDidChange:))]
        fn text_did_change(&self, n: &NSNotification) {
            let field: Retained<NSTextField> = unsafe { Retained::cast_unchecked(n.object().unwrap()) };
            STATE.with(|s| {
                let mut st = s.borrow_mut();
                st.query = field.stringValue().to_string();
                st.selected = None;
            });
            let v = visible();
            STATE.with(|s| s.borrow_mut().selected = v.first().copied());
            schedule_rebuild();
            print_state("search");
        }

        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn do_command(&self, _c: &NSControl, _tv: &NSTextView, cmd: Sel) -> Bool {
            Bool::new(self.command(cmd))
        }
    }

    unsafe impl NSTextFieldDelegate for Target {}
    unsafe impl NSSearchFieldDelegate for Target {}
);

impl Target {
    fn command(&self, cmd: Sel) -> bool {
            let mtm = self.mtm();
            let v = visible();
            let sel_pos = STATE.with(|s| s.borrow().selected).and_then(|i| v.iter().position(|&x| x == i));
            if cmd == sel!(moveDown:) || cmd == sel!(moveUp:) {
                if v.is_empty() {
                    return true;
                }
                let n = v.len() as isize;
                let cur = sel_pos.map(|p| p as isize).unwrap_or(-1);
                let next = if cmd == sel!(moveDown:) { (cur + 1).min(n - 1) } else { (cur - 1).max(0) };
                STATE.with(|s| s.borrow_mut().selected = Some(v[next as usize]));
                schedule_rebuild();
                return true;
            }
            if cmd == sel!(insertNewline:) || cmd == sel!(noop:) {
                let Some(i) = STATE.with(|s| s.borrow().selected) else { return true };
                let cmd_down = NSApplication::sharedApplication(mtm)
                    .currentEvent()
                    .map(|e| e.modifierFlags().contains(NSEventModifierFlags::Command))
                    .unwrap_or(false);
                // ↩ = last-used client, ⌘↩ = the other one.
                let last = SESSIONS[i].last_client;
                let other = if last == Client::Cli { Client::Desktop } else { Client::Cli };
                open(mtm, i, if cmd_down { other } else { last });
                return true;
            }
        false
    }

    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    // A list row: draws selection, click selects, double-click opens in the last-used client.
    // Buttons inside the row still get their own clicks.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "USVRow"]
    #[ivars = Cell<isize>]
    struct Row;

    impl Row {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let i = self.ivars().get();
            if i >= 0 && STATE.with(|s| s.borrow().selected) == Some(i as usize) {
                let r = self.bounds();
                NSColor::controlAccentColor().colorWithAlphaComponent(0.22).set();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(r, 5.0, 5.0).fill();
            }
        }

        #[unsafe(method(hitTest:))]
        fn hit_test(&self, p: NSPoint) -> *mut NSView {
            let hit: *mut NSView = unsafe { msg_send![super(self), hitTest: p] };
            if hit.is_null() {
                return hit;
            }
            let is_button: bool = unsafe { msg_send![hit, isKindOfClass: NSButton::class()] };
            if is_button { hit } else { self as *const Self as *mut NSView }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, e: &NSEvent) {
            let i = self.ivars().get();
            if i < 0 {
                return;
            }
            let i = i as usize;
            STATE.with(|s| s.borrow_mut().selected = Some(i));
            if e.clickCount() == 2 {
                open(self.mtm(), i, SESSIONS[i].last_client);
            }
            schedule_rebuild();
            print_state("select");
        }
    }
);

impl Row {
    fn wrap(mtm: MainThreadMarker, content: &NSView, index: isize) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Cell::new(index));
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        this.addSubview(content);
        content.setTranslatesAutoresizingMaskIntoConstraints(false);
        pin_insets(content, &this, 4.0, 8.0);
        this
    }
}

define_class!(
    // Top-left origin, so a stack inside a scroll view starts at the top.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "USVFlipped"]
    struct Flipped;

    impl Flipped {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

// ---------------------------------------------------------------- view helpers

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

fn pin_insets(child: &NSView, parent: &NSView, v: f64, h: f64) {
    child.leadingAnchor().constraintEqualToAnchor_constant(&parent.leadingAnchor(), h).setActive(true);
    child.trailingAnchor().constraintEqualToAnchor_constant(&parent.trailingAnchor(), -h).setActive(true);
    child.topAnchor().constraintEqualToAnchor_constant(&parent.topAnchor(), v).setActive(true);
    child.bottomAnchor().constraintEqualToAnchor_constant(&parent.bottomAnchor(), -v).setActive(true);
}

#[derive(Clone, Copy)]
enum Tone {
    Primary,
    Secondary,
    Tertiary,
    Warning,
    Good,
}

fn label(mtm: MainThreadMarker, text: &str, size: f64, bold: bool, tone: Tone) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&ns(text), mtm);
    let font = if bold { NSFont::boldSystemFontOfSize(size) } else { NSFont::systemFontOfSize(size) };
    l.setFont(Some(&font));
    let c = match tone {
        Tone::Primary => None,
        Tone::Secondary => Some(NSColor::secondaryLabelColor()),
        Tone::Tertiary => Some(NSColor::tertiaryLabelColor()),
        Tone::Warning => Some(NSColor::systemOrangeColor()),
        Tone::Good => Some(NSColor::systemGreenColor()),
    };
    if let Some(c) = c {
        l.setTextColor(Some(&c));
    }
    l.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    l.setContentCompressionResistancePriority_forOrientation(250.0, NSLayoutConstraintOrientation::Horizontal);
    l
}

fn wrapping(mtm: MainThreadMarker, text: &str, size: f64, tone: Tone, width: f64) -> Retained<NSTextField> {
    let l = NSTextField::wrappingLabelWithString(&ns(text), mtm);
    l.setFont(Some(&NSFont::systemFontOfSize(size)));
    if let Tone::Secondary = tone {
        l.setTextColor(Some(&NSColor::secondaryLabelColor()));
    }
    if let Tone::Warning = tone {
        l.setTextColor(Some(&NSColor::systemOrangeColor()));
    }
    l.setPreferredMaxLayoutWidth(width);
    fixed_width(&l, width);
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
    l.setContentCompressionResistancePriority_forOrientation(750.0, NSLayoutConstraintOrientation::Horizontal);
    l
}

fn spacer(mtm: MainThreadMarker) -> Retained<NSView> {
    let v = NSView::new(mtm);
    v.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    v.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
    v
}

fn stack(mtm: MainThreadMarker, vertical: bool, spacing: f64, views: &[&NSView]) -> Retained<NSStackView> {
    let s = NSStackView::new(mtm);
    s.setOrientation(if vertical { NSUserInterfaceLayoutOrientation::Vertical } else { NSUserInterfaceLayoutOrientation::Horizontal });
    s.setAlignment(if vertical { NSLayoutAttribute::Leading } else { NSLayoutAttribute::CenterY });
    s.setSpacing(spacing);
    for v in views {
        s.addArrangedSubview(v);
    }
    s
}

fn fill_width(s: &NSStackView) {
    let i = s.edgeInsets();
    for v in s.arrangedSubviews().iter() {
        v.widthAnchor().constraintEqualToAnchor_constant(&s.widthAnchor(), -(i.left + i.right)).setActive(true);
    }
}

fn target() -> Retained<Target> {
    STATE.with(|s| s.borrow().target.clone().unwrap())
}

fn button(mtm: MainThreadMarker, title: &str, tag: isize, enabled: bool) -> Retained<NSButton> {
    let t = target();
    let b = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), Some(&t), Some(sel!(act:)), mtm) };
    b.setTag(tag);
    b.setControlSize(NSControlSize::Small);
    b.setFont(Some(&NSFont::systemFontOfSize(11.0)));
    b.setEnabled(enabled);
    b
}

fn separator(mtm: MainThreadMarker) -> Retained<NSView> {
    let b = NSBox::new(mtm);
    b.setBoxType(NSBoxType::Separator);
    b.into_super()
}

fn scroll(mtm: MainThreadMarker, content: &NSStackView) -> Retained<NSScrollView> {
    let sv = NSScrollView::new(mtm);
    sv.setHasVerticalScroller(true);
    sv.setDrawsBackground(false);
    sv.setTranslatesAutoresizingMaskIntoConstraints(false);
    let doc: Retained<Flipped> = unsafe { msg_send![super(Flipped::alloc(mtm).set_ivars(())), initWithFrame: NSRect::ZERO] };
    doc.setTranslatesAutoresizingMaskIntoConstraints(false);
    doc.addSubview(content);
    content.setTranslatesAutoresizingMaskIntoConstraints(false);
    pin_insets(content, &doc, 0.0, 0.0);
    sv.setDocumentView(Some(&doc));
    let clip = sv.contentView();
    doc.leadingAnchor().constraintEqualToAnchor(&clip.leadingAnchor()).setActive(true);
    doc.trailingAnchor().constraintEqualToAnchor(&clip.trailingAnchor()).setActive(true);
    doc.topAnchor().constraintEqualToAnchor(&clip.topAnchor()).setActive(true);
    sv
}

fn search_field(mtm: MainThreadMarker, placeholder: &str, big: bool) -> Retained<NSSearchField> {
    let f = NSSearchField::new(mtm);
    f.setPlaceholderString(Some(&ns(placeholder)));
    let t = target();
    unsafe { f.setDelegate(Some(ProtocolObject::from_ref(&*t))) };
    if big {
        f.setFont(Some(&NSFont::systemFontOfSize(20.0)));
        f.heightAnchor().constraintEqualToConstant(36.0).setActive(true);
    }
    let q = STATE.with(|s| s.borrow().query.clone());
    f.setStringValue(&ns(&q));
    f
}

fn status_dot(mtm: MainThreadMarker, s: &Session) -> Retained<NSView> {
    match s.running {
        Running::Terminal => label(mtm, "●", 10.0, false, Tone::Good).into_super().into_super(),
        Running::DesktopBusy => label(mtm, "●", 10.0, false, Tone::Warning).into_super().into_super(),
        Running::No => {
            let l = label(mtm, "●", 10.0, false, Tone::Primary);
            l.setTextColor(Some(&NSColor::clearColor()));
            l.into_super().into_super()
        }
    }
}

fn profile_badges(mtm: MainThreadMarker, s: &Session) -> Retained<NSStackView> {
    let desktop = STATE.with(|st| st.borrow().desktop);
    let views: Vec<Retained<NSView>> = if s.entries.is_empty() {
        vec![badge(mtm, "CLI only", &NSColor::systemGrayColor()).into_super().into_super()]
    } else {
        s.entries
            .iter()
            .map(|&p| {
                let c = if p == desktop {
                    NSColor::systemBlueColor()
                } else if PROFILES[p].signed_in {
                    NSColor::systemGrayColor()
                } else {
                    NSColor::tertiaryLabelColor()
                };
                badge(mtm, &short(p), &c).into_super().into_super()
            })
            .collect()
    };
    let refs: Vec<&NSView> = views.iter().map(|v| &**v).collect();
    stack(mtm, false, 4.0, &refs)
}

/// The Claude button's title says whether opening needs a choice (ellipsis).
fn claude_title(s: &Session) -> &'static str {
    let desktop = STATE.with(|st| st.borrow().desktop);
    if s.entries.contains(&desktop) {
        "Claude"
    } else {
        "Claude…"
    }
}

// ---------------------------------------------------------------- variant A: Table

fn variant_a_toolbar(mtm: MainThreadMarker) -> Retained<NSView> {
    let f = search_field(mtm, "Search titles, prompts, projects, branches", false);
    fixed_width(&f, 320.0);
    let pop = NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, false);
    for t in ["Group: None", "Group: Project", "Group: Profile", "Group: Day"] {
        pop.addItemWithTitle(&ns(t));
    }
    let (group, archived) = STATE.with(|s| (s.borrow().group, s.borrow().show_archived));
    pop.selectItemAtIndex(group as isize);
    pop.setTag(GROUP);
    unsafe {
        pop.setTarget(Some(&target()));
        pop.setAction(Some(sel!(act:)));
    }
    pop.setControlSize(NSControlSize::Small);
    let cb = unsafe { NSButton::checkboxWithTitle_target_action(&ns("Show archived"), Some(&target()), Some(sel!(act:)), mtm) };
    cb.setTag(SHOW_ARCHIVED);
    cb.setState(if archived { NSControlStateValueOn } else { NSControlStateValueOff });
    cb.setControlSize(NSControlSize::Small);
    let st = stack(mtm, false, 10.0, &[&f, &pop, &cb, &spacer(mtm)]);
    st.into_super()
}

fn a_row(mtm: MainThreadMarker, i: usize) -> Retained<NSView> {
    let s = &SESSIONS[i];
    let tone = if s.transcript { Tone::Primary } else { Tone::Tertiary };
    let title = label(mtm, &s.name(), 13.0, false, tone);
    fixed_width(&title, 300.0);
    let proj = label(mtm, &format!("{} · {}", s.project, s.branch), 11.0, false, if s.transcript { Tone::Secondary } else { Tone::Tertiary });
    fixed_width(&proj, 210.0);
    let badges = profile_badges(mtm, s);
    fixed_width(&badges, 130.0);
    let age = label(mtm, &s.age(), 11.0, false, Tone::Secondary);
    fixed_width(&age, 80.0);
    let actions: Retained<NSView> = if s.transcript {
        let t = button(mtm, "Terminal", OPEN_TERMINAL + i as isize, true);
        let c = button(mtm, claude_title(s), OPEN_CLAUDE + i as isize, true);
        stack(mtm, false, 4.0, &[&t, &c]).into_super()
    } else {
        label(mtm, "Transcript deleted", 11.0, false, Tone::Warning).into_super().into_super()
    };
    let row = stack(mtm, false, 8.0, &[&status_dot(mtm, s), &title, &proj, &badges, &age, &spacer(mtm), &actions]);
    Row::wrap(mtm, &row, i as isize).into_super()
}

fn a_header(mtm: MainThreadMarker) -> Retained<NSView> {
    let cols: Vec<(&str, f64)> = vec![("", 10.0), ("Session", 300.0), ("Project · branch", 210.0), ("In sidebar of", 130.0), ("Last activity", 80.0)];
    let views: Vec<Retained<NSView>> = cols
        .iter()
        .map(|(t, w)| {
            let l = label(mtm, t, 11.0, true, Tone::Secondary);
            fixed_width(&l, *w);
            l.into_super().into_super()
        })
        .collect();
    let refs: Vec<&NSView> = views.iter().map(|v| &**v).collect();
    let s = stack(mtm, false, 8.0, &refs);
    s.setEdgeInsets(NSEdgeInsets { top: 0.0, left: 8.0, bottom: 0.0, right: 8.0 });
    s.into_super()
}

fn variant_a_body(mtm: MainThreadMarker) -> Retained<NSView> {
    let v = visible();
    let group = STATE.with(|s| s.borrow().group);
    let list = stack(mtm, true, 0.0, &[]);
    let key = |i: usize| -> Vec<String> {
        let s = &SESSIONS[i];
        match group {
            1 => vec![s.project.to_string()],
            2 => {
                if s.entries.is_empty() {
                    vec!["CLI only (no Sidebar entry)".into()]
                } else {
                    s.entries.iter().map(|&p| PROFILES[p].full.to_string()).collect()
                }
            }
            3 => vec![s.day_bucket().to_string()],
            _ => vec![String::new()],
        }
    };
    let mut groups: Vec<String> = vec![];
    for &i in &v {
        for k in key(i) {
            if !groups.contains(&k) {
                groups.push(k);
            }
        }
    }
    if group == 1 {
        groups.sort();
    }
    for g in &groups {
        if !g.is_empty() {
            let h = label(mtm, g, 12.0, true, Tone::Primary);
            let hs = stack(mtm, true, 0.0, &[&h]);
            hs.setEdgeInsets(NSEdgeInsets { top: 10.0, left: 8.0, bottom: 2.0, right: 8.0 });
            list.addArrangedSubview(&hs);
        }
        for &i in &v {
            if key(i).contains(g) {
                list.addArrangedSubview(&a_row(mtm, i));
            }
        }
    }
    if v.is_empty() {
        list.addArrangedSubview(&label(mtm, "  No Session matches.", 12.0, false, Tone::Secondary));
    }
    fill_width(&list);
    let (cli, desktop) = STATE.with(|s| (s.borrow().cli, s.borrow().desktop));
    let footer = label(
        mtm,
        &format!(
            "{} of {} Sessions · CLI: {} · Desktop: {} · Sidebar sync: Off · ● running (green Terminal, orange desktop turn) · double-click opens where it was last used",
            v.len(),
            SESSIONS.len(),
            short(cli),
            short(desktop)
        ),
        11.0,
        false,
        Tone::Secondary,
    );
    let sv = scroll(mtm, &list);
    let body = stack(mtm, true, 6.0, &[&a_header(mtm), &separator(mtm), &sv, &footer]);
    sv.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
    fill_width(&body);
    body.into_super()
}

// ---------------------------------------------------------------- variant B: Browser

fn variant_b_toolbar(mtm: MainThreadMarker) -> Retained<NSView> {
    let f = search_field(mtm, "Search", false);
    fixed_width(&f, 260.0);
    stack(mtm, false, 10.0, &[&spacer(mtm), &f]).into_super()
}

fn b_sidebar(mtm: MainThreadMarker) -> Retained<NSView> {
    let cur = STATE.with(|s| s.borrow().filter);
    let list = stack(mtm, true, 2.0, &[]);
    let mut last_kind = 99;
    for (n, (f, name)) in filters().iter().enumerate() {
        let kind = match f {
            Filter::All | Filter::Running => 0,
            Filter::Project(_) => 1,
            Filter::Profile(_) | Filter::CliOnly => 2,
        };
        if kind != last_kind {
            let h = ["", "Projects", "Sidebars"][kind];
            if !h.is_empty() {
                list.addArrangedSubview(&label(mtm, h, 11.0, true, Tone::Secondary));
            }
            last_kind = kind;
        }
        let b = button(mtm, name, FILTER + n as isize, true);
        b.setBordered(false);
        b.setAlignment(NSTextAlignment::Left);
        if n == cur {
            b.setFont(Some(&NSFont::boldSystemFontOfSize(12.0)));
        } else {
            b.setFont(Some(&NSFont::systemFontOfSize(12.0)));
        }
        list.addArrangedSubview(&b);
    }
    list.setEdgeInsets(NSEdgeInsets { top: 8.0, left: 10.0, bottom: 8.0, right: 6.0 });
    let sv = scroll(mtm, &list);
    fixed_width(&sv, 210.0);
    sv.into_super()
}

fn b_list(mtm: MainThreadMarker) -> Retained<NSView> {
    let list = stack(mtm, true, 0.0, &[]);
    for i in visible() {
        let s = &SESSIONS[i];
        let tone = if s.transcript { Tone::Primary } else { Tone::Tertiary };
        let t = label(mtm, &s.name(), 13.0, false, tone);
        let mut sub = format!("{} · {}", s.project, s.age());
        if !s.transcript {
            sub.push_str(" · transcript deleted");
        }
        if s.archived {
            sub.push_str(" · archived");
        }
        let d = label(mtm, &sub, 11.0, false, Tone::Secondary);
        let col = stack(mtm, true, 1.0, &[&t, &d]);
        let row = stack(mtm, false, 6.0, &[&status_dot(mtm, s), &col]);
        list.addArrangedSubview(&Row::wrap(mtm, &row, i as isize));
    }
    if list.arrangedSubviews().count() == 0 {
        list.addArrangedSubview(&label(mtm, "  No Session here.", 12.0, false, Tone::Secondary));
    }
    fill_width(&list);
    let sv = scroll(mtm, &list);
    sv.widthAnchor().constraintGreaterThanOrEqualToConstant(300.0).setActive(true);
    sv.into_super()
}

fn b_detail(mtm: MainThreadMarker) -> Retained<NSView> {
    const W: f64 = 330.0;
    let (sel, cli, desktop) = STATE.with(|s| (s.borrow().selected, s.borrow().cli, s.borrow().desktop));
    let d = stack(mtm, true, 8.0, &[]);
    d.setEdgeInsets(NSEdgeInsets { top: 12.0, left: 14.0, bottom: 12.0, right: 14.0 });
    let Some(i) = sel else {
        d.addArrangedSubview(&label(mtm, "Select a Session.", 13.0, false, Tone::Secondary));
        d.addArrangedSubview(&spacer(mtm));
        d.setDistribution(NSStackViewDistribution::Fill);
        fixed_width(&d, W);
        return d.into_super();
    };
    let s = &SESSIONS[i];
    d.addArrangedSubview(&wrapping(mtm, &s.name(), 15.0, Tone::Primary, W - 28.0));
    let facts = format!(
        "~/Dev/{}  ·  {}\nLast used in {}, {}  ·  {} turns\nFirst prompt: “{}”",
        s.project,
        s.branch,
        if s.last_client == Client::Cli { "the CLI" } else { "Claude desktop" },
        s.age(),
        s.turns,
        trunc(s.first_prompt, 80)
    );
    d.addArrangedSubview(&wrapping(mtm, &facts, 11.0, Tone::Secondary, W - 28.0));
    match s.running {
        Running::Terminal => d.addArrangedSubview(&label(mtm, "● Running in Terminal now", 11.0, true, Tone::Good)),
        Running::DesktopBusy => d.addArrangedSubview(&label(mtm, "● A desktop turn is running now", 11.0, true, Tone::Warning)),
        Running::No => {}
    }
    d.addArrangedSubview(&separator(mtm));
    d.addArrangedSubview(&label(mtm, "Sidebar entries", 11.0, true, Tone::Secondary));
    for p in 0..PROFILES.len() {
        let has = s.entries.contains(&p);
        let mark = if has { "✓" } else { "–" };
        let mut t = format!("{mark}  {}", short(p));
        if p == desktop {
            t.push_str("  (desktop now)");
        }
        if !PROFILES[p].signed_in {
            t.push_str("  (not signed in)");
        }
        d.addArrangedSubview(&label(mtm, &t, 11.0, false, if has { Tone::Primary } else { Tone::Tertiary }));
    }
    d.addArrangedSubview(&separator(mtm));
    if !s.transcript {
        d.addArrangedSubview(&wrapping(
            mtm,
            "The Transcript was deleted (cleanupPeriodDays). The Sidebar entry still shows a title in Work, but nothing can be resumed.",
            11.0,
            Tone::Warning,
            W - 28.0,
        ));
    } else {
        d.addArrangedSubview(&label(mtm, "Open", 11.0, true, Tone::Secondary));
        let t_title = if s.running == Running::Terminal { "Show in Terminal" } else { "Resume in Terminal" };
        d.addArrangedSubview(&button(mtm, t_title, OPEN_TERMINAL + i as isize, true));
        d.addArrangedSubview(&label(mtm, &format!("New turns count against {} (CLI).", short(cli)), 10.0, false, Tone::Secondary));
        if s.entries.contains(&desktop) {
            d.addArrangedSubview(&button(mtm, "Open in Claude", OPEN_CLAUDE + i as isize, true));
            d.addArrangedSubview(&label(mtm, &format!("Opens its entry in {}'s sidebar.", short(desktop)), 10.0, false, Tone::Secondary));
        } else {
            d.addArrangedSubview(&button(mtm, &format!("Open in Claude as {}", short(desktop)), OPEN_CLAUDE_HERE + i as isize, true));
            d.addArrangedSubview(&wrapping(
                mtm,
                &format!("Adds a new entry to {}'s sidebar; title and archive state don't carry over.", short(desktop)),
                10.0,
                Tone::Secondary,
                W - 28.0,
            ));
            for &p in s.entries {
                let ok = s.running != Running::DesktopBusy;
                d.addArrangedSubview(&button(mtm, &format!("Switch Claude to {}, then open…", short(p)), GUIDED_SWITCH + (i * 10 + p) as isize, ok));
            }
        }
    }
    d.addArrangedSubview(&spacer(mtm));
    fixed_width(&d, W);
    d.setDistribution(NSStackViewDistribution::Fill);
    d.into_super()
}

fn variant_b_body(mtm: MainThreadMarker) -> Retained<NSView> {
    let row = stack(mtm, false, 0.0, &[&b_sidebar(mtm), &separator(mtm), &b_list(mtm), &separator(mtm), &b_detail(mtm)]);
    row.setAlignment(NSLayoutAttribute::Height);
    row.setDistribution(NSStackViewDistribution::Fill);
    row.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
    row.heightAnchor().constraintGreaterThanOrEqualToConstant(440.0).setActive(true);
    row.into_super()
}

// ---------------------------------------------------------------- variant C: Launcher

fn variant_c_toolbar(mtm: MainThreadMarker) -> Retained<NSView> {
    let f = search_field(mtm, "Find a Session…", true);
    f.into_super().into_super().into_super()
}

fn variant_c_body(mtm: MainThreadMarker) -> Retained<NSView> {
    let v = visible();
    let sel = STATE.with(|s| s.borrow().selected);
    let list = stack(mtm, true, 0.0, &[]);
    for &i in &v {
        let s = &SESSIONS[i];
        let tone = if s.transcript { Tone::Primary } else { Tone::Tertiary };
        let t = label(mtm, &s.name(), 14.0, false, tone);
        let meta = label(mtm, &format!("{} · {}", s.project, s.age()), 11.0, false, Tone::Secondary);
        fixed_width(&meta, 240.0);
        meta.setAlignment(NSTextAlignment::Right);
        let mut views: Vec<Retained<NSView>> = vec![status_dot(mtm, s), t.into_super().into_super(), spacer(mtm)];
        if !s.transcript {
            views.push(label(mtm, "transcript deleted", 10.0, false, Tone::Warning).into_super().into_super());
        }
        views.push(profile_badges(mtm, s).into_super());
        views.push(meta.into_super().into_super());
        let refs: Vec<&NSView> = views.iter().map(|v| &**v).collect();
        let row = stack(mtm, false, 8.0, &refs);
        list.addArrangedSubview(&Row::wrap(mtm, &row, i as isize));
    }
    if v.is_empty() {
        list.addArrangedSubview(&label(mtm, "  No Session matches.", 13.0, false, Tone::Secondary));
    }
    fill_width(&list);
    let hint = match sel {
        Some(i) if SESSIONS[i].transcript => {
            let s = &SESSIONS[i];
            let (ret, cmd) = if s.last_client == Client::Cli { ("Terminal", claude_title(s)) } else { (claude_title(s), "Terminal") };
            format!("↑↓ select    ↩ open in {ret} (last used)    ⌘↩ open in {cmd}    double-click = ↩")
        }
        Some(_) => "Transcript deleted: nothing to open.".to_string(),
        None => "↑↓ select    ↩ open where last used    ⌘↩ open in the other client".to_string(),
    };
    let sv = scroll(mtm, &list);
    sv.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
    let body = stack(mtm, true, 6.0, &[&sv, &separator(mtm), &label(mtm, &hint, 11.0, false, Tone::Secondary)]);
    fill_width(&body);
    body.into_super()
}

// ---------------------------------------------------------------- window assembly

fn switcher(mtm: MainThreadMarker) -> Retained<NSView> {
    let v = STATE.with(|s| s.borrow().variant);
    let prev = button(mtm, "◀", PREV_VARIANT, true);
    let next = button(mtm, "▶", NEXT_VARIANT, true);
    let l = label(mtm, &format!("PROTOTYPE  {}", VARIANTS[v]), 11.0, true, Tone::Primary);
    let s = stack(mtm, false, 6.0, &[&spacer(mtm), &prev, &l, &next, &spacer(mtm)]);
    s.setEdgeInsets(NSEdgeInsets { top: 4.0, left: 0.0, bottom: 4.0, right: 0.0 });
    // Tinted pill so it reads as prototype chrome, not part of the design.
    let b = NSBox::new(mtm);
    b.setBoxType(NSBoxType::Custom);
    b.setBorderWidth(0.0);
    b.setCornerRadius(6.0);
    b.setFillColor(&NSColor::systemPurpleColor().colorWithAlphaComponent(0.25));
    b.setContentView(Some(&s));
    b.into_super()
}

fn build_window(mtm: MainThreadMarker) {
    let v = STATE.with(|s| s.borrow().variant);
    let (w, h, title) = match v {
        0 => (980.0, 560.0, "All Sessions"),
        1 => (1040.0, 580.0, "All Sessions"),
        _ => (720.0, 480.0, "Find a Session"),
    };
    let toolbar = match v {
        0 => variant_a_toolbar(mtm),
        1 => variant_b_toolbar(mtm),
        _ => variant_c_toolbar(mtm),
    };
    toolbar.setContentHuggingPriority_forOrientation(750.0, NSLayoutConstraintOrientation::Vertical);
    let body = stack(mtm, true, 0.0, &[]);
    body.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
    let root = stack(mtm, true, 8.0, &[&toolbar, &body, &switcher(mtm)]);
    root.setEdgeInsets(NSEdgeInsets { top: 10.0, left: 12.0, bottom: 8.0, right: 12.0 });
    root.setDistribution(NSStackViewDistribution::Fill);
    fill_width(&root);

    let win = STATE.with(|s| s.borrow().window.clone()).unwrap_or_else(|| {
        let mask = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Resizable | NSWindowStyleMask::Miniaturizable;
        let win = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), NSRect::new(NSPoint::ZERO, NSSize::new(w, h)), mask, NSBackingStoreType::Buffered, false)
        };
        unsafe { win.setReleasedWhenClosed(false) };
        win
    });
    win.setTitle(&ns(title));
    win.setContentView(Some(&root));
    win.setContentSize(NSSize::new(w, h));
    win.center();
    STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.window = Some(win.clone());
        st.body = Some(body);
    });
    rebuild_body(mtm);
    win.makeKeyAndOrderFront(None);
    println!("window-number {}", win.windowNumber());
    // Put the cursor in the search field.
    if let Some(f) = find_search(&toolbar) {
        win.makeFirstResponder(Some(&f));
    }
}

fn find_search(v: &NSView) -> Option<Retained<NSView>> {
    let is: bool = unsafe { msg_send![v, isKindOfClass: NSSearchField::class()] };
    if is {
        return Some(unsafe { Retained::retain(v as *const NSView as *mut NSView) }.unwrap());
    }
    for c in v.subviews().iter() {
        if let Some(f) = find_search(&c) {
            return Some(f);
        }
    }
    None
}

fn rebuild_body(mtm: MainThreadMarker) {
    let (v, body) = STATE.with(|s| (s.borrow().variant, s.borrow().body.clone().unwrap()));
    for sub in body.arrangedSubviews().iter() {
        body.removeArrangedSubview(&sub);
        sub.removeFromSuperview();
    }
    let content = match v {
        0 => variant_a_body(mtm),
        1 => variant_b_body(mtm),
        _ => variant_c_body(mtm),
    };
    body.addArrangedSubview(&content);
    fill_width(&body);
    content.heightAnchor().constraintEqualToAnchor(&body.heightAnchor()).setActive(true);
}

fn schedule_rebuild() {
    let t = target();
    let _: () = unsafe { msg_send![&*t, performSelector: sel!(rebuild:), withObject: std::ptr::null::<AnyObject>(), afterDelay: 0.0f64] };
}

// ---------------------------------------------------------------- actions (stubs)

fn alert(mtm: MainThreadMarker, title: &str, info: &str, buttons: &[&str]) -> usize {
    println!("  stub: {title}: {info}");
    let a = NSAlert::new(mtm);
    a.setMessageText(&ns(title));
    a.setInformativeText(&ns(info));
    for b in buttons {
        a.addButtonWithTitle(&ns(b));
    }
    let r = a.runModal();
    let n = (r - 1000).max(0) as usize;
    if !buttons.is_empty() {
        println!("  chose: {}", buttons.get(n).unwrap_or(&"?"));
    }
    n
}

fn open(mtm: MainThreadMarker, i: usize, client: Client) {
    let s = &SESSIONS[i];
    if !s.transcript {
        alert(mtm, "Transcript deleted", &format!("\"{}\" can't be resumed: its Transcript was cleaned up. Only its Sidebar entry is left.", s.name()), &[]);
        return;
    }
    match client {
        Client::Cli => open_terminal(mtm, i),
        Client::Desktop => open_claude(mtm, i),
    }
}

fn open_terminal(mtm: MainThreadMarker, i: usize) {
    let s = &SESSIONS[i];
    let cli = STATE.with(|st| st.borrow().cli);
    if s.running == Running::Terminal {
        alert(mtm, "Already running", &format!("\"{}\" is open in a Terminal window (pid 4242). Would bring that window forward instead of starting a second copy.", s.name()), &[]);
    } else {
        alert(mtm, "Resume in Terminal", &format!("Would open Terminal in ~/Dev/{} and run `claude --resume <id>`. New turns count against {} (the CLI's Active Profile).", s.project, short(cli)), &[]);
    }
}

fn open_claude(mtm: MainThreadMarker, i: usize) {
    let s = &SESSIONS[i];
    let desktop = STATE.with(|st| st.borrow().desktop);
    if s.entries.contains(&desktop) {
        alert(mtm, "Open in Claude", &format!("Would open claude://code/continue?session=local_<id>: \"{}\" in {}'s sidebar.", s.name(), short(desktop)), &[]);
        return;
    }
    // No entry in the desktop's Active Profile: offer both ways.
    let mut buttons: Vec<String> = vec![format!("Open as {}", short(desktop))];
    for &p in s.entries {
        buttons.push(format!("Switch to {}…", short(p)));
    }
    buttons.push("Cancel".into());
    let refs: Vec<&str> = buttons.iter().map(|b| b.as_str()).collect();
    let where_ = if s.entries.is_empty() { "a CLI Session".to_string() } else { format!("in {}'s sidebar", s.where_text()) };
    let n = alert(
        mtm,
        &format!("\"{}\" isn't in {}'s sidebar", s.name(), short(desktop)),
        &format!("It is {where_}. Claude desktop is signed in as {}. Opening it there adds a new entry (title and archive state don't carry over); switching first keeps its entry.", short(desktop)),
        &refs,
    );
    if n == 0 {
        open_claude_here(mtm, i);
    } else if n <= s.entries.len() {
        guided_switch(mtm, i, s.entries[n - 1]);
    }
}

fn open_claude_here(mtm: MainThreadMarker, i: usize) {
    let desktop = STATE.with(|st| st.borrow().desktop);
    alert(mtm, "Open in Claude", &format!("Would run `claude --desktop --resume <id>`: \"{}\" gets a fresh entry in {}'s sidebar.", SESSIONS[i].name(), short(desktop)), &[]);
}

fn guided_switch(mtm: MainThreadMarker, i: usize, p: usize) {
    if SESSIONS.iter().any(|s| s.running == Running::DesktopBusy) {
        alert(mtm, "Can't switch now", "A desktop turn is running (\"Investigate flaky checkout test\"). Switching accounts would stop it. Try again when it's done.", &[]);
        return;
    }
    let sign = if PROFILES[p].signed_in { "" } else { " (this Profile isn't signed in to the app yet: you'll sign in in the browser too)" };
    alert(mtm, "Guided desktop switch", &format!("Claude comes forward. Sign out, then sign in as {} ({}){sign}. Once confirmed, \"{}\" opens from its entry.", short(p), PROFILES[p].full, SESSIONS[i].name()), &[]);
    STATE.with(|s| s.borrow_mut().desktop = p);
}

fn handle(tag: isize, sender: &AnyObject, mtm: MainThreadMarker) {
    match tag {
        PREV_VARIANT | NEXT_VARIANT => {
            STATE.with(|s| {
                let mut st = s.borrow_mut();
                st.variant = (st.variant + if tag == NEXT_VARIANT { 1 } else { 2 }) % 3;
                st.selected = None;
            });
            // Defer: the sender lives in the window being rebuilt.
            let t = target();
            let _: () = unsafe { msg_send![&*t, performSelector: sel!(rebuildWindow:), withObject: std::ptr::null::<AnyObject>(), afterDelay: 0.0f64] };
            print_state("variant");
            return;
        }
        GROUP => {
            let n: isize = unsafe { msg_send![sender, indexOfSelectedItem] };
            STATE.with(|s| s.borrow_mut().group = n as usize);
        }
        SHOW_ARCHIVED => {
            let on: isize = unsafe { msg_send![sender, state] };
            STATE.with(|s| s.borrow_mut().show_archived = on == 1);
        }
        t if (FILTER..FILTER + 1000).contains(&t) => {
            STATE.with(|s| {
                let mut st = s.borrow_mut();
                st.filter = (t - FILTER) as usize;
                st.selected = None;
            });
        }
        t if (OPEN_TERMINAL..OPEN_TERMINAL + 1000).contains(&t) => open(mtm, (t - OPEN_TERMINAL) as usize, Client::Cli),
        t if (OPEN_CLAUDE..OPEN_CLAUDE + 1000).contains(&t) => open(mtm, (t - OPEN_CLAUDE) as usize, Client::Desktop),
        t if (OPEN_CLAUDE_HERE..OPEN_CLAUDE_HERE + 1000).contains(&t) => open_claude_here(mtm, (t - OPEN_CLAUDE_HERE) as usize),
        t if (GUIDED_SWITCH..GUIDED_SWITCH + 1000).contains(&t) => {
            let n = (t - GUIDED_SWITCH) as usize;
            guided_switch(mtm, n / 10, n % 10);
        }
        _ => {}
    }
    schedule_rebuild();
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
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let target = Target::new(mtm);
    let select = args.iter().position(|a| a == "--select").and_then(|i| args.get(i + 1)).and_then(|s| s.parse::<usize>().ok());
    STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.variant = variant;
        st.selected = select;
        st.target = Some(target);
    });
    build_window(mtm);
    print_state("start");
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    app.run();
}
