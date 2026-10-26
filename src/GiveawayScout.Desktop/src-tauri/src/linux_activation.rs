use std::{cell::RefCell, ffi::OsStr, rc::Rc};

use gio::prelude::*;

pub(crate) const SERVICE_ARGUMENT: &str = "--notification-service";
const OPEN_ACTION: &str = "open-notification";

// DBusConnection is thread-safe; the Application itself stays on the GTK thread.
pub(crate) struct NotificationService {
    pub connection: gio::DBusConnection,
    pub application_id: String,
    pub persistent: bool,
}

thread_local! {
    // Gio objects stay on the GTK main thread, which also dispatches their actions.
    static APPLICATION: RefCell<Option<gio::Application>> = const { RefCell::new(None) };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Activation {
    Show,
    Notification(String),
}

pub(crate) fn is_service_launch<I, S>(arguments: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    arguments
        .into_iter()
        .any(|value| value.as_ref() == SERVICE_ARGUMENT)
}

fn notification_id(value: &str) -> Option<String> {
    // No URL, path or shell command can cross this entry point.
    uuid::Uuid::parse_str(value).ok().map(|id| id.to_string())
}

fn register_actions(
    application_id: &str,
    on_activation: impl Fn(Activation) + 'static,
) -> Result<gio::Application, String> {
    if !gio::Application::id_is_valid(application_id) {
        return Err("invalid D-Bus application ID".to_owned());
    }
    let application = gio::Application::new(Some(application_id), gio::ApplicationFlags::empty());
    let callback = Rc::new(on_activation);
    let show = Rc::clone(&callback);
    application.connect_activate(move |_| show(Activation::Show));
    let show = Rc::clone(&callback);
    let show_action = gio::SimpleAction::new("show", None);
    show_action.connect_activate(move |_, _| show(Activation::Show));
    application.add_action(&show_action);

    let open = gio::SimpleAction::new(OPEN_ACTION, Some(glib::VariantTy::STRING));
    open.connect_activate(move |_, parameter| {
        if let Some(id) = parameter
            .and_then(glib::Variant::str)
            .and_then(notification_id)
        {
            callback(Activation::Notification(id));
        } else {
            log::warn!("ignored invalid notification UUID from D-Bus activation");
        }
    });
    application.add_action(&open);
    // Register actions before acquiring the bus name so queued calls cannot race setup.
    application
        .register(None::<&gio::Cancellable>)
        .map_err(|error| error.to_string())?;
    if application.is_remote() {
        return Err("another process already owns the D-Bus application ID".to_owned());
    }
    Ok(application)
}

pub(crate) fn initialize(app: &tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;
    let handle = app.clone();
    let application = register_actions(&app.config().identifier, move |action| {
        let result = match action {
            Activation::Show => crate::show_main_window(&handle).map_err(|error| error.to_string()),
            Activation::Notification(id) => {
                use tauri::Manager;
                let storage = handle.state::<crate::StorageState>();
                crate::activate_notification(&handle, storage.inner(), &id)
            }
        };
        if let Err(error) = result {
            log::warn!("could not handle Linux application activation: {error}");
        }
    })?;
    let connection = application
        .dbus_connection()
        .ok_or_else(|| "Gio application has no session bus connection".to_owned())?;
    let persistent =
        crate::linux_notifications::installed_registration_available(&app.config().identifier);
    if !persistent {
        log::info!(
            "Linux notification cold activation unavailable for this build/location; using live callbacks"
        );
    }
    app.manage(NotificationService {
        connection,
        application_id: app.config().identifier.clone(),
        persistent,
    });
    APPLICATION.with(|state| *state.borrow_mut() = Some(application));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_launch_requires_the_exact_argument() {
        assert!(is_service_launch(["giveaway-scout", SERVICE_ARGUMENT]));
        assert!(!is_service_launch(["giveaway-scout", "--autostart"]));
        assert!(!is_service_launch(["--notification-service=other"]));
    }

    #[test]
    fn notification_activation_accepts_uuid_but_not_external_input() {
        assert_eq!(
            notification_id("550E8400-E29B-41D4-A716-446655440000").as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
        for value in [
            "",
            "https://example.org",
            "file:///tmp/test",
            "giveaway-scout://show",
            "../settings.json",
            "id; command",
            "550e8400-e29b-41d4-a716-446655440000?url=other",
        ] {
            assert!(notification_id(value).is_none(), "{value}");
        }
    }

    #[test]
    fn deb_service_and_desktop_alias_match_the_application_identity() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let id = config["identifier"].as_str().unwrap();
        let service = include_str!("../linux/com.giveawayscout.desktop.service");
        let desktop = include_str!("../linux/com.giveawayscout.desktop.desktop");
        assert!(service.lines().any(|line| line == format!("Name={id}")));
        assert!(
            service
                .lines()
                .any(|line| line == format!("Exec=/usr/bin/giveaway-scout {SERVICE_ARGUMENT}"))
        );
        assert!(desktop.lines().any(|line| line == "DBusActivatable=true"));
        assert!(desktop.lines().any(|line| line == "NoDisplay=true"));
        assert!(desktop.lines().any(|line| line == "Exec=giveaway-scout"));
        let files = &config["bundle"]["linux"]["deb"]["files"];
        assert_eq!(
            files[format!("/usr/share/dbus-1/services/{id}.service")],
            format!("linux/{id}.service")
        );
        assert_eq!(
            files[format!("/usr/share/applications/{id}.desktop")],
            format!("linux/{id}.desktop")
        );
    }

    #[test]
    #[ignore = "run scripts/check-linux-activation.py on a private D-Bus session"]
    fn dbus_activation_fixture() {
        use std::{cell::Cell, io::Write};
        let original =
            std::env::var("GIVEAWAY_SCOUT_ORIGINAL_BUS").expect("use the private-bus script");
        assert_ne!(std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap(), original);
        let log_path =
            std::env::var("GIVEAWAY_SCOUT_ACTIVATION_LOG").expect("missing isolated test log");
        let append = move |line: &str| {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .unwrap();
            writeln!(file, "{line}").unwrap();
        };
        let write = Rc::new(append);
        let count = Rc::new(Cell::new(0));
        let main_loop = glib::MainLoop::new(None, false);
        let callback_loop = main_loop.clone();
        let callback_count = Rc::clone(&count);
        let callback_write = Rc::clone(&write);
        let application =
            register_actions("com.giveawayscout.desktop.activationtest", move |action| {
                match action {
                    Activation::Show => callback_write("show"),
                    Activation::Notification(id) => callback_write(&format!("notification:{id}")),
                }
                callback_count.set(callback_count.get() + 1);
                if callback_count.get() == 3 {
                    let exit_loop = callback_loop.clone();
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(100),
                        move || exit_loop.quit(),
                    );
                }
            })
            .unwrap();
        write(&format!("ready:{}", std::process::id()));
        let watchdog_loop = main_loop.clone();
        let watchdog = glib::timeout_add_seconds_local_once(15, move || watchdog_loop.quit());
        main_loop.run();
        if count.get() == 3 {
            watchdog.remove();
        }
        assert_eq!(count.get(), 3, "activation fixture timed out");
        drop(application);
    }
}
