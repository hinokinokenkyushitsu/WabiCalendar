//! The tray icon and the countdown that lives beside it.
//!
//! Which surface can actually show text differs by platform, so the remaining
//! time is written to all three and each platform picks up whichever it has:
//!
//! | surface        | macOS | Windows | Linux |
//! |----------------|-------|---------|-------|
//! | `set_title`    | yes   | **no**  | yes   |
//! | `set_tooltip`  | yes   | yes     | **no**|
//! | menu item text | yes   | yes     | yes   |
//!
//! The menu item is the one that works everywhere, which is why the countdown is
//! repeated there rather than only shown next to the icon.
//!
//! Nothing here returns an error to a caller. A tray that will not build is
//! recorded in [`super::IntegrationStatus`] and the app runs on as a plain
//! window; a repaint that fails is dropped, because a cosmetic failure must not
//! take the timer down with it.

use std::sync::Mutex;

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use super::updates;
use crate::commands::AppState;
use crate::error::{AppError, Result};
use crate::timer::{Phase, RunState, TimerState};

/// The handles we have to keep in order to keep repainting.
pub struct Tray {
    icon: TrayIcon<Wry>,
    remaining: MenuItem<Wry>,
    toggle: MenuItem<Wry>,
    /// Handed out by [`updates_item`] so the check can report into it. Nothing
    /// in `render` touches it, so the two never fight over the label.
    updates: MenuItem<Wry>,
    /// What we last painted, so an unchanged second costs nothing. Every setter
    /// below is a round trip to the main thread; at 1 Hz forever that adds up.
    last: Mutex<Painted>,
}

#[derive(Default, Clone, PartialEq, Eq)]
struct Painted {
    title: String,
    tooltip: String,
    remaining: String,
    toggle: String,
}

pub fn build(app: &AppHandle) -> Result<Tray> {
    let remaining = MenuItem::with_id(app, "remaining", "--:--", false, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", "Start", true, None::<&str>)?;
    let reset = MenuItem::with_id(app, "reset", "Reset", true, None::<&str>)?;
    let show = MenuItem::with_id(app, "show", "Show window", true, None::<&str>)?;
    let updates = MenuItem::with_id(
        app,
        updates::MENU_ID,
        updates::MENU_IDLE,
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &remaining,
            &PredefinedMenuItem::separator(app)?,
            &toggle,
            &reset,
            &PredefinedMenuItem::separator(app)?,
            &show,
            &updates,
            &quit,
        ],
    )?;

    // Reuse the bundled app icon rather than loading one at runtime: the context
    // embeds it already, which keeps the `image-png` feature off.
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| AppError::Integration {
            feature: "tray",
            reason: "this build has no window icon to reuse".to_string(),
        })?;

    let icon = TrayIconBuilder::with_id("main")
        .icon(icon)
        .menu(&menu)
        // Left click belongs to "show the window"; the menu is the right click.
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu)
        .on_tray_icon_event(on_icon)
        .build(app)?;

    Ok(Tray {
        icon,
        remaining,
        toggle,
        updates,
        last: Mutex::new(Painted::default()),
    })
}

fn on_menu(app: &AppHandle, event: MenuEvent) {
    match event.id.as_ref() {
        "toggle" => {
            app.state::<AppState>().timer().toggle();
            // A failed repaint is not worth propagating out of a click handler.
            let _ = super::pump(app);
        }
        "reset" => {
            app.state::<AppState>().timer().reset();
            let _ = super::pump(app);
        }
        "show" => show_window(app),
        // Returns at once; the network half runs on a worker.
        updates::MENU_ID => updates::check(app),
        // The one path that really exits. It raises `RunEvent::ExitRequested`
        // rather than the window's `CloseRequested`, so the hide-on-close
        // handler never sees it.
        "quit" => app.exit(0),
        _ => {}
    }
}

fn on_icon(tray: &TrayIcon<Wry>, event: TrayIconEvent) {
    // Linux delivers no tray icon events at all, which is why `Show window` also
    // exists as a menu item.
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        show_window(tray.app_handle());
    }
}

pub fn show_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
}

/// The menu entry that starts an update check, when there is a tray to hold it.
///
/// Handed out rather than driven from here because everything that happens to
/// its label belongs to the check, not to the countdown.
pub fn updates_item(app: &AppHandle) -> Option<MenuItem<Wry>> {
    app.try_state::<Tray>().map(|tray| tray.updates.clone())
}

/// True when the tray is up, which is also the answer to "may closing the window
/// hide it?".
pub fn is_live(app: &AppHandle) -> bool {
    app.try_state::<Tray>().is_some()
}

pub fn format_clock(total_sec: u64) -> String {
    format!("{:02}:{:02}", total_sec / 60, total_sec % 60)
}

/// Repaint every surface that this platform supports. Cheap and safe to call
/// every tick.
pub fn render(app: &AppHandle, state: &TimerState) {
    let Some(tray) = app.try_state::<Tray>() else {
        return;
    };

    let clock = format_clock(state.remaining_sec);
    let phase = match state.phase {
        Phase::Work => "Work",
        Phase::Break => "Break",
    };

    // The icon-adjacent title stays empty when nothing is counting: a number
    // frozen in the menu bar reads as a bug.
    let (title, remaining) = match state.run {
        RunState::Running => (clock.clone(), format!("{clock} left")),
        RunState::Paused => (clock.clone(), format!("Paused at {clock}")),
        RunState::Idle => (String::new(), format!("{phase} {clock}, ready to start")),
        RunState::Finished => (
            String::new(),
            format!("Last segment done — {phase} {clock}, ready to start"),
        ),
        RunState::Invalidated => (String::new(), "Last segment voided by sleep".to_string()),
    };
    let tooltip = format!("CalenPomo — {phase} {remaining}");
    let toggle = match state.run {
        RunState::Running => "Pause",
        RunState::Paused => "Resume",
        _ => "Start",
    };

    let painted = Painted {
        title,
        tooltip,
        remaining,
        toggle: toggle.to_string(),
    };

    {
        let mut last = tray.last.lock().unwrap_or_else(|e| e.into_inner());
        if *last == painted {
            return;
        }
        *last = painted.clone();
    }

    // Every one of these can fail on a desktop that took the tray away under us.
    // Dropping the error is deliberate, in the same spirit as `fs_atomic`'s
    // ignored `F_FULLFSYNC` result: there is no recovery, and no data at stake.
    #[cfg(not(target_os = "windows"))]
    let _ = tray.icon.set_title(Some(&painted.title));
    #[cfg(not(target_os = "linux"))]
    let _ = tray.icon.set_tooltip(Some(&painted.tooltip));

    let _ = tray.remaining.set_text(&painted.remaining);
    let _ = tray.toggle.set_text(&painted.toggle);
}
