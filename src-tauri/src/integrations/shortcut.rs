//! The global start/pause key.
//!
//! Linux support is X11-only -- the underlying `global-hotkey` crate grabs keys
//! through X -- so under a pure Wayland session either the plugin or the
//! registration will fail. That is a normal outcome here, not an error path: the
//! app keeps its window buttons and its tray menu.
//!
//! macOS registers through Carbon's `RegisterEventHotKey`, which needs no
//! Accessibility permission, so there is no TCC prompt to handle.

use std::str::FromStr;

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use super::FeatureStatus;
use crate::commands::AppState;
use crate::error::{AppError, Result};

/// Point the global key at the timer, replacing whatever was bound before.
///
/// `None` means the user turned the shortcut off, which is reported as
/// [`FeatureStatus::Off`] rather than as a failure.
///
/// Returns `Err` only when `accelerator` is not a key combination at all -- that
/// is bad input and the caller should say so. A combination that is merely
/// *taken* comes back as [`FeatureStatus::Denied`], because there is nothing
/// wrong with what the user typed and the app carries on regardless.
pub fn apply(app: &AppHandle, accelerator: Option<&str>) -> Result<FeatureStatus> {
    // We only ever own one shortcut, so this needs no bookkeeping of the old one.
    let _ = app.global_shortcut().unregister_all();

    let Some(accelerator) = accelerator else {
        return Ok(FeatureStatus::Off);
    };

    let shortcut = Shortcut::from_str(accelerator)
        .map_err(|_| AppError::InvalidShortcut(accelerator.to_string()))?;

    match app
        .global_shortcut()
        .on_shortcut(shortcut, |app, _, event| {
            // The handler fires on press *and* release. Without this filter one
            // keypress toggles twice and lands back exactly where it started, which
            // looks like the shortcut doing nothing at all.
            if event.state == ShortcutState::Pressed {
                app.state::<AppState>().timer().toggle();
                let _ = super::pump(app);
            }
        }) {
        Ok(()) => Ok(FeatureStatus::Ready),
        // Almost always "another application already owns this combination".
        Err(e) => Ok(FeatureStatus::Denied {
            reason: e.to_string(),
        }),
    }
}
