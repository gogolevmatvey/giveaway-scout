pub use giveaway_scout_notifications::NotificationRequest;

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
