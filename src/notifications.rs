//! User notifications through `UNUserNotificationCenter`
//! (`objc2-user-notifications`), with an optional action button whose click
//! goes back to the core as a [`NotificationAction`]. The action travels in
//! the notification's `userInfo`, so a click after a relaunch still works.
//!
//! Only a bundled app may use the center (outside a bundle it throws), so an
//! unbundled run skips notifications. Everything here runs on the main
//! thread; the center's delegate hops there before touching any state.

use std::cell::{OnceCell, RefCell};

use accroster_core::NotificationAction;
use block2::{DynBlock, RcBlock};
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, ProtocolObject};
use objc2::{AnyThread, define_class, msg_send};
use objc2_foundation::{
    NSArray, NSBundle, NSDictionary, NSError, NSObjectProtocol, NSSet, NSString, NSUUID,
};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationAction,
    UNNotificationActionOptions, UNNotificationCategory, UNNotificationCategoryOptions,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};

/// The identifier of a notification's one action button.
const ACTION: &str = "action";
/// The `userInfo` key holding [`NotificationAction::encode`]'s text.
const ACTION_KEY: &str = "accroster.action";

thread_local! {
    /// Sends a clicked action to the core.
    static ON_ACTION: OnceCell<Box<dyn Fn(NotificationAction)>> = const { OnceCell::new() };
    /// Keeps the center's (weak) delegate alive.
    static DELEGATE: OnceCell<Retained<Delegate>> = const { OnceCell::new() };
    /// The action titles registered as categories so far.
    static CATEGORIES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

define_class!(
    /// The center's delegate: shows banners while the app is frontmost and
    /// routes action clicks.
    #[unsafe(super(NSObject))]
    #[name = "AccRosterNotificationDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            handler: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            handler.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            handler: &DynBlock<dyn Fn()>,
        ) {
            let clicked = response.actionIdentifier().to_string() == ACTION;
            let action = clicked
                .then(|| {
                    let info = response.notification().request().content().userInfo();
                    let key = NSString::from_str(ACTION_KEY);
                    let text = info.objectForKey(&key)?.downcast::<NSString>().ok()?;
                    NotificationAction::decode(&text.to_string())
                })
                .flatten();
            if let Some(action) = action {
                DispatchQueue::main().exec_async(move || {
                    ON_ACTION.with(|on_action| {
                        if let Some(on_action) = on_action.get() {
                            on_action(action);
                        }
                    });
                });
            }
            handler.call(());
        }
    }
);

fn bundled() -> bool {
    NSBundle::mainBundle().bundleIdentifier().is_some()
}

/// Becomes the center's delegate; `on_action` receives each clicked
/// action. Call once, on the main thread.
pub fn start(on_action: impl Fn(NotificationAction) + 'static) {
    ON_ACTION.with(|cell| {
        let _ = cell.set(Box::new(on_action));
    });
    if !bundled() {
        return;
    }
    // SAFETY: `NSObject`'s `init` on a freshly allocated object.
    let delegate: Retained<Delegate> =
        unsafe { msg_send![super(Delegate::alloc().set_ivars(())), init] };
    UNUserNotificationCenter::currentNotificationCenter()
        .setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    DELEGATE.with(|cell| {
        let _ = cell.set(delegate);
    });
}

/// Posts a notification, asking for permission the first time. With an
/// action, its button titled `action.0` sends `action.1` when clicked.
pub fn post(title: &str, body: &str, action: Option<(&str, &NotificationAction)>) {
    if !bundled() {
        return;
    }
    let center = UNUserNotificationCenter::currentNotificationCenter();
    let asked = RcBlock::new(|_granted: Bool, _error: *mut NSError| {});
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &asked,
    );
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    let id = NSUUID::UUID().UUIDString();
    if let Some((title, on_click)) = action {
        content.setCategoryIdentifier(&register_category(&center, title));
        let key = NSString::from_str(ACTION_KEY);
        let value = NSString::from_str(&on_click.encode());
        let info = NSDictionary::<NSString, NSString>::from_slices(&[&*key], &[&*value]);
        // SAFETY: a dictionary of strings is a dictionary of objects, and
        // strings are property-list values, as `userInfo` requires.
        unsafe { content.setUserInfo(&Retained::cast_unchecked(info)) };
    }
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&id, &content, None);
    center.addNotificationRequest_withCompletionHandler(&request, None);
}

/// The category for an action button titled `title`, registered with the
/// center (alongside every earlier one) the first time it is used.
fn register_category(center: &UNUserNotificationCenter, title: &str) -> Retained<NSString> {
    let identifier = |title: &str| NSString::from_str(&format!("action:{title}"));
    let added = CATEGORIES.with(|known| {
        let mut known = known.borrow_mut();
        if known.iter().any(|known| known == title) {
            return false;
        }
        known.push(title.to_owned());
        true
    });
    if added {
        let categories: Vec<Retained<UNNotificationCategory>> = CATEGORIES.with(|known| {
            known
                .borrow()
                .iter()
                .map(|title| {
                    let action = UNNotificationAction::actionWithIdentifier_title_options(
                        &NSString::from_str(ACTION),
                        &NSString::from_str(title),
                        UNNotificationActionOptions::empty(),
                    );
                    UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
                        &identifier(title),
                        &NSArray::from_retained_slice(&[action]),
                        &NSArray::new(),
                        UNNotificationCategoryOptions::empty(),
                    )
                })
                .collect()
        });
        center.setNotificationCategories(&NSSet::from_retained_slice(&categories));
    }
    identifier(title)
}
