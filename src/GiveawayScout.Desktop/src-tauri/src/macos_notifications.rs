//! Tauri integration; native framework code lives in its independently checkable crate.
use giveaway_scout_notifications::{activation::Activation, macos as native};

pub(crate) use native::{opened_from_notification, register_early, show};

pub(crate) fn attach(app: &tauri::AppHandle) {
    let app = app.clone();
    native::attach(move |action| {
        let handle = app.clone();
        if let Err(error) = app.run_on_main_thread(move || {
            let result = match action {
                Activation::Show => {
                    crate::show_main_window(&handle).map_err(|error| error.to_string())
                }
                Activation::Notification(id) => {
                    use tauri::Manager;
                    let storage = handle.state::<crate::StorageState>();
                    crate::activate_notification(&handle, storage.inner(), &id)
                }
            };
            if let Err(error) = result {
                log::warn!("could not activate macOS notification: {error}");
            }
        }) {
            log::warn!("could not schedule macOS notification activation: {error}");
        }
    });
}
