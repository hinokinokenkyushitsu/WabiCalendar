//! The four OS integrations, and what to do when a platform will not give us one.
//!
//! Every one of them is optional by construction. A tray that will not build, a
//! hotkey another app already owns, a notification the user refused, an
//! autostart directory we cannot write -- none of these may stop the app from
//! running. Each records why it is unavailable and the UI says so; nothing
//! throws.
//!
//! All four are driven from Rust rather than from the webview. That keeps the
//! degradation logic in one place and keeps the plugin ACL surface in
//! `capabilities/default.json` at zero.

pub mod autostart;
pub mod notify;
pub mod shortcut;
pub mod tray;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::AppState;
use crate::error::Result;
use crate::settings::Settings;
use crate::timer::TimerState;

/// Emitted whenever a segment ends or is voided. The frontend listens so it can
/// react at once instead of waiting for its next poll.
pub const TRANSITION_EVENT: &str = "timer://transition";

/// Emitted once one or more session records have reached the disk.
///
/// Separate from [`TRANSITION_EVENT`] because the two do not coincide: an
/// abandoned pomodoro is recorded without being announced, and a transition
/// fires whether or not the vault accepted the write. The week view listens to
/// this one, so the "actual" lane only ever redraws from what is really there.
pub const SESSION_EVENT: &str = "sessions://recorded";

/// Whether one integration is actually working right now.
///
/// Mirrored by hand in `src/types/integrations.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum FeatureStatus {
    Ready,
    /// The user switched it off. Not a failure.
    Off,
    /// This platform, or this session's display server, cannot do it at all.
    /// Retrying will not help.
    Unsupported {
        reason: String,
    },
    /// The OS or the user said no, or something else already owns it. Worth
    /// offering the user a retry.
    Denied {
        reason: String,
    },
}

/// Mirrored by hand in `src/types/integrations.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationStatus {
    pub tray: FeatureStatus,
    pub shortcut: FeatureStatus,
    pub notification: FeatureStatus,
    pub autostart: FeatureStatus,
}

// Nothing is wired up until `setup` says so, and "off" is the honest reading of
// that -- claiming `Ready` before trying would make the UI lie on the way up.
impl Default for IntegrationStatus {
    fn default() -> Self {
        Self {
            tray: FeatureStatus::Off,
            shortcut: FeatureStatus::Off,
            notification: FeatureStatus::Off,
            autostart: FeatureStatus::Off,
        }
    }
}

/// Bring up every OS integration, recording what this machine would not give us.
///
/// Deliberately returns nothing. Not one of these is allowed to abort startup:
/// an app that refuses to launch because a hotkey was taken is worse than an app
/// with no hotkey.
pub fn install(app: &AppHandle) {
    let tray = match tray::build(app) {
        Ok(tray) => {
            app.manage(tray);
            FeatureStatus::Ready
        }
        Err(e) => FeatureStatus::Denied {
            reason: e.to_string(),
        },
    };

    let state = app.state::<AppState>();
    let config_dir = state.config_dir().to_path_buf();
    // Falling back to defaults here rather than failing: `vault_status` is what
    // reports a malformed settings file, and the integrations should not be the
    // thing that breaks over it.
    let settings = Settings::load(&config_dir).unwrap_or_default();

    // Registered here rather than in the `Builder` chain on purpose.
    // `AppHandle::plugin` hands back a `Result`, so a plugin that will not start
    // -- global-shortcut under Wayland being the realistic case -- becomes a
    // recorded status instead of a dead app.
    let notification = match app.plugin(tauri_plugin_notification::init()) {
        Ok(()) => notify::probe(app),
        Err(e) => FeatureStatus::Unsupported {
            reason: e.to_string(),
        },
    };

    let shortcut = match app.plugin(tauri_plugin_global_shortcut::Builder::new().build()) {
        Ok(()) => shortcut::apply(app, settings.shortcut_toggle.as_deref()).unwrap_or_else(|e| {
            // A stored accelerator we can no longer parse: report it and move on
            // rather than refusing to start.
            FeatureStatus::Denied {
                reason: e.to_string(),
            }
        }),
        Err(e) => FeatureStatus::Unsupported {
            reason: e.to_string(),
        },
    };

    let autostart = match app.plugin(tauri_plugin_autostart::init(
        tauri_plugin_autostart::MacosLauncher::LaunchAgent,
        None,
    )) {
        Ok(()) => autostart::reconcile(app, &config_dir, settings.autostart),
        Err(e) => FeatureStatus::Unsupported {
            reason: e.to_string(),
        },
    };

    let mut status = state.integrations();
    status.tray = tray;
    status.notification = notification;
    status.shortcut = shortcut;
    status.autostart = autostart;
}

/// Advance the timer and dispatch everything that hangs off a transition.
///
/// Both the tray ticker and the frontend's `timer_state` poll come through here.
/// [`crate::timer::Timer::observe`] only produces transitions when the state
/// genuinely changes, so whichever caller arrives first takes them and the other
/// gets an empty list -- which is what makes the end-of-segment notification
/// fire exactly once even with a window open and the tray ticking.
pub fn pump(app: &AppHandle) -> Result<TimerState> {
    let state = app.state::<AppState>();

    let (snapshot, transitions, ended) = {
        let mut timer = state.timer();
        let (snapshot, mut fresh) = timer.observe();

        // Whatever the cold-start restore found happened before anything we just
        // observed, so it goes first.
        let mut transitions = state.take_pending();
        transitions.append(&mut fresh);

        let ended = timer.take_ended();

        if timer.persist_due() {
            timer.persist()?;
        }
        (snapshot, transitions, ended)
    };

    // The user's own data first, and before the event below: by the time the
    // week view hears that something was recorded, it has to be readable.
    // Failures leave the record queued for the next tick rather than surfacing
    // here -- a vault that went away must not be able to stop the countdown.
    let recorded = state.drain_sessions(ended);

    // Dispatch with the timer lock released. This is not tidiness: a tray or menu
    // setter called from the ticker thread blocks until the main thread services
    // it, and Tauri runs synchronous commands *on* the main thread. Holding the
    // timer lock across that hop would deadlock the two against each other, and
    // it would look like a random freeze rather than a crash.
    for transition in &transitions {
        if let Err(e) = notify::announce(app, transition) {
            // The segment still ended; we just could not say so out loud. Record
            // it so the settings panel stops claiming notifications work.
            state.integrations().notification = FeatureStatus::Denied {
                reason: e.to_string(),
            };
        }
    }

    if !transitions.is_empty() {
        app.emit(TRANSITION_EVENT, &transitions)?;
    }
    if recorded > 0 {
        app.emit(SESSION_EVENT, recorded)?;
    }
    tray::render(app, &snapshot);

    Ok(snapshot)
}
