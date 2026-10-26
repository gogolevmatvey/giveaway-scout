pub mod activation;
#[cfg(target_os = "macos")]
pub mod macos;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationRequest {
    pub title: String,
    pub body: String,
    pub action_label: String,
    pub play_sound: bool,
}
