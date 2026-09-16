//! Turning a window of the vault into lines of text.
//!
//! The arithmetic here is a deliberate mirror of `src/lib/summary.ts`. The two
//! have to agree: a user who reads "focused 5h15m" in the week view and then
//! `wabi log --week` is entitled to the same number, and the moment they
//! disagree neither can be trusted. Any change to one belongs in the other.
//!
//! Output is plain ASCII on purpose. The labels are the user's own text and may
//! well be CJK, so every column of fixed width comes *before* the free text on
//! a line and nothing has to guess how wide a glyph is. For the same reason the
//! separators are `-` and `|` rather than the en dash and middot the UI uses:
//! both are East Asian Ambiguous, and a terminal set for CJK draws them double
//! width, which would pull every rule out of line.

use chrono::{DateTime, Local, TimeZone};

use crate::calendar::CalEvent;
use crate::sessions::Session;
use crate::timer::{Outcome, Phase};

const SEC_PER_MINUTE: i64 = 60;

/// Monday to Sunday, matching the app's week view, which is not configurable.
pub const DAYS_PER_WEEK: usize = 7;

/// The width of one row of the week table: the four columns below, added up.
const WEEK_ROW_WIDTH: usize = 7 + 9 + 10 + 8;

/// Append one line, without the padding that ran off the end of it.
///
/// Every row here pads a column that may be the last thing on the line — an
/// unlabelled session, an all-day event with no duration — and trailing spaces
/// are invisible until they show up in a diff or a test fixture.
fn line(out: &mut String, text: &str) {
    out.push_str(text.trim_end());
    out.push('\n');
}

/// Mirrors `WeekSummary` in `src/lib/summary.ts`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    /// Planned seconds inside the window.
    pub planned_sec: i64,
    /// Seconds of work the timer actually counted.
    pub focused_sec: i64,
    /// `focused_sec / planned_sec`, or `None` when nothing was planned.
    ///
    /// `None` rather than zero: a day with no plan has no completion rate, and
    /// printing "0%" for one would read as a failure that never happened. Not
    /// capped at 1 either — outworking the plan is a real thing that happened.
    pub ratio: Option<f64>,
}

/// Seconds of `[start, end)` that fall inside `[from, to)`.
fn overlap_sec<Z: TimeZone>(
    start: DateTime<Z>,
    end: DateTime<Z>,
    from: DateTime<Local>,
    to: DateTime<Local>,
) -> i64 {
    let low = start.timestamp().max(from.timestamp());
    let high = end.timestamp().min(to.timestamp());
    (high - low).max(0)
}

/// Does this session count towards time actually spent focusing?
///
/// Work only — a break is not focus — and never an invalidated one: the app
/// decided it could not vouch for that stretch, so counting it would be the app
/// contradicting its own warning.
pub fn is_focus(session: &Session) -> bool {
    session.kind == Phase::Work && session.outcome != Outcome::Invalidated
}

/// Add up one window.
///
/// The two halves are selected differently because the two numbers mean
/// different things:
///
/// - A planned block is *clipped* to the window. An event may legitimately run
///   for three days, and only the part inside this window was planned for it.
/// - A session is taken whole if it *started* inside the window, because
///   `actual_sec` is counted time and there is no honest way to cut it in half:
///   a session that was paused does not spread its counted seconds evenly over
///   the wall clock.
///
/// All-day events are left out entirely — a single one would add 24 hours and
/// drown every real block beside it.
pub fn summarise(
    events: &[CalEvent],
    sessions: &[Session],
    from: DateTime<Local>,
    to: DateTime<Local>,
) -> Summary {
    let planned_sec = events
        .iter()
        .filter(|event| !event.all_day)
        .map(|event| overlap_sec(event.start, event.end, from, to))
        .sum();

    let focused_sec = sessions
        .iter()
        .filter(|session| {
            is_focus(session) && session.started_at >= from && session.started_at < to
        })
        .map(|session| session.actual_sec as i64)
        .sum();

    Summary {
        planned_sec,
        focused_sec,
        ratio: match planned_sec {
            0 => None,
            planned => Some(focused_sec as f64 / planned as f64),
        },
    }
}

/// "8h15m", "45m", "0m" — a duration at a glance rather than to the second.
///
/// Rounded down to the minute: claiming a minute the user has not finished
/// would be the one direction that flatters.
pub fn format_duration(total_sec: i64) -> String {
    let minutes = total_sec.max(0) / SEC_PER_MINUTE;
    let (hours, rest) = (minutes / 60, minutes % 60);
    if hours == 0 {
        format!("{rest}m")
    } else {
        format!("{hours}h{rest:02}m")
    }
}

/// "66%", or "-" when there is no plan to measure against.
pub fn format_ratio(ratio: Option<f64>) -> String {
    match ratio {
        Some(ratio) => format!("{}%", (ratio * 100.0).round()),
        None => "-".to_string(),
    }
}

/// "09:05-09:30" in the vault's local zone.
fn format_span<Z: TimeZone>(start: DateTime<Z>, end: DateTime<Z>) -> String {
    let start = start.with_timezone(&Local);
    let end = end.with_timezone(&Local);
    format!("{}-{}", start.format("%H:%M"), end.format("%H:%M"))
}

/// How a segment ended, in the width the column reserves.
fn outcome_word(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Completed => "completed",
        Outcome::Aborted => "aborted",
        Outcome::Invalidated => "invalidated",
    }
}

fn phase_word(phase: Phase) -> &'static str {
    match phase {
        Phase::Work => "work",
        Phase::Break => "break",
    }
}

/// One day in full: what was planned, what actually ran, and the totals.
pub fn day(
    events: &[CalEvent],
    sessions: &[Session],
    from: DateTime<Local>,
    to: DateTime<Local>,
) -> String {
    let mut out = String::new();
    line(&mut out, &from.format("%A, %-d %B %Y").to_string());
    out.push('\n');

    line(&mut out, "Planned");
    if events.is_empty() {
        line(&mut out, "  (none)");
    }
    for event in events {
        if event.all_day {
            // No span and no duration: an all-day event is a label on the day,
            // and it is left out of the totals below for the same reason.
            line(
                &mut out,
                &format!("  {:<11}  {:>6}  {}", "all day", "", event.summary),
            );
        } else {
            line(
                &mut out,
                &format!(
                    "  {:<11}  {:>6}  {}",
                    format_span(event.start, event.end),
                    format_duration(event.end.timestamp() - event.start.timestamp()),
                    event.summary,
                ),
            );
        }
    }

    out.push('\n');
    line(&mut out, "Focus");
    if sessions.is_empty() {
        line(&mut out, "  (none)");
    }
    for session in sessions {
        line(
            &mut out,
            &format!(
                "  {:<11}  {:>6}  {:<5}  {:<11}  {}",
                format_span(session.started_at, session.ended_at),
                format_duration(session.actual_sec as i64),
                phase_word(session.kind),
                outcome_word(session.outcome),
                session.label.as_deref().unwrap_or(""),
            ),
        );
    }

    out.push('\n');
    line(
        &mut out,
        &summary_line(&summarise(events, sessions, from, to)),
    );
    out
}

/// "planned 2h30m | focused 58m | 39%"
pub fn summary_line(summary: &Summary) -> String {
    format!(
        "planned {} | focused {} | {}",
        format_duration(summary.planned_sec),
        format_duration(summary.focused_sec),
        format_ratio(summary.ratio),
    )
}

/// One row per day, then the week's own total.
///
/// Deliberately an overview and not a listing: seven days of every block would
/// scroll off the screen, and `wabi today` is there for the detail.
pub fn week(
    events: &[CalEvent],
    sessions: &[Session],
    days: &[DateTime<Local>; DAYS_PER_WEEK + 1],
) -> String {
    let mut out = String::new();
    line(&mut out, &week_title(days));
    out.push('\n');
    line(
        &mut out,
        &format!("  {:<7}{:>9}{:>10}{:>8}", "", "planned", "focused", "ratio"),
    );

    for index in 0..DAYS_PER_WEEK {
        let (from, to) = (days[index], days[index + 1]);
        let summary = summarise(events, sessions, from, to);
        line(
            &mut out,
            &format!(
                "  {:<7}{:>9}{:>10}{:>8}",
                from.format("%a %-d"),
                format_duration(summary.planned_sec),
                format_duration(summary.focused_sec),
                format_ratio(summary.ratio),
            ),
        );
    }

    let total = summarise(events, sessions, days[0], days[DAYS_PER_WEEK]);
    line(&mut out, &format!("  {}", "-".repeat(WEEK_ROW_WIDTH)));
    line(
        &mut out,
        &format!(
            "  {:<7}{:>9}{:>10}{:>8}",
            "Total",
            format_duration(total.planned_sec),
            format_duration(total.focused_sec),
            format_ratio(total.ratio),
        ),
    );
    out
}

/// "Week of 10-16 August 2026", widening only as far as the dates force it.
fn week_title(days: &[DateTime<Local>; DAYS_PER_WEEK + 1]) -> String {
    let first = days[0];
    // The last *day*, not the exclusive end of the window: the final element is
    // the following Monday's midnight, which would name the wrong day and, at
    // the turn of a month, the wrong month.
    let last = days[DAYS_PER_WEEK - 1];

    let same_year = first.format("%Y").to_string() == last.format("%Y").to_string();
    let same_month = same_year && first.format("%m").to_string() == last.format("%m").to_string();

    if same_month {
        format!(
            "Week of {}-{}",
            first.format("%-d"),
            last.format("%-d %B %Y")
        )
    } else if same_year {
        format!(
            "Week of {} - {}",
            first.format("%-d %B"),
            last.format("%-d %B %Y")
        )
    } else {
        format!(
            "Week of {} - {}",
            first.format("%-d %B %Y"),
            last.format("%-d %B %Y")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::{DateTime, FixedOffset, NaiveDate, Utc};

    use crate::calendar::CalEvent;
    use crate::cli::local_midnight;
    use crate::sessions::Session;

    /// An instant, however the machine's zone happens to render it. Every
    /// comparison in this module is between instants, so the tests say nothing
    /// about which zone they run in.
    fn at(text: &str) -> DateTime<Local> {
        DateTime::parse_from_rfc3339(text)
            .expect("valid timestamp")
            .with_timezone(&Local)
    }

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("valid timestamp")
            .with_timezone(&Utc)
    }

    fn offset(text: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(text).expect("valid timestamp")
    }

    fn event(summary: &str, start: &str, end: &str) -> CalEvent {
        CalEvent {
            id: summary.to_string(),
            uid: summary.to_string(),
            summary: summary.to_string(),
            start: utc(start),
            end: utc(end),
            recurring: false,
            all_day: false,
        }
    }

    fn all_day(summary: &str, start: &str, end: &str) -> CalEvent {
        CalEvent {
            all_day: true,
            ..event(summary, start, end)
        }
    }

    fn session(kind: Phase, outcome: Outcome, start: &str, actual_sec: u64) -> Session {
        Session {
            id: start.to_string(),
            kind,
            planned_sec: 1500,
            actual_sec,
            started_at: offset(start),
            ended_at: offset(start) + chrono::Duration::seconds(actual_sec as i64),
            outcome,
            label: None,
        }
    }

    fn work(start: &str, actual_sec: u64) -> Session {
        session(Phase::Work, Outcome::Completed, start, actual_sec)
    }

    fn day_window() -> (DateTime<Local>, DateTime<Local>) {
        (at("2026-08-16T00:00:00Z"), at("2026-08-17T00:00:00Z"))
    }

    #[test]
    fn a_planned_block_is_clipped_to_the_window() {
        let (from, to) = day_window();
        // Starts six hours before the window and runs eight, so two hours of it
        // were planned for this day.
        let events = [event(
            "overnight",
            "2026-08-15T18:00:00Z",
            "2026-08-16T02:00:00Z",
        )];

        let summary = summarise(&events, &[], from, to);

        assert_eq!(summary.planned_sec, 2 * 3600);
    }

    #[test]
    fn a_block_entirely_outside_the_window_counts_for_nothing() {
        let (from, to) = day_window();
        let events = [event(
            "yesterday",
            "2026-08-15T09:00:00Z",
            "2026-08-15T10:00:00Z",
        )];

        assert_eq!(summarise(&events, &[], from, to).planned_sec, 0);
    }

    /// One all-day event would otherwise add 24 hours and drown every real
    /// block beside it.
    #[test]
    fn an_all_day_event_is_left_out_of_the_planned_total() {
        let (from, to) = day_window();
        let events = [
            all_day("休假", "2026-08-16T00:00:00Z", "2026-08-17T00:00:00Z"),
            event("写论文", "2026-08-16T09:00:00Z", "2026-08-16T10:00:00Z"),
        ];

        assert_eq!(summarise(&events, &[], from, to).planned_sec, 3600);
    }

    /// A session is taken whole if it *started* inside the window: `actual_sec`
    /// is counted time, and a paused segment does not spread it evenly over the
    /// wall clock, so there is no honest way to cut it in half.
    #[test]
    fn a_session_that_started_inside_the_window_is_counted_whole() {
        let (from, to) = day_window();
        // Starts ten minutes before midnight and counts a full 25.
        let sessions = [work("2026-08-16T23:50:00Z", 1500)];

        assert_eq!(summarise(&[], &sessions, from, to).focused_sec, 1500);
    }

    #[test]
    fn a_session_that_started_before_the_window_is_left_out_entirely() {
        let (from, to) = day_window();
        let sessions = [work("2026-08-15T23:50:00Z", 1500)];

        assert_eq!(summarise(&[], &sessions, from, to).focused_sec, 0);
    }

    /// A break is not focus, and an invalidated segment is time the app has
    /// already said it cannot vouch for.
    #[test]
    fn breaks_and_invalidated_segments_do_not_count_as_focus() {
        let (from, to) = day_window();
        let sessions = [
            work("2026-08-16T09:00:00Z", 1500),
            session(
                Phase::Break,
                Outcome::Completed,
                "2026-08-16T09:30:00Z",
                300,
            ),
            session(
                Phase::Work,
                Outcome::Invalidated,
                "2026-08-16T10:00:00Z",
                720,
            ),
            session(Phase::Work, Outcome::Aborted, "2026-08-16T11:00:00Z", 480),
        ];

        // The completed 25 and the aborted 8. An aborted pomodoro is time that
        // really was spent working.
        assert_eq!(summarise(&[], &sessions, from, to).focused_sec, 1500 + 480);
    }

    #[test]
    fn a_window_with_no_plan_has_no_completion_rate() {
        let (from, to) = day_window();
        let sessions = [work("2026-08-16T09:00:00Z", 1500)];

        let summary = summarise(&[], &sessions, from, to);

        // Not zero: a day with no plan did not fail to meet one.
        assert_eq!(summary.ratio, None);
        assert_eq!(format_ratio(summary.ratio), "-");
    }

    /// Outworking the plan is a real thing that happened, and the number should
    /// say so rather than stopping at 100%.
    #[test]
    fn the_ratio_is_not_capped_at_one() {
        let (from, to) = day_window();
        let events = [event(
            "写论文",
            "2026-08-16T09:00:00Z",
            "2026-08-16T09:30:00Z",
        )];
        let sessions = [work("2026-08-16T09:00:00Z", 3600)];

        let summary = summarise(&events, &sessions, from, to);

        assert_eq!(format_ratio(summary.ratio), "200%");
    }

    #[test]
    fn durations_read_the_way_the_week_view_writes_them() {
        assert_eq!(format_duration(0), "0m");
        assert_eq!(format_duration(45 * 60), "45m");
        assert_eq!(format_duration(60 * 60), "1h00m");
        assert_eq!(format_duration(8 * 3600 + 15 * 60), "8h15m");
        // Rounded down: claiming a minute the user has not finished is the one
        // direction that flatters.
        assert_eq!(format_duration(59), "0m");
        assert_eq!(format_duration(-10), "0m");
    }

    /// Padding that runs off the end of a line is invisible until it turns up
    /// in a diff.
    #[test]
    fn no_rendered_line_ends_in_whitespace() {
        let (from, to) = day_window();
        let events = [all_day(
            "休假",
            "2026-08-16T00:00:00Z",
            "2026-08-17T00:00:00Z",
        )];
        // Unlabelled, so the last column is empty and the padding before it is
        // what would show.
        let sessions = [work("2026-08-16T09:00:00Z", 1500)];

        let text = day(&events, &sessions, from, to);

        for line in text.lines() {
            assert_eq!(line, line.trim_end(), "trailing whitespace in {line:?}");
        }
    }

    #[test]
    fn a_day_with_nothing_in_it_says_so_in_both_halves() {
        let (from, to) = day_window();

        let text = day(&[], &[], from, to);

        assert_eq!(text.matches("(none)").count(), 2, "{text}");
        assert!(text.contains("planned 0m | focused 0m | -"), "{text}");
    }

    fn week_of(start: &str) -> [DateTime<Local>; DAYS_PER_WEEK + 1] {
        let monday = NaiveDate::parse_from_str(start, "%Y-%m-%d").expect("valid date");
        let mut days = [local_midnight(monday); DAYS_PER_WEEK + 1];
        for (index, slot) in days.iter_mut().enumerate() {
            *slot = local_midnight(monday + chrono::Duration::days(index as i64));
        }
        days
    }

    #[test]
    fn a_week_inside_one_month_names_it_once() {
        assert_eq!(
            week_title(&week_of("2026-08-10")),
            "Week of 10-16 August 2026"
        );
    }

    #[test]
    fn a_week_straddling_two_months_names_both() {
        assert_eq!(
            week_title(&week_of("2026-08-31")),
            "Week of 31 August - 6 September 2026"
        );
    }

    #[test]
    fn a_week_straddling_new_year_names_both_years() {
        assert_eq!(
            week_title(&week_of("2026-12-28")),
            "Week of 28 December 2026 - 3 January 2027"
        );
    }

    /// The seven day rows and the total have to be the same arithmetic, or the
    /// table would not add up.
    #[test]
    fn the_week_total_is_the_sum_of_its_days() {
        let days = week_of("2026-08-10");
        let events = [
            event("mon", "2026-08-10T09:00:00Z", "2026-08-10T10:00:00Z"),
            event("wed", "2026-08-12T09:00:00Z", "2026-08-12T11:00:00Z"),
        ];
        let sessions = [work("2026-08-10T09:00:00Z", 1500)];

        let total = summarise(&events, &sessions, days[0], days[DAYS_PER_WEEK]);
        let summed: i64 = (0..DAYS_PER_WEEK)
            .map(|i| summarise(&events, &sessions, days[i], days[i + 1]).planned_sec)
            .sum();

        assert_eq!(total.planned_sec, summed);
        assert_eq!(total.planned_sec, 3 * 3600);
        assert_eq!(total.focused_sec, 1500);
    }
}
