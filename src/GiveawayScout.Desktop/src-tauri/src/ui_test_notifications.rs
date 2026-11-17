pub use giveaway_scout_notifications::NotificationRequest;

// This adapter replaces native transport in UI tests; production still uses notify-rust.
use notify_rust as _;

pub fn show(
    _app: &tauri::AppHandle,
    request: &NotificationRequest,
    activation_url: Option<&str>,
    _on_activated: impl FnOnce() + Send + 'static,
) -> Result<(), String> {
    crate::ui_tests::record(crate::ui_tests::Event::Notification {
        title: &request.title,
        activation: activation_url,
        sound: request.play_sound,
    })
}
