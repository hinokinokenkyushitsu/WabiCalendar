pub mod calendar;
pub mod cli;
pub mod error;
pub mod fs_atomic;
pub mod ipc;
pub mod sessions;
pub mod settings;
pub mod timer;
pub mod vault;

// The Tauri half. Everything above this line is shared with the `calpo` binary
// and has to keep building with Tauri absent from the dependency tree entirely.
#[cfg(feature = "gui")]
pub mod commands;
#[cfg(feature = "gui")]
pub mod integrations;

#[cfg(feature = "gui")]
use std::time::Duration;

#[cfg(feature = "gui")]
use tauri::{Manager, WindowEvent};

#[cfg(feature = "gui")]
use crate::commands::AppState;
#[cfg(feature = "gui")]
use crate::integrations::tray;

/// How often the backend re-reads its own clock.
///
/// This is what keeps the tray counting and the end-of-segment notification
/// firing while the window is hidden or closed. The frontend polls on its own
/// schedule; both go through `integrations::pump`, which is why they cannot
/// disagree or double-fire.
#[cfg(feature = "gui")]
const TICK: Duration = Duration::from_secs(1);

/// Listen for `calpo start`, or carry on without it.
///
/// Optional in exactly the way the four OS integrations are, and for the same
/// reason: an app that refused to open because a socket file was in a strange
/// state would be worse than an app without the shortcut. `calpo` finding nobody
/// home runs its own timer instead, so the cost of this failing is a pomodoro
/// the app's window does not show, not a lost one.
///
/// Unlike those four this reports on stderr rather than into `IntegrationStatus`
/// — the settings panel is about what the *user's machine* will let the app do,
/// and this is about whether a second program is talking to it.
#[cfg(feature = "gui")]
fn serve_cli(app: &tauri::AppHandle, config_dir: &std::path::Path) {
    let server = match ipc::listen(config_dir) {
        Ok(server) => server,
        Err(e) => {
            eprintln!("calenpomo: `calpo start` cannot reach this window: {e}");
            return;
        }
    };

    let app = app.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("calenpomo-ipc".to_string())
        .spawn(move || server.serve(|request| commands::handle_ipc(&app, request)))
    {
        eprintln!("calenpomo: `calpo start` cannot reach this window: {e}");
    }
}

#[cfg(feature = "gui")]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // Registering it opens no connection and starts no timer: the plugin
        // does nothing at all until `integrations::updates::check` asks it to.
        // Here rather than in `integrations::install` because, unlike the four
        // there, there is no OS permission it could be refused.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            app.manage(AppState::new(config_dir.clone()));

            let handle = app.handle().clone();
            integrations::install(&handle);
            serve_cli(&handle, &config_dir);

            // A plain thread rather than an async task: everything here is
            // synchronous, and a `std::sync::MutexGuard` is not `Send`.
            std::thread::Builder::new()
                .name("calenpomo-timer".to_string())
                .spawn(move || loop {
                    std::thread::sleep(TICK);
                    // Nothing to recover here -- the next tick tries again.
                    let _ = integrations::pump(&handle);
                })?;

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // Only hide if there is a tray to get back from. Without one the
                // user would be left with a process they can neither see nor
                // quit -- so when the tray is unavailable, close means close.
                if tray::is_live(window.app_handle()) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::vault_status,
            commands::set_vault,
            commands::calendar_range,
            commands::calendar_create,
            commands::calendar_update,
            commands::calendar_delete,
            commands::sessions_range,
            commands::timer_state,
            commands::timer_start,
            commands::timer_toggle,
            commands::timer_reset,
            commands::timer_set_durations,
            commands::integration_status,
            commands::set_autostart,
            commands::set_shortcut,
            commands::test_notification,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|_app, _event| {
        // macOS: clicking the Dock icon after the window was hidden produces
        // this and nothing else. Without it the app looks dead to anyone who
        // closed the window and then tried to come back through the Dock.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = _event {
            tray::show_window(_app);
        }
    });
}
