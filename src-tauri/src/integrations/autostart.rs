//! Launch at login.
//!
//! Registration lives outside our files -- a LaunchAgent plist on macOS, a
//! registry key on Windows, a `.desktop` file under `~/.config/autostart` on
//! Linux -- so **the OS is the source of truth, not `settings.toml`**. The user
//! may well have removed the login item in system settings, and we must not
//! quietly put it back.

use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;

use super::FeatureStatus;
use crate::error::{AppError, Result};
use crate::settings::Settings;

fn denied(e: impl std::fmt::Display) -> FeatureStatus {
    FeatureStatus::Denied {
        reason: e.to_string(),
    }
}

/// Bring `settings.toml` into line with what the OS actually has registered.
///
/// Note for anyone testing from `npm run tauri dev`: this registers the binary
/// under `target/debug/`, not an installed app.
pub fn reconcile(app: &AppHandle, config_dir: &std::path::Path, wanted: bool) -> FeatureStatus {
    let registered = match app.autolaunch().is_enabled() {
        Ok(registered) => registered,
        Err(e) => return denied(e),
    };

    if registered == wanted {
        return if wanted {
            FeatureStatus::Ready
        } else {
            FeatureStatus::Off
        };
    }

    // They disagree, and the OS wins. Writing our record back keeps the settings
    // panel honest on the next launch.
    if let Err(e) = Settings::update(config_dir, |s| s.autostart = registered) {
        return denied(e);
    }

    if registered {
        FeatureStatus::Ready
    } else {
        FeatureStatus::Off
    }
}

pub fn set(app: &AppHandle, enabled: bool) -> Result<FeatureStatus> {
    let manager = app.autolaunch();
    let outcome = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };

    outcome.map_err(|e| AppError::Integration {
        feature: "autostart",
        reason: e.to_string(),
    })?;

    Ok(if enabled {
        FeatureStatus::Ready
    } else {
        FeatureStatus::Off
    })
}
