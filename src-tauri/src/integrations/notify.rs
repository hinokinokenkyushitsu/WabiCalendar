//! End-of-segment system notifications.
//!
//! This is the one alarm CLAUDE.md's "no reminder system" rule makes an
//! exception for, and it is the reason the countdown has to live in Rust: the
//! window is usually closed when a pomodoro ends.

use tauri::AppHandle;
use tauri_plugin_notification::{NotificationExt, PermissionState};

use super::FeatureStatus;
use crate::error::{AppError, Result};
use crate::timer::{Phase, Transition};

/// Ask the OS where we stand, prompting once if it has not been decided yet.
pub fn probe(app: &AppHandle) -> FeatureStatus {
    let state = match app.notification().permission_state() {
        Ok(state) => state,
        // No notification centre to talk to at all.
        Err(e) => {
            return FeatureStatus::Unsupported {
                reason: e.to_string(),
            }
        }
    };

    let decided = match state {
        PermissionState::Prompt | PermissionState::PromptWithRationale => {
            match app.notification().request_permission() {
                Ok(state) => state,
                Err(e) => {
                    return FeatureStatus::Denied {
                        reason: e.to_string(),
                    }
                }
            }
        }
        settled => settled,
    };

    match decided {
        PermissionState::Granted => FeatureStatus::Ready,
        other => FeatureStatus::Denied {
            reason: format!("the system has not granted notification permission ({other:?})"),
        },
    }
}

fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Work => "Work",
        Phase::Break => "Break",
    }
}

/// Announce a transition. Returns `Err` so the caller can downgrade the recorded
/// status; it is never worth failing an operation over.
pub fn announce(app: &AppHandle, transition: &Transition) -> Result<()> {
    let (title, body) = match transition {
        // Only the break starts by itself (`Phase::auto_starts`), so the two
        // hand-overs have to read differently: one is already counting, the
        // other is waiting on the user.
        Transition::Finished { phase, next, .. } => (
            format!("{} segment finished", phase_name(*phase)),
            match next {
                Phase::Work => "Break's over — start the next pomodoro.".to_string(),
                Phase::Break => {
                    "Focus done — your break has started, so get up and walk around.".to_string()
                }
            },
        ),
        Transition::Invalidated { slept_sec, .. } => (
            "This pomodoro was voided".to_string(),
            format!(
                "The computer slept for about {} minutes while the timer ran, so this segment is not recorded.",
                slept_sec / 60
            ),
        ),
    };

    show(app, &title, &body)
}

pub fn show(app: &AppHandle, title: &str, body: &str) -> Result<()> {
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| AppError::Integration {
            feature: "notification",
            reason: e.to_string(),
        })
}
