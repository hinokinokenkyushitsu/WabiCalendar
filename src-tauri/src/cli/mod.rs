//! The `wabi` command line: the same vault as the app, from a terminal.
//!
//! The two reports here are *readers* of files the app also writes, so they take
//! the vault's write lock exactly as the app does. Reads are locked too, so that
//! a report can never be assembled from the middle of a cross-month move, where
//! an event is briefly present in two shards at once.
//!
//! `start` is the exception and holds no lock while it runs — a 25 minute
//! pomodoro is not a write, and a `wabi today` in another terminal must not
//! have to wait one out. It takes the lock at the end, for the one append. See
//! [`start`] for the other half of that story, which is what happens when the
//! app is already running.

mod report;
mod start;

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use chrono::{
    DateTime, Datelike, Duration as Delta, Local, LocalResult, NaiveDate, NaiveTime, TimeZone, Utc,
};
use clap::{Args, Parser, Subcommand};

use crate::calendar::{CalEvent, Calendars};
use crate::error::{AppError, Result};
use crate::sessions::{Session, Sessions};
use crate::settings::Settings;
use crate::timer::{Phase, StartOptions};
use crate::vault::Vault;
use report::DAYS_PER_WEEK;

/// The bundle identifier, which is also the directory `settings.toml` sits in.
///
/// Must stay in step with `identifier` in `tauri.conf.json`. Tauri's
/// `app_config_dir()` is `dirs::config_dir()/${identifier}` and nothing else
/// (`tauri/src/path/desktop.rs`), so this reaches the same place the app does
/// without asking Tauri — which is the point, since the CLI does not link it.
pub const APP_IDENTIFIER: &str = "com.hinoki.wabicalendar";

/// Overrides the remembered vault for one invocation, below `--vault` and above
/// `settings.toml`.
pub const VAULT_ENV: &str = "WABICALENDAR_VAULT";

#[derive(Debug, Parser)]
#[command(
    name = "wabi",
    version,
    about = "The WabiCalendar vault from a terminal.",
    long_about = "Works on the same vault the WabiCalendar app does: iCalendar files under \
                  calendar/ and pomodoro records under sessions/.\n\n\
                  The vault is the one the app remembers, unless WABICALENDAR_VAULT or \
                  --vault says otherwise.\n\n\
                  While the app is open, `wabi start` asks it to run the pomodoro so \
                  that only one countdown exists; with the app closed, wabi runs it \
                  here."
)]
pub struct Cli {
    /// Use this vault instead of the remembered one, for this command only.
    ///
    /// Never written back to `settings.toml`: looking at another vault must not
    /// move the app's.
    #[arg(long, global = true, value_name = "PATH")]
    vault: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start a pomodoro now.
    Start(StartArgs),
    /// Today's plan and pomodoros, in full.
    Today,
    /// A week of totals, one row per day.
    Log(LogArgs),
}

#[derive(Debug, Args)]
struct StartArgs {
    /// What you are working on, written into the session record.
    #[arg(value_name = "LABEL")]
    label: Option<String>,

    /// How long this one runs: 25m, 90s, 2h, or a bare number of minutes.
    ///
    /// This pomodoro only. The app's own work length is left exactly as it
    /// was: one long session is not a change of mind about every session after
    /// it. `--50m` is accepted as shorthand for the same thing.
    #[arg(
        long = "for",
        short = 'd',
        value_name = "DURATION",
        value_parser = start::parse_duration
    )]
    planned: Option<Duration>,
}

impl From<StartArgs> for StartOptions {
    fn from(args: StartArgs) -> Self {
        Self {
            label: args.label,
            planned: args.planned,
            // Named work, not "whatever comes next": `wabi start "写论文"` says
            // what the user is about to do, so it has to mean work even when the
            // app is halfway through a break.
            phase: Some(Phase::Work),
        }
    }
}

#[derive(Debug, Args)]
struct LogArgs {
    /// Which week: 0 is the current one, 1 the week before, and so on.
    ///
    /// Weeks start on Monday, matching the app's week view — which is the only
    /// range there is, the app being week-only by design.
    #[arg(
        long,
        value_name = "N",
        default_value_t = 0,
        num_args = 0..=1,
        default_missing_value = "0"
    )]
    week: u32,
}

/// Where `settings.toml` lives, reproducing Tauri's `app_config_dir()`.
pub fn config_dir() -> Result<PathBuf> {
    dirs::config_dir()
        .map(|dir| dir.join(APP_IDENTIFIER))
        .ok_or(AppError::NoConfigDir)
}

/// `--vault`, then `WABICALENDAR_VAULT`, then whatever the app remembers.
///
/// Opening rebuilds a missing skeleton exactly as the app's `set_vault` does,
/// and — just as deliberately — will not create the root directory: a CLI that
/// conjured an empty vault at the mount point of an unplugged drive would look
/// to the user like the day's records had vanished.
fn open_vault(explicit: Option<PathBuf>) -> Result<Vault> {
    let from_env = std::env::var_os(VAULT_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);

    let path = choose_vault_path(explicit, from_env, || {
        Ok(Settings::load(&config_dir()?)?.vault_path)
    })?;

    let (vault, report) = Vault::open(path)?;
    if !report.created.is_empty() {
        // On stderr, so that piping the report somewhere still shows this and
        // still yields clean data on stdout.
        eprintln!(
            "wabi: rebuilt in {}: {}",
            vault.root().display(),
            report.created.join(", ")
        );
    }
    Ok(vault)
}

/// Pick a vault from the three places one can come from, most specific first.
///
/// `remembered` is a closure and not a value so that it is never consulted when
/// something more specific already answered. That is not an optimisation: a
/// malformed `settings.toml` is a hard error, and `--vault` has to stay usable
/// as the way out of one.
fn choose_vault_path(
    explicit: Option<PathBuf>,
    from_env: Option<PathBuf>,
    remembered: impl FnOnce() -> Result<Option<PathBuf>>,
) -> Result<PathBuf> {
    if let Some(path) = explicit.or(from_env) {
        return Ok(path);
    }
    remembered()?.ok_or(AppError::VaultUnset)
}

/// Local midnight at the start of `day`.
///
/// `from_local_datetime` can come back ambiguous or non-existent: a handful of
/// zones move their clocks at midnight, so that instant either happened twice
/// or never. Taking the earliest reading keeps a day contiguous with the one
/// before it, and where midnight was skipped entirely the first hour that did
/// exist is the honest start of the day.
fn local_midnight(day: NaiveDate) -> DateTime<Local> {
    let midnight = day.and_time(NaiveTime::MIN);
    for hour in 0..24 {
        match Local.from_local_datetime(&(midnight + Delta::hours(hour))) {
            LocalResult::Single(at) => return at,
            LocalResult::Ambiguous(early, _) => return early,
            LocalResult::None => continue,
        }
    }
    // No zone skips a whole day. Reading it as UTC beats failing a report.
    Utc.from_utc_datetime(&midnight).with_timezone(&Local)
}

/// Local midnight of the Monday `weeks_back` weeks before the one on or before
/// `day`.
fn monday_of(day: NaiveDate, weeks_back: u32) -> NaiveDate {
    let since_monday = i64::from(day.weekday().num_days_from_monday());
    day - Delta::days(since_monday + 7 * i64::from(weeks_back))
}

/// Everything in `[from, to)`.
fn window(vault: &Vault, from: DateTime<Local>, to: DateTime<Local>) -> Result<Contents> {
    let (from_utc, to_utc) = (from.with_timezone(&Utc), to.with_timezone(&Utc));
    Ok(Contents {
        events: Calendars::local(vault).range(from_utc, to_utc)?,
        sessions: Sessions::local(vault).range(from_utc, to_utc)?,
    })
}

/// One window of the vault, as both halves of a report need it.
struct Contents {
    events: Vec<CalEvent>,
    sessions: Vec<Session>,
}

fn today(vault: &Vault) -> Result<String> {
    let start = local_midnight(Local::now().date_naive());
    let end = local_midnight(Local::now().date_naive() + Delta::days(1));

    let contents = window(vault, start, end)?;
    // `range` sorts sessions; events come back in whatever order the shards
    // yielded them, and a day reads top to bottom.
    let mut events = contents.events;
    events.sort_by_key(|event| (event.start, event.end, event.id.clone()));

    Ok(report::day(&events, &contents.sessions, start, end))
}

fn log(vault: &Vault, weeks_back: u32) -> Result<String> {
    let monday = monday_of(Local::now().date_naive(), weeks_back);

    // The eight midnights bounding seven days, built from calendar dates rather
    // than by adding 24 hours, so the two days a year that are not 24 hours long
    // still land on midnight.
    let mut days = [local_midnight(monday); DAYS_PER_WEEK + 1];
    for (index, slot) in days.iter_mut().enumerate() {
        *slot = local_midnight(monday + Delta::days(index as i64));
    }

    let contents = window(vault, days[0], days[DAYS_PER_WEEK])?;
    Ok(report::week(&contents.events, &contents.sessions, &days))
}

/// clap has no way to spell a flag whose *name* is its value, and `--25m` is
/// the spelling this was asked for in.
///
/// So the rewrite happens before clap ever sees the arguments: any `--<digits>…`
/// becomes `--for=<digits>…`, and everything else is passed through untouched.
/// Whatever follows a bare `--` is the user saying "stop reading these as
/// flags", which is exactly what this would otherwise do to a label.
fn expand_bare_duration(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut out = Vec::new();
    let mut literal = false;

    for arg in args {
        let rewritten = arg.to_str().filter(|_| !literal).and_then(|text| {
            let rest = text.strip_prefix("--")?;
            rest.starts_with(|c: char| c.is_ascii_digit())
                .then(|| OsString::from(format!("--for={rest}")))
        });

        literal = literal || arg == "--";
        out.push(rewritten.unwrap_or(arg));
    }
    out
}

fn run(cli: Cli) -> Result<String> {
    let vault = open_vault(cli.vault)?;

    match cli.command {
        // The lock is held across the read and released before anything is
        // written out: a terminal that has stopped consuming stdout must not be
        // able to hold the app's ticker out of the vault.
        Command::Today => {
            let _lock = vault.lock()?;
            today(&vault)
        }
        Command::Log(args) => {
            let _lock = vault.lock()?;
            log(&vault, args.week)
        }
        // Takes the lock itself, for the append at the end and not for the
        // twenty-five minutes before it.
        Command::Start(args) => start::start(&vault, &config_dir()?, args.into()),
    }
}

/// Write the report out, treating a closed pipe as the reader's decision.
///
/// `print!` panics on `EPIPE`, so `wabi log | head` would end in a Rust panic
/// message rather than silence. A reader that has seen enough is not an error
/// to report.
fn emit(text: &str) -> ExitCode {
    let mut stdout = io::stdout();
    match stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("wabi: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The `wabi` entry point.
pub fn main() -> ExitCode {
    let args = expand_bare_duration(std::env::args_os());
    match run(Cli::parse_from(args)) {
        Ok(text) => emit(&text),
        Err(e) => {
            eprintln!("wabi: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::Cell;

    use clap::CommandFactory;

    fn date(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").expect("valid date")
    }

    #[test]
    fn the_command_line_itself_is_well_formed() {
        // clap's own audit: duplicate short flags, a `default_missing_value`
        // that does not parse, and similar are all caught here rather than by
        // the first user to type the command.
        Cli::command().debug_assert();
    }

    fn expanded(args: &[&str]) -> Vec<String> {
        expand_bare_duration(args.iter().map(OsString::from))
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_bare_duration_flag_becomes_one_clap_can_read() {
        assert_eq!(
            expanded(&["wabi", "start", "写论文", "--25m"]),
            vec!["wabi", "start", "写论文", "--for=25m"]
        );
        assert_eq!(expanded(&["wabi", "start", "--90s"]), {
            vec!["wabi", "start", "--for=90s"]
        });
    }

    /// Only flags that begin with a digit. Everything else clap owns, and
    /// quietly rewriting a real flag would be far worse than not helping.
    #[test]
    fn every_other_argument_is_left_exactly_as_it_was() {
        let untouched = [
            "wabi", "log", "--week", "2", "--vault", "/tmp/v", "-d", "5m",
        ];
        assert_eq!(expanded(&untouched), untouched.to_vec());

        // A label that looks like a flag is still a label after `--`.
        assert_eq!(
            expanded(&["wabi", "start", "--", "--25m"]),
            vec!["wabi", "start", "--", "--25m"]
        );
    }

    /// The full spelling has to keep working; the rewrite is a convenience on
    /// top of it, not the only way in.
    #[test]
    fn the_shorthand_and_the_flag_parse_to_the_same_thing() {
        let long = Cli::parse_from(expanded(&["wabi", "start", "x", "--for", "25m"]));
        let short = Cli::parse_from(expanded(&["wabi", "start", "x", "--25m"]));

        let planned = |cli: Cli| match cli.command {
            Command::Start(args) => StartOptions::from(args).planned,
            _ => unreachable!("parsed as start"),
        };
        assert_eq!(planned(long), Some(Duration::from_secs(1500)));
        assert_eq!(planned(short), Some(Duration::from_secs(1500)));
    }

    /// A pomodoro the user did not put a length on runs for whatever the app is
    /// set to, which only the app knows.
    #[test]
    fn a_start_with_no_length_asks_for_no_particular_length() {
        let cli = Cli::parse_from(["wabi", "start"]);

        match cli.command {
            Command::Start(args) => {
                let options = StartOptions::from(args);
                assert_eq!(options.planned, None);
                assert_eq!(options.label, None);
                assert_eq!(options.phase, Some(Phase::Work));
            }
            _ => unreachable!("parsed as start"),
        }
    }

    #[test]
    fn monday_is_its_own_week_start() {
        assert_eq!(monday_of(date("2026-08-10"), 0), date("2026-08-10"));
    }

    /// The app's week view is Monday-start, so Sunday belongs to the week that
    /// began six days earlier rather than to the one about to start.
    #[test]
    fn sunday_belongs_to_the_week_that_began_six_days_before() {
        assert_eq!(monday_of(date("2026-08-16"), 0), date("2026-08-10"));
    }

    #[test]
    fn going_back_a_week_lands_on_the_monday_before() {
        assert_eq!(monday_of(date("2026-08-16"), 1), date("2026-08-03"));
        assert_eq!(monday_of(date("2026-08-16"), 2), date("2026-07-27"));
    }

    #[test]
    fn a_week_back_crosses_a_year_boundary_cleanly() {
        assert_eq!(monday_of(date("2027-01-03"), 1), date("2026-12-21"));
    }

    #[test]
    fn midnight_is_the_first_moment_of_the_local_day() {
        let start = local_midnight(date("2026-08-16"));

        assert_eq!(start.date_naive(), date("2026-08-16"));
        // Whatever the machine's zone, the day starts at 00:00 in it -- except
        // in the zones that skip that hour, where it starts as early as one that
        // exists. Either way it is never the day before.
        assert!(start.time() < NaiveTime::from_hms_opt(4, 0, 0).unwrap_or(NaiveTime::MIN));
    }

    #[test]
    fn consecutive_days_meet_without_a_gap_or_an_overlap() {
        let first = local_midnight(date("2026-08-16"));
        let second = local_midnight(date("2026-08-17"));

        assert!(second > first);
        assert_eq!(second.date_naive(), date("2026-08-17"));
    }

    #[test]
    fn an_explicit_vault_beats_both_of_the_others() {
        let path = choose_vault_path(
            Some(PathBuf::from("/explicit")),
            Some(PathBuf::from("/env")),
            || Ok(Some(PathBuf::from("/remembered"))),
        )
        .expect("choose");

        assert_eq!(path, PathBuf::from("/explicit"));
    }

    #[test]
    fn the_environment_beats_what_the_app_remembers() {
        let path = choose_vault_path(None, Some(PathBuf::from("/env")), || {
            Ok(Some(PathBuf::from("/remembered")))
        })
        .expect("choose");

        assert_eq!(path, PathBuf::from("/env"));
    }

    #[test]
    fn the_remembered_vault_is_the_fallback() {
        let path = choose_vault_path(None, None, || Ok(Some(PathBuf::from("/remembered"))))
            .expect("choose");

        assert_eq!(path, PathBuf::from("/remembered"));
    }

    /// `--vault` has to stay usable as the way *out* of a malformed
    /// `settings.toml`, which it only is if reading that file is skipped
    /// entirely rather than read and ignored.
    #[test]
    fn settings_are_not_read_at_all_when_something_more_specific_answered() {
        let consulted = Cell::new(false);

        choose_vault_path(Some(PathBuf::from("/explicit")), None, || {
            consulted.set(true);
            Err(AppError::VaultUnset)
        })
        .expect("choose");

        assert!(!consulted.get(), "settings.toml was read anyway");
    }

    #[test]
    fn nothing_anywhere_is_an_actionable_message_rather_than_a_guess() {
        let err = choose_vault_path(None, None, || Ok(None)).expect_err("should fail");

        assert!(matches!(err, AppError::VaultUnset), "{err:?}");
        // The one error here whose text is the whole point of it.
        assert!(err.to_string().contains("--vault"), "{err}");
    }
}
