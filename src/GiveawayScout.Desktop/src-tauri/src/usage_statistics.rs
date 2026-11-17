use tauri::{AppHandle, Manager};

pub(crate) fn start(app: &AppHandle) {
    let runtime = app.state::<crate::MonitorState>().runtime.clone();
    let development = crate::development_profile(&app.config().identifier, tauri::is_dev());
    tauri::async_runtime::spawn(async move {
        giveaway_scout_runtime::usage_statistics::start(
            &runtime,
            env!("CARGO_PKG_VERSION"),
            development,
            cfg!(feature = "ui-tests"),
        );
    });
}
