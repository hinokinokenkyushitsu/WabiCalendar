//! The one thing this app ever sends over the network, and only when told to.
//!
//! There is no check at startup, no timer, and no setting that could turn one
//! on later: the single entry point is the tray's "Check for Updates…", so an
//! installation nobody asks never speaks to anything. What goes out is a GET
//! for the release's `latest.json` and then, only after the user has said yes,
//! the download it names. Neither carries anything about this machine or the
//! vault on it.
//!
//! The bundle's signature is checked against the public key baked into
//! `tauri.conf.json` before a byte of it is installed. That is what makes the
//! release host somewhere the app downloads from rather than something it
//! trusts.
//!
//! Not part of [`super::IntegrationStatus`], for the same reason `serve_cli` is
//! not: that panel answers "what will this machine let the app do", and this is
//! not a capability the OS grants or withholds.

use tauri::menu::MenuItem;
use tauri::{AppHandle, Wry};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

use super::tray;
use crate::error::{AppError, Result};

/// The tray menu entry, and what it says when it is not doing anything.
pub const MENU_ID: &str = "updates";
pub const MENU_IDLE: &str = "Check for Updates…";

/// Ask the release host whether there is anything newer, and offer to install it.
///
/// Returns immediately. Everything below happens on a worker thread, which is
/// also what makes the blocking dialogs legal — they must not be shown from the
/// thread that services the menu, which is the main one.
pub fn check(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let item = tray::updates_item(&app);
        // Disabled rather than merely relabelled: a second check started on top
        // of the first would race it to the same dialog.
        if let Some(item) = &item {
            let _ = item.set_enabled(false);
            let _ = item.set_text("Checking…");
        }

        if let Err(e) = run(&app, item.as_ref()).await {
            app.dialog()
                .message(explain(&e))
                .title("Could not check for updates")
                .kind(MessageDialogKind::Error)
                .blocking_show();
        }

        // Not reached when the install went through: `restart` does not return.
        if let Some(item) = &item {
            let _ = item.set_text(MENU_IDLE);
            let _ = item.set_enabled(true);
        }
    });
}

async fn run(app: &AppHandle, item: Option<&MenuItem<Wry>>) -> Result<()> {
    let Some(update) = app.updater()?.check().await? else {
        app.dialog()
            .message(format!(
                "CalenPomo {} is the latest version.",
                app.package_info().version
            ))
            .title("Up to date")
            .blocking_show();
        return Ok(());
    };

    let install = app
        .dialog()
        .message(format!(
            "CalenPomo {} is available. This one is {}.\n\n\
             It will be downloaded and the app will restart.",
            update.version, update.current_version
        ))
        .title("Update available")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Install".to_string(),
            "Not now".to_string(),
        ))
        .blocking_show();
    if !install {
        return Ok(());
    }

    // The menu item is the only surface this has, so the download reports into
    // it. A bundle is tens of megabytes; silence for that long reads as a hang.
    let mut downloaded = 0u64;
    let mut shown = u8::MAX;
    update
        .download_and_install(
            |chunk, total| {
                downloaded += chunk as u64;
                let Some(total) = total.filter(|total| *total > 0) else {
                    return;
                };
                let percent = (downloaded * 100 / total).min(100) as u8;
                // Every setter is a hop to the main thread. Once per percent is
                // as often as this is worth saying.
                if percent != shown {
                    shown = percent;
                    if let Some(item) = item {
                        let _ = item.set_text(format!("Downloading… {percent}%"));
                    }
                }
            },
            || {},
        )
        .await?;

    // Installed beside us; this process is still the old build.
    app.restart()
}

/// The plugin's own words, except for the one failure that is not a fault.
///
/// Anything the endpoint answers that is not a release lands as
/// `ReleaseNotFound` -- including the plain 404 of a project that has not
/// published one yet, which is the likeliest reason anyone sees this at all.
/// "Could not fetch a valid release JSON from the remote" sends that person
/// looking for a bug in their own installation.
fn explain(error: &AppError) -> String {
    match error {
        AppError::Update(source)
            if matches!(**source, tauri_plugin_updater::Error::ReleaseNotFound) =>
        {
            "There is no published release to compare this version against.".to_string()
        }
        other => other.to_string(),
    }
}
