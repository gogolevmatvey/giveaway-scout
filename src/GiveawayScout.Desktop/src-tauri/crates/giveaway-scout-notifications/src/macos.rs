//! Native UserNotifications adapter. Registered before AppKit finishes launching.
use std::{
    cell::RefCell,
    ptr::NonNull,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use block2::{DynBlock, RcBlock};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, define_class, msg_send,
    rc::{Retained, autoreleasepool},
    runtime::{Bool, ProtocolObject},
};
use objc2_foundation::{NSArray, NSBundle, NSError, NSObject, NSObjectProtocol, NSSet, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationAction, UNNotificationActionOptions, UNNotificationCategory,
    UNNotificationCategoryOptions, UNNotificationDefaultActionIdentifier,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNNotificationSettings, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};

use crate::{
    NotificationRequest,
    activation::{self as notification_activation, Activation, ActivationRouter, OPEN_ACTION},
};

const BUNDLE_REQUIRED: &str = "MACOS_NOTIFICATION_APP_BUNDLE_REQUIRED";
const PERMISSION_DENIED: &str = "MACOS_NOTIFICATION_PERMISSION_DENIED";
const PERMISSION_PENDING: &str = "MACOS_NOTIFICATION_PERMISSION_PENDING";
const CATEGORY_RU: &str = "giveaway-scout-open-ru";
const CATEGORY_EN: &str = "giveaway-scout-open-en";
static INITIALIZED: OnceLock<Result<(), String>> = OnceLock::new();
static ROUTER: OnceLock<Arc<ActivationRouter>> = OnceLock::new();
static AUTHORIZATION_PENDING: AtomicBool = AtomicBool::new(false);

thread_local! {
    // UNUserNotificationCenter.delegate is weak. Keep the delegate alive until exit.
    static DELEGATE: RefCell<Option<Retained<NotificationDelegate>>> = const { RefCell::new(None) };
}

struct DelegateIvars {
    router: Arc<ActivationRouter>,
}

fn presentation_options(play_sound: bool) -> UNNotificationPresentationOptions {
    let mut options =
        UNNotificationPresentationOptions::Banner | UNNotificationPresentationOptions::List;
    if play_sound {
        options |= UNNotificationPresentationOptions::Sound;
    }
    options
}

fn notification_content(request: &NotificationRequest) -> Retained<UNMutableNotificationContent> {
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(&request.title));
    content.setBody(&NSString::from_str(&request.body));
    content.setCategoryIdentifier(&NSString::from_str(if request.action_label == "Открыть" {
        CATEGORY_RU
    } else {
        CATEGORY_EN
    }));
    if request.play_sound {
        content.setSound(Some(&UNNotificationSound::defaultSound()));
    } else {
        content.setSound(None);
    }
    content
}

define_class!(
    // SAFETY: NSObject has no additional subclassing requirements. Our ivars use
    // a mutex because UserNotifications may invoke delegate methods off-main-thread.
    #[unsafe(super = NSObject)]
    #[name = "GiveawayScoutNotificationDelegate"]
    #[ivars = DelegateIvars]
    struct NotificationDelegate;

    // SAFETY: NSObjectProtocol imposes no additional requirements.
    unsafe impl NSObjectProtocol for NotificationDelegate {}

    // SAFETY: Selectors and signatures match Apple's delegate protocol; every
    // completion handler is called exactly once, including ignored responses.
    unsafe impl UNUserNotificationCenterDelegate for NotificationDelegate {
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion: &DynBlock<dyn Fn()>,
        ) {
            let native_action = response.actionIdentifier();
            // SAFETY: This immutable NSString constant is supplied by the linked
            // UserNotifications framework. Never guess the native constant's value.
            let action = if &*native_action == unsafe { UNNotificationDefaultActionIdentifier } {
                notification_activation::APPLE_DEFAULT_ACTION.to_owned()
            } else {
                native_action.to_string()
            };
            let id = response.notification().request().identifier().to_string();
            if let Some(action) = notification_activation::response_action(&action, &id) {
                self.ivars().router.push(action);
            }
            // The validated action is handed to the app queue, not an untrusted
            // URL. Cold responses remain queued until SQLite is ready.
            completion.call(());
        }

        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            notification: &UNNotification,
            completion: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            completion.call((presentation_options(
                notification.request().content().sound().is_some(),
            ),));
        }
    }
);

impl NotificationDelegate {
    fn new(router: Arc<ActivationRouter>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars { router });
        // SAFETY: Superclass NSObject's designated initializer; ivars are set.
        unsafe { msg_send![super(this), init] }
    }
}

fn router() -> &'static Arc<ActivationRouter> {
    ROUTER.get_or_init(|| Arc::new(ActivationRouter::default()))
}

pub fn register_early(expected_id: &str) -> Result<(), String> {
    INITIALIZED.get_or_init(|| autoreleasepool(|_| {
        if MainThreadMarker::new().is_none() { return Err("macOS notification initialization must run on the main thread".to_owned()); }
        let bundle = NSBundle::mainBundle();
        let id = bundle.bundleIdentifier().map(|id| id.to_string());
        if !notification_activation::bundle_identity_matches(expected_id, id.as_deref(), &bundle.bundlePath().to_string()) {
            return Err(BUNDLE_REQUIRED.to_owned());
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let delegate = NotificationDelegate::new(Arc::clone(router()));
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        let categories = [(CATEGORY_RU, "Открыть"), (CATEGORY_EN, "Open")].map(|(id, label)| {
            let action = UNNotificationAction::actionWithIdentifier_title_options(
                &NSString::from_str(OPEN_ACTION), &NSString::from_str(label), UNNotificationActionOptions::Foreground,
            );
            UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
                &NSString::from_str(id), &NSArray::from_retained_slice(&[action]), &NSArray::<NSString>::new(), UNNotificationCategoryOptions::empty(),
            )
        });
        center.setNotificationCategories(&NSSet::from_retained_slice(&categories));
        DELEGATE.with(|state| *state.borrow_mut() = Some(delegate));
        Ok(())
    })).clone()
}

pub fn attach(handler: impl Fn(Activation) + Send + Sync + 'static) {
    router().attach(handler);
}

pub fn opened_from_notification() -> bool {
    router().opened_from_notification()
}

fn native_error(error: *mut NSError) -> Option<String> {
    // SAFETY: The framework supplies a nullable NSError valid for the duration
    // of its callback. Only an owned Rust string leaves the callback.
    unsafe { error.as_ref() }.map(|error| error.localizedDescription().to_string())
}

fn authorize(center: &UNUserNotificationCenter) -> Result<(), String> {
    let (sender, receiver) = mpsc::channel();
    let completion = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
        // SAFETY: Framework-owned settings are valid during this callback.
        let status = unsafe { settings.as_ref() }.authorizationStatus();
        let _ = sender.send(status);
    });
    center.getNotificationSettingsWithCompletionHandler(&completion);
    let status = receiver
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| "macOS notification settings timed out".to_owned())?;
    match status {
        UNAuthorizationStatus::Authorized | UNAuthorizationStatus::Provisional => return Ok(()),
        UNAuthorizationStatus::NotDetermined => {}
        _ => return Err(PERMISSION_DENIED.to_owned()),
    }
    if AUTHORIZATION_PENDING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(PERMISSION_PENDING.to_owned());
    }
    let (sender, receiver) = mpsc::channel();
    let completion = RcBlock::new(move |granted: Bool, error: *mut NSError| {
        let result = native_error(error).map_or_else(
            || {
                if granted.as_bool() {
                    Ok(())
                } else {
                    Err(PERMISSION_DENIED.to_owned())
                }
            },
            Err,
        );
        AUTHORIZATION_PENDING.store(false, Ordering::Release);
        let _ = sender.send(result);
    });
    // Ask only for ordinary alerts/sound, never critical alerts or remote push.
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &completion,
    );
    // Timing out does not enqueue a late notification. The native callback only
    // resolves authorization; later checks can retry once the user has answered.
    receiver
        .recv_timeout(Duration::from_secs(30))
        .map_err(|_| PERMISSION_PENDING.to_owned())?
}

pub fn show(request: &NotificationRequest, activation_url: &str) -> Result<(), String> {
    INITIALIZED
        .get()
        .ok_or_else(|| "macOS notifications are not initialized".to_owned())?
        .clone()?;
    // Never block AppKit on permission dialogs or completion callbacks.
    if MainThreadMarker::new().is_some() {
        return Err("macOS notification delivery must run off the main thread".to_owned());
    }
    let identifier = notification_activation::request_id(activation_url)?;
    autoreleasepool(|_| {
        let center = UNUserNotificationCenter::currentNotificationCenter();
        authorize(&center)?;
        let content = notification_content(request);
        let notification = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&identifier),
            &content,
            None,
        );
        let (sender, receiver) = mpsc::channel();
        let completion = RcBlock::new(move |error: *mut NSError| {
            let _ = sender.send(native_error(error).map_or(Ok(()), Err));
        });
        center.addNotificationRequest_withCompletionHandler(&notification, Some(&completion));
        receiver
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "macOS notification delivery acknowledgement timed out".to_owned())?
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> NotificationRequest {
        NotificationRequest {
            title: "<b>Giveaway</b> & title".to_owned(),
            body: "Source: <a href='https://untrusted.invalid/'>Example</a>".to_owned(),
            action_label: "Открыть".to_owned(),
            play_sound: false,
        }
    }

    #[test]
    fn native_content_keeps_literal_text_and_has_no_activation_url_in_user_info() {
        autoreleasepool(|_| {
            let request = request();
            let content = notification_content(&request);
            assert_eq!(content.title().to_string(), request.title);
            assert_eq!(content.body().to_string(), request.body);
            assert_eq!(content.userInfo().count(), 0);
        });
    }

    #[test]
    fn native_sound_and_foreground_options_follow_the_setting() {
        autoreleasepool(|_| {
            let mut request = request();
            for play_sound in [false, true] {
                request.play_sound = play_sound;
                assert_eq!(notification_content(&request).sound().is_some(), play_sound);
                let options = presentation_options(play_sound);
                assert!(options.contains(
                    UNNotificationPresentationOptions::Banner
                        | UNNotificationPresentationOptions::List
                ));
                assert_eq!(
                    options.contains(UNNotificationPresentationOptions::Sound),
                    play_sound
                );
            }
        });
    }

    #[test]
    fn native_content_selects_the_localized_action_category() {
        autoreleasepool(|_| {
            let mut request = request();
            for (label, category) in [("Открыть", CATEGORY_RU), ("Open", CATEGORY_EN)] {
                request.action_label = label.to_owned();
                assert_eq!(
                    notification_content(&request)
                        .categoryIdentifier()
                        .to_string(),
                    category
                );
            }
        });
    }
}
