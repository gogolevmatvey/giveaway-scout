//! Persistent GNOME notifications using the same wire format as Gio.Notification.
//!
//! Unlike Application::send_notification, an acknowledged D-Bus call lets us retain
//! failed deliveries in SQLite. A timeout is retried with the same UUID, replacing
//! the old notification rather than creating another one.
use std::{collections::BTreeMap, path::Path, process::Stdio, time::Duration};

use gio::prelude::*;
use glib::variant::ToVariant;

use crate::notifications::NotificationRequest;

const INTERFACE: &str = "org.gtk.Notifications";
const OBJECT: &str = "/org/gtk/Notifications";
const TIMEOUT_MS: i32 = 3_000;
const SOUND_PLAYER: &str = "/usr/bin/canberra-gtk-play";

pub(crate) fn installed_registration_available(application_id: &str) -> bool {
    let expected_id = "com.giveawayscout.desktop";
    let expected_exe = Path::new("/usr/bin/giveaway-scout");
    let executable_matches = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .ok()
        .zip(std::fs::canonicalize(expected_exe).ok())
        .is_some_and(|(actual, expected)| actual == expected);
    let desktop_matches = gio::DesktopAppInfo::new(&format!("{application_id}.desktop"))
        .is_some_and(|desktop| desktop.boolean("DBusActivatable"));
    let service_matches = std::fs::read_to_string(format!(
        "/usr/share/dbus-1/services/{application_id}.service"
    ))
    .ok()
    .is_some_and(|text| {
        text.lines()
            .any(|line| line == format!("Name={application_id}"))
            && text.lines().any(|line| {
                line == format!(
                    "Exec=/usr/bin/giveaway-scout {}",
                    crate::linux_activation::SERVICE_ARGUMENT
                )
            })
    });
    registration_matches(
        tauri::is_dev(),
        application_id == expected_id,
        executable_matches,
        desktop_matches,
        service_matches,
    )
}

fn registration_matches(
    dev: bool,
    identity: bool,
    executable: bool,
    desktop: bool,
    service: bool,
) -> bool {
    !dev && identity && executable && desktop && service
}

fn notification_payload(
    request: &NotificationRequest,
    activation_url: &str,
) -> Result<(String, BTreeMap<&'static str, glib::Variant>), String> {
    let url = url::Url::parse(activation_url).map_err(|error| error.to_string())?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("notification activation must contain only a UUID or show action".to_owned());
    }
    let (id, action, target) = match crate::deep_link_action(&url) {
        Some(crate::DeepLinkAction::Notification(id)) => {
            let id = uuid::Uuid::parse_str(&id)
                .map_err(|error| error.to_string())?
                .to_string();
            (id.clone(), "app.open-notification", Some(id.to_variant()))
        }
        Some(crate::DeepLinkAction::Show) => ("test".to_owned(), "app.show", None),
        None => return Err("unsupported notification activation".to_owned()),
    };
    // GTK's body is plain text, not the freedesktop body-markup format.
    let mut values = BTreeMap::from([
        ("title", request.title.to_variant()),
        ("body", request.body.to_variant()),
        ("priority", "normal".to_variant()),
        ("default-action", action.to_variant()),
    ]);
    let mut button = BTreeMap::from([
        ("label", request.action_label.to_variant()),
        ("action", action.to_variant()),
    ]);
    if let Some(target) = target {
        button.insert("target", target.clone());
        values.insert("default-action-target", target);
    }
    values.insert("buttons", vec![button].to_variant());
    let icon = gio::BytesIcon::new(&glib::Bytes::from_static(include_bytes!(
        "../icons/icon.png"
    )));
    if let Some(icon) = icon.serialize() {
        values.insert("icon", icon);
    }
    Ok((id, values))
}

pub(crate) fn try_show(
    connection: &gio::DBusConnection,
    application_id: &str,
    request: &NotificationRequest,
    activation_url: &str,
) -> Result<bool, String> {
    let (id, values) = notification_payload(request, activation_url)?;
    let available = connection
        .call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            Some(&(INTERFACE,).to_variant()),
            Some(glib::VariantTy::new("(b)").unwrap()),
            gio::DBusCallFlags::NO_AUTO_START,
            TIMEOUT_MS,
            None::<&gio::Cancellable>,
        )
        .map_err(|error| error.to_string())?
        .get::<(bool,)>()
        .ok_or_else(|| "invalid notification service availability reply".to_owned())?
        .0;
    if !available {
        return Ok(false);
    }
    // Resolve the unique owner first: an owner change must not silently redirect
    // the send or cause a second notification through another backend.
    let owner = match connection.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "GetNameOwner",
        Some(&(INTERFACE,).to_variant()),
        Some(glib::VariantTy::new("(s)").unwrap()),
        gio::DBusCallFlags::NO_AUTO_START,
        TIMEOUT_MS,
        None::<&gio::Cancellable>,
    ) {
        Ok(reply) => reply
            .get::<(String,)>()
            .map(|value| value.0)
            .ok_or_else(|| "invalid notification service owner reply".to_owned())?,
        Err(error) => return Err(error.to_string()),
    };
    connection
        .call_sync(
            Some(&owner),
            OBJECT,
            INTERFACE,
            "AddNotification",
            Some(&(application_id, id.as_str(), values).to_variant()),
            Some(glib::VariantTy::UNIT),
            gio::DBusCallFlags::NO_AUTO_START,
            TIMEOUT_MS,
            None::<&gio::Cancellable>,
        )
        .map_err(|error| error.to_string())?;
    if request.play_sound && sound_policy_allows(application_id) && screen_is_unlocked(connection) {
        play_sound();
    }
    Ok(true)
}

fn setting_enabled(schema_id: &str, key: &str, path: Option<&str>) -> bool {
    // Missing schemas are not an excuse to bypass an unknown desktop's policies.
    let Some(schema) =
        gio::SettingsSchemaSource::default().and_then(|source| source.lookup(schema_id, true))
    else {
        return false;
    };
    schema.has_key(key)
        && gio::Settings::new_full(&schema, None::<&gio::SettingsBackend>, path).boolean(key)
}

fn sound_policy_allows(application_id: &str) -> bool {
    let canonical_id: String = application_id
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect();
    let path = format!("/org/gnome/desktop/notifications/application/{canonical_id}/");
    setting_enabled("org.gnome.desktop.notifications", "show-banners", None)
        && setting_enabled("org.gnome.desktop.sound", "event-sounds", None)
        && setting_enabled(
            "org.gnome.desktop.notifications.application",
            "enable",
            Some(&path),
        )
        && setting_enabled(
            "org.gnome.desktop.notifications.application",
            "enable-sound-alerts",
            Some(&path),
        )
        && setting_enabled(
            "org.gnome.desktop.notifications.application",
            "show-banners",
            Some(&path),
        )
}

fn screen_is_unlocked(connection: &gio::DBusConnection) -> bool {
    connection
        .call_sync(
            Some("org.gnome.ScreenSaver"),
            "/org/gnome/ScreenSaver",
            "org.gnome.ScreenSaver",
            "GetActive",
            None,
            Some(glib::VariantTy::new("(b)").unwrap()),
            gio::DBusCallFlags::NO_AUTO_START,
            TIMEOUT_MS,
            None::<&gio::Cancellable>,
        )
        .ok()
        .and_then(|reply| reply.get::<(bool,)>())
        .is_some_and(|(active,)| !active)
}

fn sound_command() -> std::process::Command {
    let mut command = std::process::Command::new(SOUND_PLAYER);
    // Constant arguments, no shell, no untrusted site text or executable from PATH.
    command
        .args([
            "--id",
            "message-new-instant",
            "--description",
            "Giveaway Scout",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn play_sound() {
    // GTK notifications have no sound field. Use the desktop event-sound player
    // only after acknowledgement and policy checks, never replay the toast on a
    // sound failure. Bound the helper's lifetime and always reap the process.
    std::thread::spawn(|| {
        let mut child = match sound_command().spawn() {
            Ok(child) => child,
            Err(error) => {
                log::warn!("could not start Linux notification event sound: {error}");
                return;
            }
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        log::warn!("Linux notification event sound exited with {status}");
                    }
                    return;
                }
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                result => {
                    log::warn!("Linux notification sound helper timed out or failed: {result:?}");
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "550e8400-e29b-41d4-a716-446655440000";

    fn request() -> NotificationRequest {
        NotificationRequest {
            title: "<b>Title</b> & example".to_owned(),
            body: "<a href='https://untrusted.invalid/'>Offer</a> & text".to_owned(),
            action_label: "Открыть".to_owned(),
            play_sound: false,
        }
    }

    #[test]
    fn persistent_delivery_requires_the_installed_release_registration() {
        assert!(registration_matches(false, true, true, true, true));
        for index in 0..5 {
            let mut flags = [false, true, true, true, true];
            flags[index] = !flags[index];
            assert!(!registration_matches(
                flags[0], flags[1], flags[2], flags[3], flags[4]
            ));
        }
    }

    #[test]
    fn payload_keeps_plain_text_uuid_and_the_application_icon() {
        let request = request();
        let (id, values) = notification_payload(
            &request,
            &format!("giveaway-scout://notification/{}", ID.to_uppercase()),
        )
        .unwrap();
        assert_eq!(id, ID);
        assert_eq!(values["title"].str(), Some(request.title.as_str()));
        assert_eq!(values["body"].str(), Some(request.body.as_str()));
        assert_eq!(
            values["default-action"].str(),
            Some("app.open-notification")
        );
        assert_eq!(values["default-action-target"].str(), Some(ID));
        assert!(values.contains_key("icon"));
        let buttons = values["buttons"]
            .get::<Vec<BTreeMap<String, glib::Variant>>>()
            .unwrap();
        assert_eq!(buttons.len(), 1);
        assert_eq!(buttons[0]["target"].str(), Some(ID));
        assert_eq!(buttons[0]["action"], values["default-action"]);
        assert_eq!(buttons[0]["label"].str(), Some("Открыть"));
    }

    #[test]
    fn test_notification_uses_an_action_without_a_database_record() {
        let (id, values) = notification_payload(&request(), "giveaway-scout://show").unwrap();
        assert_eq!(id, "test");
        assert_eq!(values["default-action"].str(), Some("app.show"));
        assert!(!values.contains_key("default-action-target"));
    }

    #[test]
    fn invalid_activation_is_rejected_before_any_bus_call() {
        for url in [
            "https://untrusted.invalid",
            "giveaway-scout://notification/not-a-uuid",
            "giveaway-scout://show?url=other",
            "giveaway-scout://show#fragment",
            "giveaway-scout://user@show",
            "giveaway-scout://show:123",
        ] {
            assert!(notification_payload(&request(), url).is_err(), "{url}");
        }
    }

    #[test]
    fn sound_player_has_only_constant_arguments_and_an_absolute_path() {
        let command = sound_command();
        assert_eq!(command.get_program(), SOUND_PLAYER);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                "--id",
                "message-new-instant",
                "--description",
                "Giveaway Scout"
            ]
        );
    }

    #[test]
    #[ignore = "run scripts/check-linux-persistent-notifications.py on a private D-Bus session"]
    fn persistent_dbus_fixture() {
        let original =
            std::env::var("GIVEAWAY_SCOUT_ORIGINAL_BUS").expect("use the private-bus script");
        assert_ne!(std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap(), original);
        let case = std::env::var("GIVEAWAY_SCOUT_PERSISTENT_CASE").unwrap();
        let connection =
            gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>).unwrap();
        let url = if case == "show" {
            "giveaway-scout://show".to_owned()
        } else {
            format!("giveaway-scout://notification/{ID}")
        };
        let result = try_show(
            &connection,
            "com.giveawayscout.desktop.activationtest",
            &request(),
            &url,
        );
        match case.as_str() {
            "absent" => assert!(!result.unwrap()),
            "rejected" => assert!(result.unwrap_err().contains("test rejection")),
            "timeout" => assert!(
                result.is_err(),
                "a missing acknowledgement must remain pending"
            ),
            "repeat" => {
                assert!(result.unwrap());
                assert!(
                    try_show(
                        &connection,
                        "com.giveawayscout.desktop.activationtest",
                        &request(),
                        &url
                    )
                    .unwrap()
                );
            }
            "uuid" | "show" => assert!(result.unwrap()),
            _ => panic!("unexpected test case"),
        }
    }

    #[test]
    #[ignore = "run scripts/check-linux-persistent-notifications.py with an isolated settings backend"]
    fn sound_policy_fixture() {
        assert_eq!(std::env::var("GSETTINGS_BACKEND").unwrap(), "memory");
        let original = std::env::var("GIVEAWAY_SCOUT_ORIGINAL_BUS").unwrap();
        assert_ne!(std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap(), original);
        let connection =
            gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>).unwrap();
        let app = "com.giveawayscout.desktop.activationtest";
        let master = gio::Settings::new("org.gnome.desktop.notifications");
        let sound = gio::Settings::new("org.gnome.desktop.sound");
        let local = gio::Settings::with_path(
            "org.gnome.desktop.notifications.application",
            "/org/gnome/desktop/notifications/application/com-giveawayscout-desktop-activationtest/",
        );
        for (settings, key) in [
            (&master, "show-banners"),
            (&sound, "event-sounds"),
            (&local, "enable"),
            (&local, "enable-sound-alerts"),
            (&local, "show-banners"),
        ] {
            settings.set_boolean(key, true).unwrap();
        }
        assert!(sound_policy_allows(app));
        for (settings, key) in [
            (&master, "show-banners"),
            (&sound, "event-sounds"),
            (&local, "enable"),
            (&local, "enable-sound-alerts"),
            (&local, "show-banners"),
        ] {
            settings.set_boolean(key, false).unwrap();
            assert!(!sound_policy_allows(app), "{key} must disable sound");
            settings.set_boolean(key, true).unwrap();
            assert!(sound_policy_allows(app));
        }
        assert!(!setting_enabled(
            "com.giveawayscout.nonexistent.schema",
            "unknown",
            None
        ));
        assert!(!setting_enabled("org.gnome.desktop.sound", "unknown", None));
        let unlocked = std::env::var("GIVEAWAY_SCOUT_LOCK_CASE").unwrap() == "unlocked";
        assert_eq!(screen_is_unlocked(&connection), unlocked);
    }
}
