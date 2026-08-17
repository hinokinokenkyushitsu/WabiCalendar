//! `calpo start` — begin a pomodoro from the terminal.
//!
//! Two ways of doing one thing, and which one runs is not the user's problem:
//!
//! - **The app is open.** It owns the timer — the countdown lives in its memory
//!   and it rewrites `timer.json` every ten seconds — so a second countdown here
//!   would be overwritten and forgotten. The request goes over the local socket
//!   and the app presses its own start button, tray, notification and all. This
//!   command returns immediately.
//! - **Nothing is open.** There is no timer to collide with, so this process
//!   runs one itself and holds the terminal until the segment ends. It writes
//!   the session and nothing else: `timer.json` stays the app's.
//!
//! The one seam left is a user who opens the app while a standalone countdown is
//! running here. Both then count, and both record. Closing it needs invariant #4
//! (file watching), which has no code yet.

use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use super::report;
use crate::error::{AppError, Result};
use crate::ipc::{self, Request, Response};
use crate::sessions::Sessions;
use crate::settings::Settings;
use crate::timer::{Ended, Outcome, Phase, RealClock, StartOptions, Timer};
use crate::vault::Vault;

/// How often the standalone countdown looks at its own clock.
///
/// Short enough that Ctrl-C feels immediate, and the whole point of invariant #2
/// is that this number cannot affect the count: every reading comes from
/// `Timer`, and nothing here accumulates.
const POLL: Duration = Duration::from_millis(200);

/// The longest a single segment may be asked to run.
///
/// Not a rule about pomodoros — it is there so that a typo (`--50h`) is refused
/// at the command line instead of parking the terminal until next week.
const MAX_SEGMENT: Duration = Duration::from_secs(24 * 60 * 60);

/// Fixed width of the countdown line, in ASCII columns, for erasing it again.
const COUNTDOWN_WIDTH: usize = 20;

/// Start a pomodoro, wherever there is one to start.
pub fn start(vault: &Vault, config_dir: &Path, options: StartOptions) -> Result<String> {
    let request = Request::Start {
        label: options.label.clone(),
        planned_sec: options.planned.map(|d| d.as_secs()),
        vault: Some(vault.root().to_path_buf()),
    };

    match ipc::send(config_dir, &request) {
        Ok(Some(Response::Started { planned_sec, label })) => {
            eprintln!("calpo: CalenPomo is open; it is keeping the time.");
            Ok(summary(
                "started",
                planned_sec,
                Phase::Work,
                label.as_deref(),
            ))
        }
        // Something is listening, so running a timer here anyway would produce
        // exactly the double count the socket exists to prevent.
        Ok(Some(Response::Refused { reason })) => Err(AppError::IpcRefused(reason)),
        Ok(None) => here(vault, config_dir, options),
        // Not "no app": an app we could not reach. Said out loud, because the
        // fallback is only correct if there really is nobody there.
        Err(e) => {
            eprintln!("calpo: {e}");
            eprintln!("calpo: running the countdown here instead.");
            here(vault, config_dir, options)
        }
    }
}

/// Run the whole segment in this process, then write it down.
fn here(vault: &Vault, config_dir: &Path, options: StartOptions) -> Result<String> {
    let settings = Settings::load(config_dir)?;
    let mut timer = Timer::ephemeral(
        RealClock::new(),
        Duration::from_secs(settings.work_secs.max(1)),
        Duration::from_secs(settings.break_secs.max(1)),
    );

    // Ctrl-C has to record rather than kill: twenty minutes of work that went
    // unwritten because the user stopped early is the one loss this whole
    // project is arranged against. The handler only sets a flag; the recording
    // happens on the way out of the loop below, with the vault lock held like
    // any other write.
    let interrupted = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&interrupted);
    let handled = ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst)).is_ok();
    if !handled {
        eprintln!("calpo: Ctrl-C will not be able to save this one; stop it in the app instead.");
    }

    timer.start_with(options);
    let mut screen = Countdown::new();

    let ended = loop {
        let (state, _) = timer.observe();

        // Either the segment ran out or it was voided by a suspend. `Timer`
        // has already recorded which; there is nothing to decide here.
        let ended = timer.take_ended();
        if !ended.is_empty() {
            break ended;
        }

        if interrupted.load(Ordering::SeqCst) {
            timer.reset();
            break timer.take_ended();
        }

        screen.draw(state.remaining_sec, state.phase);
        thread::sleep(POLL);
    };
    screen.clear();

    // The lock is taken here and not around the countdown: a `calpo today` in
    // another terminal must not have to wait out a 25 minute pomodoro.
    let lock = vault.lock()?;
    let sessions = Sessions::local(vault);
    for segment in &ended {
        sessions.record(segment)?;
    }
    drop(lock);

    Ok(report_of(&ended))
}

/// What to print once it is over.
///
/// An empty list is not a failure: pressing Ctrl-C in the first second is a slip
/// rather than a pomodoro, and `Timer` deliberately records nothing for it.
fn report_of(ended: &[Ended]) -> String {
    let Some(segment) = ended.last() else {
        return "nothing recorded\n".to_string();
    };
    summary(
        outcome_word(segment.outcome),
        segment.actual_sec,
        segment.phase,
        segment.label.as_deref(),
    )
}

fn outcome_word(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Completed => "completed",
        Outcome::Aborted => "stopped",
        Outcome::Invalidated => "voided",
    }
}

/// One line, in the shape `calpo today` uses for a session: fixed-width columns
/// first, the user's own text last, so that a CJK label cannot pull it out of
/// line.
fn summary(state: &str, secs: u64, phase: Phase, label: Option<&str>) -> String {
    let line = format!(
        "{:<9}  {:>6}  {:<5}  {}",
        state,
        brief(secs),
        match phase {
            Phase::Work => "work",
            Phase::Break => "break",
        },
        label.unwrap_or(""),
    );
    format!("{}\n", line.trim_end())
}

/// A length for a single segment, in seconds when it is under a minute.
///
/// `report::format_duration` floors to the minute, which is right for a day's
/// total and wrong here: a pomodoro stopped after forty seconds would report
/// "0m" and read as though nothing had been written down.
fn brief(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else {
        report::format_duration(secs as i64)
    }
}

/// The live countdown, redrawn in place.
///
/// Goes to stderr so that `calpo start ... > log` still records the one line
/// that matters, and only when stderr is a terminal — a redirected countdown is
/// thousands of lines of nothing. ASCII and a fixed width, so that erasing it is
/// exact: the label is deliberately *not* on this line, because nothing here can
/// know how many columns a CJK glyph takes.
struct Countdown {
    live: bool,
    /// The last whole second drawn, so the line is only rewritten when it moves.
    shown: Option<u64>,
}

impl Countdown {
    fn new() -> Self {
        Self {
            live: io::stderr().is_terminal(),
            shown: None,
        }
    }

    fn draw(&mut self, remaining_sec: u64, phase: Phase) {
        if !self.live || self.shown == Some(remaining_sec) {
            return;
        }
        self.shown = Some(remaining_sec);

        let word = match phase {
            Phase::Work => "work",
            Phase::Break => "break",
        };
        let clock = format!("{}:{:02}", remaining_sec / 60, remaining_sec % 60);
        let mut err = io::stderr();
        // Failures here are cosmetic by definition; the timer carries on.
        let _ = write!(err, "\r  {clock:>6}  {word:<5} ");
        let _ = err.flush();
    }

    fn clear(&mut self) {
        if !self.live || self.shown.is_none() {
            return;
        }
        let mut err = io::stderr();
        let _ = write!(err, "\r{}\r", " ".repeat(COUNTDOWN_WIDTH));
        let _ = err.flush();
    }
}

/// `25m`, `90s`, `1h`, or a bare `25` meaning minutes.
///
/// One unit and no arithmetic: a pomodoro is a round number of minutes, and
/// accepting `1h30m` would mean having to explain what `1h30` does.
pub fn parse_duration(text: &str) -> std::result::Result<Duration, String> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (digits, unit) = text.split_at(split);

    let seconds_per_unit: u64 = match unit.trim().to_ascii_lowercase().as_str() {
        // A bare number is minutes: `--for 50` is what anyone would read it as
        // for a pomodoro, and seconds would be a strange thing to default to.
        "" | "m" | "min" | "mins" | "minute" | "minutes" => 60,
        "s" | "sec" | "secs" | "second" | "seconds" => 1,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
        _ => return Err(format!("{text:?} is not a length like 25m")),
    };

    let total = digits
        .parse::<u64>()
        .ok()
        .and_then(|count| count.checked_mul(seconds_per_unit))
        .map(Duration::from_secs)
        .ok_or_else(|| format!("{text:?} is not a length like 25m"))?;

    if total.is_zero() {
        return Err("a pomodoro has to be longer than nothing".to_string());
    }
    if total > MAX_SEGMENT {
        return Err(format!(
            "{text:?} is longer than a day, which is almost certainly a typo"
        ));
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::thread::JoinHandle;

    use tempfile::TempDir;

    use crate::sessions::Session;

    fn secs(text: &str) -> u64 {
        parse_duration(text).expect(text).as_secs()
    }

    fn vault(dir: &TempDir) -> Vault {
        let (vault, _) = Vault::open(dir.path()).expect("open");
        vault
    }

    /// A stand-in for the running app: answers one caller with `reply`.
    fn app_answering(config_dir: &Path, reply: Response) -> JoinHandle<()> {
        let server = ipc::listen(config_dir).expect("listen");
        thread::spawn(move || {
            server.serve_one(&move |_| reply.clone()).expect("serve");
        })
    }

    /// Every session in the vault, however it got there.
    fn recorded(vault: &Vault) -> Vec<Session> {
        use chrono::{Duration as Delta, Utc};

        let now = Utc::now();
        Sessions::local(vault)
            .range(now - Delta::days(2), now + Delta::days(2))
            .expect("range")
    }

    /// With the app up, this command is a messenger: it must not run a timer of
    /// its own, and it must not write the session -- the app does both.
    #[test]
    fn an_open_app_is_handed_the_pomodoro_and_this_returns_at_once() {
        let vault_dir = TempDir::new().expect("tempdir");
        let config = TempDir::new().expect("tempdir");
        let vault = vault(&vault_dir);
        let app = app_answering(
            config.path(),
            Response::Started {
                planned_sec: 1500,
                label: Some("写论文".to_string()),
            },
        );

        let out = start(
            &vault,
            config.path(),
            StartOptions {
                label: Some("写论文".to_string()),
                planned: None,
                phase: Some(Phase::Work),
            },
        )
        .expect("start");

        app.join().expect("the app thread");
        assert_eq!(out, "started       25m  work   写论文\n");
        assert!(
            recorded(&vault).is_empty(),
            "the session is the app's to write"
        );
    }

    /// A refusal means something *is* listening. Starting a second countdown
    /// beside it is the one outcome the socket exists to prevent, so this has to
    /// fail rather than fall back.
    #[test]
    fn a_refusal_stops_the_command_instead_of_starting_a_second_timer() {
        let vault_dir = TempDir::new().expect("tempdir");
        let config = TempDir::new().expect("tempdir");
        let vault = vault(&vault_dir);
        let app = app_answering(
            config.path(),
            Response::Refused {
                reason: "a different vault is open".to_string(),
            },
        );

        let err = start(&vault, config.path(), StartOptions::default()).expect_err("should fail");

        app.join().expect("the app thread");
        assert!(matches!(err, AppError::IpcRefused(_)), "{err:?}");
        assert!(
            err.to_string().contains("a different vault is open"),
            "{err}"
        );
        assert!(recorded(&vault).is_empty(), "nothing should have run");
    }

    /// With nothing listening there is no timer to collide with, so the whole
    /// segment runs here and lands in the vault. A one-second pomodoro because
    /// this is a test; nothing else about the path differs.
    #[test]
    fn with_no_app_the_countdown_runs_here_and_the_session_is_written() {
        let vault_dir = TempDir::new().expect("tempdir");
        let config = TempDir::new().expect("tempdir");
        let vault = vault(&vault_dir);

        let out = start(
            &vault,
            config.path(),
            StartOptions {
                label: Some("写论文".to_string()),
                planned: Some(Duration::from_secs(1)),
                phase: Some(Phase::Work),
            },
        )
        .expect("start");

        assert_eq!(out, "completed      1s  work   写论文\n");

        let sessions = recorded(&vault);
        assert_eq!(sessions.len(), 1, "{sessions:?}");
        assert_eq!(sessions[0].label.as_deref(), Some("写论文"));
        assert_eq!(sessions[0].planned_sec, 1);
        assert_eq!(sessions[0].outcome, Outcome::Completed);
        assert_eq!(sessions[0].kind, Phase::Work);
        // The app's timer state is untouched -- there is not even a file.
        assert!(!crate::timer::state_path(config.path()).exists());
    }

    #[test]
    fn a_bare_number_is_minutes() {
        assert_eq!(secs("25"), 1500);
        assert_eq!(secs("1"), 60);
    }

    #[test]
    fn the_three_units_all_work() {
        assert_eq!(secs("90s"), 90);
        assert_eq!(secs("25m"), 1500);
        assert_eq!(secs("2h"), 7200);
    }

    #[test]
    fn the_spelled_out_units_work_too() {
        assert_eq!(secs("25min"), 1500);
        assert_eq!(secs("90sec"), 90);
        assert_eq!(secs("25M"), 1500);
    }

    /// A zero-length segment would finish the instant it began and spin the
    /// notification, so it is refused where the user can still see why.
    #[test]
    fn nothing_at_all_is_refused() {
        assert!(parse_duration("0").is_err());
        assert!(parse_duration("0m").is_err());
    }

    #[test]
    fn a_typo_is_refused_rather_than_parked_on() {
        assert!(parse_duration("50h").is_err());
        assert!(parse_duration("").is_err());
        assert!(parse_duration("25x").is_err());
        assert!(parse_duration("half an hour").is_err());
        assert!(parse_duration("-5m").is_err());
    }

    /// The one line that reaches stdout, in the same column order as a session
    /// row in `calpo today`.
    #[test]
    fn the_summary_puts_the_users_own_text_last() {
        let line = summary("completed", 1500, Phase::Work, Some("写论文"));

        assert_eq!(line, "completed     25m  work   写论文\n");
        assert!(line.ends_with("写论文\n"), "{line:?}");
    }

    #[test]
    fn a_summary_without_a_label_has_no_trailing_space() {
        let line = summary("started", 1500, Phase::Work, None);

        assert_eq!(line, "started       25m  work\n");
    }

    /// A pomodoro stopped after forty seconds did happen, and "0m" would read
    /// as though it had not been written down.
    #[test]
    fn a_segment_under_a_minute_is_reported_in_seconds() {
        assert_eq!(
            summary("stopped", 40, Phase::Work, None),
            "stopped       40s  work\n"
        );
        assert_eq!(brief(59), "59s");
        assert_eq!(brief(60), "1m", "a full minute is back to the usual shape");
    }

    #[test]
    fn a_slip_too_short_to_record_says_so_rather_than_inventing_a_pomodoro() {
        assert_eq!(report_of(&[]), "nothing recorded\n");
    }

    #[test]
    fn every_way_a_segment_can_end_has_a_word() {
        let segment = |outcome| Ended {
            phase: Phase::Work,
            planned_sec: 1500,
            actual_sec: 600,
            started_at: std::time::UNIX_EPOCH,
            ended_at: std::time::UNIX_EPOCH,
            outcome,
            label: None,
        };

        assert!(report_of(&[segment(Outcome::Completed)]).starts_with("completed"));
        assert!(report_of(&[segment(Outcome::Aborted)]).starts_with("stopped"));
        assert!(report_of(&[segment(Outcome::Invalidated)]).starts_with("voided"));
    }
}
