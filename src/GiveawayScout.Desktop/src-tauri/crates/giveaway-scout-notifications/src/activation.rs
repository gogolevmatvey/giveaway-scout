//! Platform-neutral validation and startup hand-off for native notification actions.
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

pub const TEST_NOTIFICATION_ID: &str = "giveaway-scout-test";
pub const OPEN_ACTION: &str = "giveaway-scout-open";
pub const APPLE_DEFAULT_ACTION: &str = "com.apple.UNNotificationDefaultActionIdentifier";

pub fn bundle_identity_matches(expected: &str, actual: Option<&str>, path: &str) -> bool {
    actual == Some(expected) && path.ends_with(".app")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Activation {
    Show,
    Notification(String),
}

pub fn response_action(action: &str, identifier: &str) -> Option<Activation> {
    if !matches!(action, OPEN_ACTION | APPLE_DEFAULT_ACTION) {
        return None;
    }
    if identifier == TEST_NOTIFICATION_ID {
        return Some(Activation::Show);
    }
    uuid::Uuid::parse_str(identifier)
        .ok()
        .map(|id| Activation::Notification(id.to_string()))
}

pub fn request_id(activation_url: &str) -> Result<String, String> {
    let url = url::Url::parse(activation_url).map_err(|error| error.to_string())?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("notification activation must contain only a UUID or show action".to_owned());
    }
    let segments = url
        .path_segments()
        .map(|segments| {
            segments
                .filter(|segment| !segment.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    match (url.scheme(), url.host_str(), segments.as_slice()) {
        ("giveaway-scout", Some("show"), []) => Ok(TEST_NOTIFICATION_ID.to_owned()),
        ("giveaway-scout", Some("notification"), [id]) => uuid::Uuid::parse_str(id)
            .map(|id| id.to_string())
            .map_err(|error| error.to_string()),
        _ => Err("unsupported notification activation".to_owned()),
    }
}

type Handler = Arc<dyn Fn(Activation) + Send + Sync>;

#[derive(Default)]
struct State {
    pending: VecDeque<Activation>,
    handler: Option<Handler>,
    draining: bool,
    opened_from_notification: bool,
}

#[derive(Default)]
pub struct ActivationRouter(Mutex<State>);

impl ActivationRouter {
    pub fn push(&self, action: Activation) {
        {
            let mut state = self
                .0
                .lock()
                .expect("notification activation lock poisoned");
            state.opened_from_notification |= matches!(action, Activation::Notification(_));
            state.pending.push_back(action);
        }
        self.drain();
    }

    pub fn attach(&self, handler: impl Fn(Activation) + Send + Sync + 'static) {
        self.0
            .lock()
            .expect("notification activation lock poisoned")
            .handler = Some(Arc::new(handler));
        self.drain();
    }

    pub fn opened_from_notification(&self) -> bool {
        self.0
            .lock()
            .expect("notification activation lock poisoned")
            .opened_from_notification
    }

    fn drain(&self) {
        {
            let mut state = self
                .0
                .lock()
                .expect("notification activation lock poisoned");
            if state.draining || state.handler.is_none() {
                return;
            }
            state.draining = true;
        }
        loop {
            let next = {
                let mut state = self
                    .0
                    .lock()
                    .expect("notification activation lock poisoned");
                match state.pending.pop_front() {
                    Some(action) => Some((state.handler.as_ref().unwrap().clone(), action)),
                    None => {
                        state.draining = false;
                        None
                    }
                }
            };
            let Some((handler, action)) = next else {
                return;
            };
            // No lock crosses a callback; reentrant pushes stay FIFO and run once.
            handler(action);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "550e8400-e29b-41d4-a716-446655440000";

    #[test]
    fn only_open_or_default_action_with_a_valid_id_can_activate() {
        for action in [OPEN_ACTION, APPLE_DEFAULT_ACTION] {
            assert_eq!(
                response_action(action, &ID.to_uppercase()),
                Some(Activation::Notification(ID.to_owned()))
            );
            assert_eq!(
                response_action(action, TEST_NOTIFICATION_ID),
                Some(Activation::Show)
            );
            for id in [
                "",
                "https://untrusted.invalid/",
                "../settings.json",
                "giveaway-scout://show",
            ] {
                assert_eq!(response_action(action, id), None);
            }
        }
        for action in [
            "com.apple.UNNotificationDismissActionIdentifier",
            "unexpected",
            "",
        ] {
            assert_eq!(response_action(action, ID), None);
            assert_eq!(response_action(action, TEST_NOTIFICATION_ID), None);
        }
    }

    #[test]
    fn native_request_contains_a_uuid_not_an_external_url() {
        assert_eq!(
            request_id(&format!(
                "giveaway-scout://notification/{}",
                ID.to_uppercase()
            ))
            .unwrap(),
            ID
        );
        assert_eq!(
            request_id("giveaway-scout://show").unwrap(),
            TEST_NOTIFICATION_ID
        );
        for value in [
            "https://untrusted.invalid/",
            "giveaway-scout://show?url=other",
            "giveaway-scout://show#fragment",
            "giveaway-scout://user@show",
            "giveaway-scout://show:123",
            "giveaway-scout://notification/invalid",
        ] {
            assert!(request_id(value).is_err());
        }
    }

    #[test]
    fn native_identity_requires_our_own_bundle_and_profile() {
        let id = "com.giveawayscout.desktop";
        assert!(bundle_identity_matches(
            id,
            Some(id),
            "/Applications/Giveaway Scout.app"
        ));
        assert!(!bundle_identity_matches(
            id,
            Some("com.apple.Terminal"),
            "/System/Terminal.app"
        ));
        assert!(!bundle_identity_matches(id, Some(id), "/target/debug"));
        assert!(!bundle_identity_matches(
            id,
            None,
            "/Applications/Giveaway Scout.app"
        ));
        assert!(!bundle_identity_matches(
            &format!("{id}.dev"),
            Some(id),
            "/Applications/Giveaway Scout.app"
        ));
    }

    #[test]
    fn cold_actions_wait_for_storage_and_warm_actions_are_immediate() {
        let router = ActivationRouter::default();
        let seen = Arc::new(Mutex::new(Vec::new()));
        router.push(Activation::Notification(ID.to_owned()));
        router.push(Activation::Show);
        assert!(seen.lock().unwrap().is_empty());
        assert!(router.opened_from_notification());
        let output = Arc::clone(&seen);
        router.attach(move |action| output.lock().unwrap().push(action));
        router.push(Activation::Show);
        assert_eq!(
            *seen.lock().unwrap(),
            [
                Activation::Notification(ID.to_owned()),
                Activation::Show,
                Activation::Show
            ]
        );
    }

    #[test]
    fn reentrant_dispatch_neither_deadlocks_nor_loses_a_notification() {
        let router = Arc::new(ActivationRouter::default());
        let seen = Arc::new(Mutex::new(Vec::new()));
        router.push(Activation::Notification(ID.to_owned()));
        let input = Arc::downgrade(&router);
        let output = Arc::clone(&seen);
        router.attach(move |action| {
            output.lock().unwrap().push(action.clone());
            if matches!(action, Activation::Notification(_)) {
                input.upgrade().unwrap().push(Activation::Show);
            }
        });
        assert_eq!(
            *seen.lock().unwrap(),
            [Activation::Notification(ID.to_owned()), Activation::Show]
        );
    }

    #[test]
    fn simultaneous_callbacks_are_serialized_and_each_action_is_delivered_once() {
        use std::sync::{
            Barrier,
            atomic::{AtomicUsize, Ordering},
        };
        let router = Arc::new(ActivationRouter::default());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let running = Arc::new(AtomicUsize::new(0));
        let output = Arc::clone(&seen);
        let active = Arc::clone(&running);
        router.attach(move |action| {
            assert_eq!(active.fetch_add(1, Ordering::SeqCst), 0);
            output.lock().unwrap().push(action);
            assert_eq!(active.fetch_sub(1, Ordering::SeqCst), 1);
        });
        let barrier = Arc::new(Barrier::new(8));
        std::thread::scope(|scope| {
            for index in 0..8 {
                let input = Arc::clone(&router);
                let start = Arc::clone(&barrier);
                scope.spawn(move || {
                    start.wait();
                    input.push(Activation::Notification(format!("test-{index}")));
                });
            }
        });
        let mut ids: Vec<_> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|action| format!("{action:?}"))
            .collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 8);
        assert_eq!(seen.lock().unwrap().len(), 8);
    }
}
