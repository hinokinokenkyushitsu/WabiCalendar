//! The Tauri command surface.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::calendar::{CalEvent, Calendars, EventDraft};
use crate::error::{AppError, Result};
use crate::integrations::{self, IntegrationStatus};
use crate::ipc;
use crate::sessions::{SessionView, Sessions};
use crate::settings::Settings;
use crate::timer::{self, Ended, Phase, RealClock, StartOptions, Timer, TimerState, Transition};
use crate::vault::{same_vault, Vault};

/// How many finished segments may pile up waiting for a vault to write them to.
///
/// The queue only grows while the vault is unreachable — an unplugged drive, or
/// a timer used before one was ever chosen — and drains the moment it comes
/// back. The cap is here so that an app left running for weeks with no vault
/// cannot grow without bound; at one work segment every 25 minutes it is over
/// four days of solid pomodoros.
const MAX_UNWRITTEN: usize = 256;

/// Everything the frontend needs to know about the vault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum VaultStatus {
    /// First launch: no vault has been chosen yet.
    Unconfigured,
    /// A path was remembered but is not there any more — unplugged drive, moved
    /// or renamed folder. We ask again rather than guess.
    Missing { path: String },
    Ready {
        path: String,
        /// Parts of the skeleton that were missing and had to be rebuilt.
        rebuilt: Vec<String>,
    },
}

pub struct AppState {
    config_dir: PathBuf,
    vault: Mutex<Option<Vault>>,
    timer: Mutex<Timer<RealClock>>,
    /// Transitions the cold-start restore found, waiting for the first `pump`
    /// to dispatch them. Nothing can be emitted during `setup` -- there is no
    /// window listening yet.
    pending: Mutex<Vec<Transition>>,
    /// Segments that ended but are not on disk yet.
    ///
    /// The timer deliberately runs without a vault, so a pomodoro can finish
    /// with nowhere to put it. Holding the record here rather than dropping it
    /// means choosing a vault afterwards still saves the morning's work.
    unwritten: Mutex<Vec<Ended>>,
    integrations: Mutex<IntegrationStatus>,
}

impl AppState {
    pub fn new(config_dir: PathBuf) -> Self {
        // A malformed settings file is deliberately fatal for the vault (see
        // `Settings::load`), but the timer still has to come up: the user needs
        // a working app to fix the file from. `vault_status` is what surfaces
        // the parse error.
        let settings = Settings::load(&config_dir).unwrap_or_default();
        let (timer, pending) = Timer::load(
            RealClock::new(),
            timer::state_path(&config_dir),
            Duration::from_secs(settings.work_secs),
            Duration::from_secs(settings.break_secs),
        );

        Self {
            config_dir,
            vault: Mutex::new(None),
            timer: Mutex::new(timer),
            pending: Mutex::new(pending),
            unwritten: Mutex::new(Vec::new()),
            integrations: Mutex::new(IntegrationStatus::default()),
        }
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    /// A panic elsewhere while this lock was held says nothing about the data
    /// behind it, so recover from poisoning instead of propagating it.
    fn vault(&self) -> MutexGuard<'_, Option<Vault>> {
        self.vault.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn timer(&self) -> MutexGuard<'_, Timer<RealClock>> {
        self.timer.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn integrations(&self) -> MutexGuard<'_, IntegrationStatus> {
        self.integrations.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn take_pending(&self) -> Vec<Transition> {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut pending)
    }

    /// Get finished segments onto disk, reporting how many landed.
    ///
    /// Whatever cannot be written stays queued for the next call, so an
    /// unplugged drive costs a retry rather than the record. Deliberately
    /// infallible: this runs on every tick, and a vault that has gone away must
    /// not be allowed to stop the countdown.
    pub fn drain_sessions(&self, fresh: Vec<Ended>) -> usize {
        let mut queue = self.unwritten.lock().unwrap_or_else(|e| e.into_inner());
        queue.extend(fresh);
        // Oldest first: if something has to go, lose what the user is least
        // likely to still be looking for.
        let overflow = queue.len().saturating_sub(MAX_UNWRITTEN);
        queue.drain(..overflow);
        if queue.is_empty() {
            return 0;
        }

        let Some(vault) = self.vault().as_ref().cloned() else {
            return 0;
        };

        // A single attempt, never a wait: this runs on the one-second ticker,
        // and a tick parked behind the CLI's write is a countdown that stops.
        // Losing the race costs nothing -- the records stay queued and the next
        // tick tries again, which is the same path an unplugged drive takes.
        let Ok(_lock) = vault.try_lock() else {
            return 0;
        };
        let sessions = Sessions::local(&vault);

        // Stopping at the first failure rather than skipping past it: these
        // files are append-only, so writing a later record over the gap left by
        // an earlier one would put the day out of order for anyone reading it
        // by hand.
        let mut written = 0;
        while let Some(ended) = queue.first() {
            if sessions.record(ended).is_err() {
                break;
            }
            queue.remove(0);
            written += 1;
        }

        written
    }

    /// Where this app would put a session right now.
    ///
    /// The open vault when there is one, and otherwise the path it is about to
    /// open: nothing opens a vault until the frontend asks for its status, so
    /// for the first moments of a launch the settings file is the only one that
    /// knows. Used to check that `wabi` means the same vault this app does.
    fn vault_destination(&self) -> Option<PathBuf> {
        if let Some(vault) = self.vault().as_ref() {
            return Some(vault.root().to_path_buf());
        }
        Settings::load(&self.config_dir).ok()?.vault_path
    }

    /// Run `job` against the open vault, or fail if there is not one.
    ///
    /// Two locks, for two different neighbours. The mutex is held for the whole
    /// job so that two calendar writes in *this* process cannot interleave a
    /// read-modify-write on the same shard. The vault's file lock does the same
    /// against the `wabi` binary, which shares the files and not the mutex.
    ///
    /// Reads take the exclusive lock too. It costs a few milliseconds and buys
    /// the guarantee that a `calendar_range` never lands inside a cross-month
    /// move, where the event is briefly present in both shards.
    fn with_vault<T>(&self, job: impl FnOnce(&Vault) -> Result<T>) -> Result<T> {
        match self.vault().as_ref() {
            Some(vault) => {
                let _lock = vault.lock()?;
                job(vault)
            }
            None => Err(AppError::NoVault),
        }
    }
}

#[tauri::command]
pub fn vault_status(state: State<'_, AppState>) -> Result<VaultStatus> {
    let Some(path) = Settings::load(&state.config_dir)?.vault_path else {
        return Ok(VaultStatus::Unconfigured);
    };
    open_and_remember(&state, path)
}

#[tauri::command]
pub fn set_vault(state: State<'_, AppState>, path: String) -> Result<VaultStatus> {
    let (vault, report) = Vault::open(PathBuf::from(path))?;

    // The vault first, the pointer to it second: remembering a path we failed to
    // open would strand the app on the next launch. Load-mutate-save, not a
    // struct literal -- a literal would reset every other setting.
    Settings::update(&state.config_dir, |s| {
        s.vault_path = Some(vault.root().to_path_buf())
    })?;

    let status = VaultStatus::Ready {
        path: vault.root().display().to_string(),
        rebuilt: report.created,
    };
    *state.vault() = Some(vault);
    Ok(status)
}

/// Every block the week view should draw, for the half-open window `[from, to)`.
///
/// The window comes from the frontend as RFC 3339 and is compared as an instant,
/// so which week that is stays a question for whoever is looking at the screen.
#[tauri::command]
pub fn calendar_range(
    state: State<'_, AppState>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<CalEvent>> {
    state.with_vault(|vault| Calendars::local(vault).range(from, to))
}

#[tauri::command]
pub fn calendar_create(state: State<'_, AppState>, draft: EventDraft) -> Result<CalEvent> {
    state.with_vault(|vault| Calendars::local(vault).create(&draft))
}

#[tauri::command]
pub fn calendar_update(
    state: State<'_, AppState>,
    uid: String,
    draft: EventDraft,
) -> Result<CalEvent> {
    state.with_vault(|vault| Calendars::local(vault).update(&uid, &draft))
}

#[tauri::command]
pub fn calendar_delete(state: State<'_, AppState>, uid: String) -> Result<()> {
    state.with_vault(|vault| Calendars::local(vault).delete(&uid))
}

/// Every pomodoro that ran inside the same window `calendar_range` is asked for.
///
/// The week view draws these beside the planned blocks, so the two calls take
/// the same bounds and are read against the same instants.
#[tauri::command]
pub fn sessions_range(
    state: State<'_, AppState>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<SessionView>> {
    state.with_vault(|vault| {
        let found = Sessions::local(vault).range(from, to)?;
        Ok(found.iter().map(SessionView::from).collect())
    })
}

/// The frontend polls this roughly once a second.
///
/// It returns a *computed* remaining time rather than a tick, because the
/// frontend is forbidden from accumulating its own count (invariant #2): a
/// dropped frame, a throttled background tab or a suspend would all put a
/// locally-accumulated counter out of step with the real one.
#[tauri::command]
pub fn timer_state(app: AppHandle) -> Result<TimerState> {
    integrations::pump(&app)
}

#[tauri::command]
pub fn timer_start(app: AppHandle) -> Result<TimerState> {
    app.state::<AppState>().timer().start();
    integrations::pump(&app)
}

#[tauri::command]
pub fn timer_toggle(app: AppHandle) -> Result<TimerState> {
    app.state::<AppState>().timer().toggle();
    integrations::pump(&app)
}

#[tauri::command]
pub fn timer_reset(app: AppHandle) -> Result<TimerState> {
    app.state::<AppState>().timer().reset();
    integrations::pump(&app)
}

/// `timer_start`, reached over the local socket instead of from the window.
///
/// This is the whole reason [`crate::ipc`] exists: while the app is up it owns
/// the timer, so `wabi start` cannot run one of its own without the two writing
/// over each other. Asking the app to press its own button is the only version
/// of this that leaves one countdown, one `timer.json` and one session record.
///
/// Infallible on purpose — it is an *answer*, not a command whose failure the
/// caller can retry. Anything that goes wrong comes back as
/// [`ipc::Response::Refused`] with a sentence the user can read.
pub fn handle_ipc(app: &AppHandle, request: ipc::Request) -> ipc::Response {
    let ipc::Request::Start {
        label,
        planned_sec,
        vault,
    } = request;

    let state = app.state::<AppState>();

    // A `wabi --vault /elsewhere start` that quietly landed in whichever vault
    // this window happens to have open would be the one failure nobody could
    // debug from the outside, so the mismatch is reported rather than resolved.
    if let Some(theirs) = vault.as_deref() {
        match state.vault_destination() {
            Some(ours) if same_vault(&ours, theirs) => {}
            Some(ours) => {
                return ipc::Response::Refused {
                    reason: format!(
                        "WabiCalendar has {} open, but wabi was pointed at {}",
                        ours.display(),
                        theirs.display()
                    ),
                }
            }
            None => {
                return ipc::Response::Refused {
                    reason: "WabiCalendar has no vault open".to_string(),
                }
            }
        }
    }

    // A zero-length segment would finish the instant it began and spin the
    // notification. `timer_set_durations` clamps for the same reason.
    let planned = planned_sec.map(|secs| Duration::from_secs(secs.max(1)));
    let planned_sec = {
        let mut timer = state.timer();
        timer.start_with(StartOptions {
            label: label.clone(),
            planned,
            // `wabi start` names something to work on, so it means work even
            // if this window is halfway through a break.
            phase: Some(Phase::Work),
        });
        timer.state().planned_sec
    };

    // The timer lock is released first: `pump` reaches the tray, which is
    // serviced on the main thread, and this runs on the socket's own.
    //
    // Its failure is not the caller's business -- the segment is running either
    // way, and what `pump` can fail at is persisting and emitting.
    let _ = integrations::pump(app);

    ipc::Response::Started { planned_sec, label }
}

#[tauri::command]
pub fn timer_set_durations(app: AppHandle, work_secs: u64, break_secs: u64) -> Result<TimerState> {
    // A zero-length segment would finish the instant it started and spin the
    // notification, so refuse to store one.
    let work = Duration::from_secs(work_secs.max(1));
    let brk = Duration::from_secs(break_secs.max(1));

    {
        let state = app.state::<AppState>();
        Settings::update(state.config_dir(), |s| {
            s.work_secs = work.as_secs();
            s.break_secs = brk.as_secs();
        })?;
        state.timer().set_durations(work, brk);
    }

    integrations::pump(&app)
}

/// Everything the settings panel needs: which integrations are live, and the
/// preferences behind them, in one round trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationReport {
    pub status: IntegrationStatus,
    pub shortcut_toggle: Option<String>,
    pub autostart: bool,
    pub work_secs: u64,
    pub break_secs: u64,
    /// Where the countdown is actually visible on *this* platform, so the panel
    /// does not promise text the OS will never draw.
    pub countdown: CountdownSupport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CountdownSupport {
    /// Text beside the tray icon. Unsupported on Windows.
    pub title: bool,
    /// Hover tooltip. Unsupported on Linux.
    pub tooltip: bool,
    /// The disabled first menu entry. Works everywhere, which is why it exists.
    pub menu_item: bool,
}

fn report(state: &AppState) -> Result<IntegrationReport> {
    let settings = Settings::load(state.config_dir())?;
    Ok(IntegrationReport {
        status: state.integrations().clone(),
        shortcut_toggle: settings.shortcut_toggle,
        autostart: settings.autostart,
        work_secs: settings.work_secs,
        break_secs: settings.break_secs,
        countdown: CountdownSupport {
            title: cfg!(any(target_os = "macos", target_os = "linux")),
            tooltip: cfg!(any(target_os = "macos", target_os = "windows")),
            menu_item: true,
        },
    })
}

#[tauri::command]
pub fn integration_status(state: State<'_, AppState>) -> Result<IntegrationReport> {
    report(&state)
}

#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> Result<IntegrationReport> {
    let state = app.state::<AppState>();

    // Ask the OS first: if registration fails there is nothing worth recording,
    // and a settings file claiming autostart that the OS never accepted is
    // exactly the lie this ordering avoids.
    let status = integrations::autostart::set(&app, enabled)?;
    Settings::update(state.config_dir(), |s| s.autostart = enabled)?;
    state.integrations().autostart = status;

    report(&state)
}

/// `None` turns the global shortcut off, which is a normal choice rather than a
/// failure.
#[tauri::command]
pub fn set_shortcut(app: AppHandle, accelerator: Option<String>) -> Result<IntegrationReport> {
    let state = app.state::<AppState>();

    // Bind it before storing it, for the same reason `set_vault` opens the vault
    // before remembering the path: an unparseable accelerator in the file would
    // come back every launch.
    let status = integrations::shortcut::apply(&app, accelerator.as_deref())?;
    Settings::update(state.config_dir(), |s| {
        s.shortcut_toggle = accelerator.clone()
    })?;
    state.integrations().shortcut = status;

    report(&state)
}

/// Notifications are the one integration whose failure is invisible until it
/// matters, so let the user provoke it on demand.
///
/// Worth knowing while testing: under `npm run tauri dev` the binary is not a
/// signed bundle, and macOS will drop the notification without a word.
#[tauri::command]
pub fn test_notification(app: AppHandle) -> Result<IntegrationReport> {
    let outcome = integrations::notify::show(&app, "WabiCalendar", "Notifications are working.");

    {
        let state = app.state::<AppState>();
        state.integrations().notification = match &outcome {
            Ok(()) => integrations::FeatureStatus::Ready,
            Err(e) => integrations::FeatureStatus::Denied {
                reason: e.to_string(),
            },
        };
    }

    // Report the new status either way -- the panel is more useful than an
    // exception here.
    let _ = outcome;
    report(&app.state::<AppState>())
}

fn open_and_remember(state: &AppState, path: PathBuf) -> Result<VaultStatus> {
    match Vault::open(path) {
        Ok((vault, report)) => {
            let status = VaultStatus::Ready {
                path: vault.root().display().to_string(),
                rebuilt: report.created,
            };
            *state.vault() = Some(vault);
            Ok(status)
        }
        // Expected, and recoverable by asking the user — not an error dialog.
        Err(AppError::VaultMissing(path)) => {
            *state.vault() = None;
            Ok(VaultStatus::Missing {
                path: path.display().to_string(),
            })
        }
        Err(e) => Err(e),
    }
}
