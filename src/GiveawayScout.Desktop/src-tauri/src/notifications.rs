#[cfg(not(target_os = "macos"))]
use notify_rust::{Notification, NotificationResponse};

#[cfg(target_os = "windows")]
pub const APPLICATION_ID: &str = "com.giveawayscout.desktop";

#[cfg(target_os = "windows")]
const WINDOWS_NOTIFICATION_ICON: &[u8] = include_bytes!("../icons/icon.png");

#[cfg(target_os = "windows")]
const WINDOWS_NOTIFICATION_ICON_FILE_NAME: &str = "notification-icon.png";

#[cfg(target_os = "linux")]
const LINUX_DESKTOP_ENTRY: &str = "Giveaway Scout";

#[derive(Clone, Copy)]
#[cfg(not(target_os = "macos"))]
struct CallbackCapabilities {
    actions: bool,
    body_markup: bool,
}

pub use giveaway_scout_notifications::NotificationRequest;

pub fn show(
    _app: &tauri::AppHandle,
    request: &NotificationRequest,
    activation_url: Option<&str>,
    on_activated: impl FnOnce() + Send + 'static,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let _ = on_activated;
        crate::macos_notifications::show(
            request,
            activation_url.ok_or_else(|| "missing notification activation".to_owned())?,
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        #[cfg(target_os = "linux")]
        {
            use tauri::Manager;
            if let Some(service) = _app.try_state::<crate::linux_activation::NotificationService>()
                && service.persistent
                && let Some(url) = activation_url
                && crate::linux_notifications::try_show(
                    &service.connection,
                    &service.application_id,
                    request,
                    url,
                )?
            {
                return Ok(());
            }
        }

        #[cfg(target_os = "windows")]
        {
            ensure_windows_application_identity()?;
        }

        #[cfg(target_os = "windows")]
        if let Some(activation_url) = activation_url {
            let executable =
                tauri::utils::platform::current_exe().map_err(|error| error.to_string())?;
            let directory = executable
                .parent()
                .ok_or_else(|| "application executable has no parent directory".to_owned())?;
            if protocol_activation_for_installation(
                directory,
                crate::windows_installation::notification_protocol_available(_app),
            ) {
                return show_windows_protocol(request, activation_url);
            }
        }

        show_with_callback(request, on_activated)
    }
}

#[cfg(not(target_os = "macos"))]
fn show_with_callback(
    request: &NotificationRequest,
    on_activated: impl FnOnce() + Send + 'static,
) -> Result<(), String> {
    let capabilities = callback_capabilities();
    let mut notification = callback_notification(request, capabilities);
    configure_platform(&mut notification, request.play_sound)?;
    let handle = notification.show().map_err(|error| error.to_string())?;
    if !capabilities.actions {
        // The history remains available when the desktop does not support actions.
        return Ok(());
    }
    std::thread::spawn(move || {
        if let Err(error) = handle.wait_for_response(move |response: &NotificationResponse| {
            activate_response(response, on_activated);
        }) {
            log::warn!("could not wait for notification activation: {error}");
        }
    });

    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn callback_notification(
    request: &NotificationRequest,
    capabilities: CallbackCapabilities,
) -> Notification {
    let mut notification = Notification::new();
    let body = if capabilities.body_markup {
        escape_xml(&request.body)
    } else {
        request.body.clone()
    };
    notification
        .appname("Giveaway Scout")
        .summary(&request.title)
        .body(&body)
        .auto_icon();
    if capabilities.actions {
        #[cfg(target_os = "linux")]
        notification.action("default", &request.action_label);
        notification.action("open", &request.action_label);
    }
    notification
}

#[cfg(not(target_os = "macos"))]
fn activate_response(response: &NotificationResponse, on_activated: impl FnOnce()) {
    if matches!(response, NotificationResponse::Default)
        || matches!(response, NotificationResponse::Action(action) if action == "open" || action == "default")
    {
        on_activated();
    }
}

#[cfg(target_os = "linux")]
fn callback_capabilities() -> CallbackCapabilities {
    match notify_rust::get_capabilities() {
        Ok(capabilities) => CallbackCapabilities {
            actions: capabilities
                .iter()
                .any(|capability| capability == "actions"),
            body_markup: capabilities
                .iter()
                .any(|capability| capability == "body-markup"),
        },
        Err(error) => {
            log::warn!("could not read notification server capabilities: {error}");
            // Conservative fallback: no unusable buttons and no untrusted markup.
            CallbackCapabilities {
                actions: false,
                body_markup: true,
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn callback_capabilities() -> CallbackCapabilities {
    CallbackCapabilities {
        actions: true,
        body_markup: false,
    }
}

#[cfg(target_os = "windows")]
fn show_windows_protocol(
    request: &NotificationRequest,
    activation_url: &str,
) -> Result<(), String> {
    use windows::{
        Data::Xml::Dom::XmlDocument,
        UI::Notifications::{ToastNotification, ToastNotificationManager},
        core::HSTRING,
    };

    let title = escape_xml(&request.title);
    let body = escape_xml(&request.body);
    let action_label = escape_xml(&request.action_label);
    let activation_url = escape_xml(activation_url);
    let audio = if request.play_sound {
        String::new()
    } else {
        "<audio silent=\"true\"/>".to_owned()
    };
    let xml = format!(
        "<toast launch=\"{activation_url}\" activationType=\"protocol\"><visual><binding template=\"ToastGeneric\"><text>{title}</text><text>{body}</text></binding></visual><actions><action content=\"{action_label}\" arguments=\"{activation_url}\" activationType=\"protocol\"/></actions>{audio}</toast>"
    );

    let document = XmlDocument::new().map_err(|error| error.to_string())?;
    document
        .LoadXml(&HSTRING::from(xml))
        .map_err(|error| error.to_string())?;
    let toast =
        ToastNotification::CreateToastNotification(&document).map_err(|error| error.to_string())?;
    let notifier =
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APPLICATION_ID))
            .map_err(|error| error.to_string())?;
    notifier.Show(&toast).map_err(|error| error.to_string())
}

#[cfg(not(target_os = "macos"))]
fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(target_os = "windows")]
fn configure_platform(notification: &mut Notification, play_sound: bool) -> Result<(), String> {
    notification.app_id(APPLICATION_ID);
    if play_sound {
        notification.sound_name("Default");
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn ensure_windows_application_identity() -> Result<(), String> {
    use windows_registry::CURRENT_USER;

    let icon_path = ensure_windows_notification_icon()?;
    let key = CURRENT_USER
        .create(format!(r"SOFTWARE\Classes\AppUserModelId\{APPLICATION_ID}"))
        .map_err(|error| error.to_string())?;
    key.set_string("DisplayName", "Giveaway Scout")
        .map_err(|error| error.to_string())?;
    let icon_uri = icon_path.to_string_lossy();
    key.set_string("IconUri", icon_uri.as_ref())
        .map_err(|error| error.to_string())?;
    key.set_string("IconBackgroundColor", "00000000")
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn ensure_windows_notification_icon() -> Result<std::path::PathBuf, String> {
    let local_app_data = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| "LOCALAPPDATA is not available".to_owned())?;
    let directory = std::path::PathBuf::from(local_app_data).join("Giveaway Scout");
    let icon_path = directory.join(WINDOWS_NOTIFICATION_ICON_FILE_NAME);
    let icon_is_current = std::fs::read(&icon_path)
        .map(|contents| contents == WINDOWS_NOTIFICATION_ICON)
        .unwrap_or(false);

    if !icon_is_current {
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        std::fs::write(&icon_path, WINDOWS_NOTIFICATION_ICON).map_err(|error| error.to_string())?;
    }

    Ok(icon_path)
}

#[cfg(target_os = "windows")]
fn is_rust_build_output(directory: &std::path::Path) -> bool {
    let Some(configuration) = directory.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let Some(target) = directory
        .parent()
        .and_then(std::path::Path::file_name)
        .and_then(|value| value.to_str())
    else {
        return false;
    };

    matches!(
        configuration.to_ascii_lowercase().as_str(),
        "debug" | "release"
    ) && target.to_ascii_lowercase().starts_with("target")
}

#[cfg(target_os = "windows")]
fn protocol_activation_for_installation(
    directory: &std::path::Path,
    current_installation: bool,
) -> bool {
    current_installation && !is_rust_build_output(directory)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn configure_platform(notification: &mut Notification, play_sound: bool) -> Result<(), String> {
    notification.hint(notify_rust::Hint::SuppressSound(!play_sound));
    #[cfg(target_os = "linux")]
    notification.hint(notify_rust::Hint::DesktopEntry(
        LINUX_DESKTOP_ENTRY.to_owned(),
    ));
    if play_sound {
        notification.sound_name("message-new-instant");
    }
    Ok(())
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::{
        CallbackCapabilities, NotificationRequest, activate_response, callback_notification,
    };
    use notify_rust::{CloseReason, NotificationResponse};

    fn sample_request() -> NotificationRequest {
        NotificationRequest {
            title: "<b>Title</b> & example".to_owned(),
            body: "Source: <a href=\"https://untrusted.invalid/\">Offer</a> & 'text'".to_owned(),
            action_label: "Открыть".to_owned(),
            play_sound: false,
        }
    }

    #[test]
    fn body_markup_is_escaped_but_summary_remains_plain_text() {
        let request = sample_request();
        let notification = callback_notification(
            &request,
            CallbackCapabilities {
                actions: true,
                body_markup: true,
            },
        );
        assert_eq!(notification.summary, request.title);
        assert_eq!(
            notification.body,
            "Source: &lt;a href=&quot;https://untrusted.invalid/&quot;&gt;Offer&lt;/a&gt; &amp; &apos;text&apos;"
        );
    }

    #[test]
    fn a_plain_text_server_does_not_receive_xml_entities() {
        let request = sample_request();
        let notification = callback_notification(
            &request,
            CallbackCapabilities {
                actions: false,
                body_markup: false,
            },
        );
        assert_eq!(notification.body, request.body);
        assert!(notification.actions.is_empty());
    }

    #[test]
    fn only_default_and_open_actions_activate_the_notification() {
        for response in [
            NotificationResponse::Default,
            NotificationResponse::Action("open".to_owned()),
            NotificationResponse::Action("default".to_owned()),
        ] {
            let mut calls = 0;
            activate_response(&response, || calls += 1);
            assert_eq!(calls, 1);
        }
    }

    #[test]
    fn dismissal_unknown_actions_and_replies_do_not_open_a_link() {
        for response in [
            NotificationResponse::Closed(CloseReason::Dismissed),
            NotificationResponse::Closed(CloseReason::Expired),
            NotificationResponse::Action("unexpected".to_owned()),
            NotificationResponse::Reply("unexpected".to_owned()),
        ] {
            activate_response(&response, || panic!("unexpected activation: {response:?}"));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_registers_both_the_body_click_and_open_button() {
        let notification = callback_notification(
            &sample_request(),
            CallbackCapabilities {
                actions: true,
                body_markup: false,
            },
        );
        assert_eq!(
            notification.actions,
            ["default", "Открыть", "open", "Открыть"]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_explicitly_suppresses_sound_when_disabled() {
        use notify_rust::Hint;
        for play_sound in [false, true] {
            let mut notification = notify_rust::Notification::new();
            super::configure_platform(&mut notification, play_sound).unwrap();
            assert!(
                notification
                    .hints
                    .contains(&Hint::SuppressSound(!play_sound))
            );
            assert_eq!(
                notification
                    .hints
                    .contains(&Hint::SoundName("message-new-instant".to_owned())),
                play_sound
            );
            let config: serde_json::Value =
                serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
            assert!(notification.hints.contains(&Hint::DesktopEntry(
                config["productName"].as_str().unwrap().to_owned()
            )));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "run scripts/check-linux-notifications.py on a private D-Bus session"]
    fn linux_dbus_round_trip() {
        use std::{sync::mpsc, time::Duration};
        let case = std::env::var("GIVEAWAY_SCOUT_NOTIFICATION_SMOKE")
            .expect("use the private-session test script; never send to the user's desktop bus");
        assert!(matches!(
            case.as_str(),
            "default"
                | "open"
                | "dismissed"
                | "expired"
                | "unknown"
                | "no-actions"
                | "plain-body"
                | "sound-on"
                | "rejected"
                | "capabilities-error"
        ));
        let mut request = sample_request();
        request.play_sound = case == "sound-on";
        let (sender, receiver) = mpsc::channel();
        let result = super::show_with_callback(&request, move || {
            sender.send(()).unwrap();
        });
        if case == "rejected" {
            assert!(result.unwrap_err().contains("test rejection"));
            return;
        }
        result.unwrap();
        let response = receiver.recv_timeout(Duration::from_secs(5));
        if matches!(
            case.as_str(),
            "default" | "open" | "plain-body" | "sound-on"
        ) {
            response.expect("the notification action should activate its callback");
        } else {
            assert_eq!(
                response,
                Err(mpsc::RecvTimeoutError::Disconnected),
                "dismissal/unsupported actions must not activate or leave a waiting callback"
            );
        }
    }

    #[test]
    fn notification_request_keeps_only_display_data() {
        let request = NotificationRequest {
            title: "New giveaway: Test".to_owned(),
            body: "Source: Givee Club".to_owned(),
            action_label: "Open".to_owned(),
            play_sound: true,
        };

        assert!(!request.title.contains("https://"));
        assert_eq!(request.action_label, "Open");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn protocol_toast_text_is_xml_escaped() {
        assert_eq!(
            super::escape_xml("A & <B> \"C\" 'D'"),
            "A &amp; &lt;B&gt; &quot;C&quot; &apos;D&apos;"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn recognizes_standard_and_isolated_rust_build_directories() {
        use std::path::Path;

        assert!(super::is_rust_build_output(Path::new(
            r"C:\source\target\release"
        )));
        assert!(super::is_rust_build_output(Path::new(
            r"C:\source\target-windows-x64\debug"
        )));
        assert!(!super::is_rust_build_output(Path::new(
            r"C:\Program Files\Giveaway Scout"
        )));
        assert!(super::protocol_activation_for_installation(
            Path::new(r"C:\Program Files\Giveaway Scout"),
            true,
        ));
        assert!(!super::protocol_activation_for_installation(
            Path::new(r"C:\source\target\release"),
            true,
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn portable_outside_rust_output_does_not_use_an_unregistered_protocol() {
        assert!(!super::protocol_activation_for_installation(
            std::path::Path::new(r"C:\Downloads\Giveaway Scout"),
            false,
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn another_installed_copy_does_not_make_dist_portable_protocol_capable() {
        assert!(!super::protocol_activation_for_installation(
            std::path::Path::new(r"C:\GitHubRepositories\giveaway-scout\dist\windows-x64"),
            false,
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn embedded_notification_icon_is_a_png() {
        assert!(super::WINDOWS_NOTIFICATION_ICON.starts_with(b"\x89PNG\r\n\x1a\n"));
    }
}
