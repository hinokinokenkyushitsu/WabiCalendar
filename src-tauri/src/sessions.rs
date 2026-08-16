//! The pomodoro half of the vault: `sessions/YYYY-MM-DD.jsonl`, one line per
//! segment that ended.
//!
//! Append-only by construction. A finished pomodoro is a fact about a day that
//! has already happened, so nothing here ever rewrites or removes a line —
//! which is also why this half needs none of the shard-juggling the calendar
//! does. A session lives in the file for the *local* date it started on,
//! because that is the day the user would go looking in; that is why
//! [`Sessions`] is generic over the zone in exactly the way [`crate::calendar::Calendars`]
//! is, with the tests pinning a fixed offset and the app proper using `Local`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Datelike, FixedOffset, Local, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::fs_atomic::{append_jsonl, read_jsonl};
use crate::timer::{Ended, Outcome, Phase};
use crate::vault::Vault;

/// How far before the window a session might have started and still reach into
/// it.
///
/// Files are keyed on the date a session *started*, so one running through
/// midnight is filed under the day before. A pomodoro is minutes long, so a
/// single extra day is a wide margin; nothing shorter than 24 hours can escape
/// it.
const REACH_BACK_DAYS: i64 = 1;

/// One segment that ran, as it sits on disk.
///
/// The field names are snake_case because this is the format documented for the
/// user's own files, not a wire type — [`SessionView`] is what the frontend
/// sees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    /// Work or break. The same enum the timer counts in, so the two can never
    /// drift apart.
    pub kind: Phase,
    pub planned_sec: u64,
    /// What actually got counted, by the monotonic clock. Not the wall-clock
    /// gap between the two stamps below: a segment that was paused for ten
    /// minutes spans more wall time than it counted, and both facts are true.
    pub actual_sec: u64,
    pub started_at: DateTime<FixedOffset>,
    pub ended_at: DateTime<FixedOffset>,
    pub outcome: Outcome,
    /// Nothing writes this yet — there is no way to label a pomodoro in v1 —
    /// but the field is written out as `null` rather than omitted so that every
    /// line has the same shape for anyone reading the file by hand.
    #[serde(default)]
    pub label: Option<String>,
}

/// One session as the week view receives it. Mirrored by hand in
/// `src/types/session.ts`.
///
/// Separate from [`Session`] only because the two have different contracts: the
/// record on disk is snake_case, and everything the frontend sees is camelCase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub id: String,
    pub kind: Phase,
    pub planned_sec: u64,
    pub actual_sec: u64,
    pub started_at: DateTime<FixedOffset>,
    pub ended_at: DateTime<FixedOffset>,
    pub outcome: Outcome,
    pub label: Option<String>,
}

impl From<&Session> for SessionView {
    fn from(session: &Session) -> Self {
        Self {
            id: session.id.clone(),
            kind: session.kind,
            planned_sec: session.planned_sec,
            actual_sec: session.actual_sec,
            started_at: session.started_at,
            ended_at: session.ended_at,
            outcome: session.outcome,
            label: session.label.clone(),
        }
    }
}

/// Reads and writes the `sessions/` half of one vault.
pub struct Sessions<'a, Z: TimeZone> {
    vault: &'a Vault,
    /// Decides which day file a session belongs in, and the offset its
    /// timestamps are written with.
    tz: Z,
}

impl<'a> Sessions<'a, Local> {
    pub fn local(vault: &'a Vault) -> Self {
        Self { vault, tz: Local }
    }
}

impl<'a, Z: TimeZone> Sessions<'a, Z> {
    pub fn new(vault: &'a Vault, tz: Z) -> Self {
        Self { vault, tz }
    }

    /// Write down a segment the timer just closed.
    pub fn record(&self, ended: &Ended) -> Result<Session> {
        let session = Session {
            id: new_id(),
            kind: ended.phase,
            planned_sec: ended.planned_sec,
            actual_sec: ended.actual_sec,
            started_at: self.stamp(ended.started_at),
            ended_at: self.stamp(ended.ended_at),
            outcome: ended.outcome,
            label: None,
        };

        append_jsonl(&self.file(session.started_at), &session)?;
        Ok(session)
    }

    /// Every session overlapping `[from, to)`, earliest first.
    pub fn range(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<Session>> {
        let last = to.with_timezone(&self.tz).date_naive();
        let mut day =
            from.with_timezone(&self.tz).date_naive() - chrono::Duration::days(REACH_BACK_DAYS);

        let mut out = Vec::new();
        while day <= last {
            // A damaged line costs only itself: `read_jsonl` separates the ones
            // it could not decode, and one bad record must not take a whole
            // day's pomodoros down with it.
            let read = read_jsonl::<Session>(&self.day_file(day))?;
            out.extend(
                read.records
                    .into_iter()
                    // Half-open at both ends, so a session ending exactly when
                    // the week begins belongs to the week before.
                    .filter(|s| s.ended_at.to_utc() > from && s.started_at.to_utc() < to),
            );

            let Some(next) = day.succ_opt() else { break };
            day = next;
        }

        out.sort_by(|a, b| {
            a.started_at
                .cmp(&b.started_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }

    /// An instant as the vault's local zone would write it: RFC 3339 carrying a
    /// real offset, never a bare local time.
    fn stamp(&self, at: SystemTime) -> DateTime<FixedOffset> {
        DateTime::<Utc>::from(at)
            .with_timezone(&self.tz)
            .fixed_offset()
    }

    fn file(&self, started_at: DateTime<FixedOffset>) -> std::path::PathBuf {
        self.day_file(started_at.with_timezone(&self.tz).date_naive())
    }

    fn day_file(&self, day: NaiveDate) -> std::path::PathBuf {
        self.vault.sessions_file(day.year(), day.month(), day.day())
    }
}

/// Unique enough for a single-user local log, and stable once written.
///
/// Same scheme, and same reasoning, as `calendar::new_uid`: a UUID dependency
/// buys nothing for a string that is only ever compared for equality.
fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);

    format!("{nanos}-{}-{seq}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::time::Duration;

    use chrono::Timelike;
    use tempfile::TempDir;

    fn jst() -> FixedOffset {
        FixedOffset::east_opt(9 * 3600).expect("valid offset")
    }

    fn vault() -> (TempDir, Vault) {
        let dir = TempDir::new().expect("tempdir");
        let (vault, _) = Vault::open(dir.path()).expect("open");
        (dir, vault)
    }

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("valid timestamp")
            .with_timezone(&Utc)
    }

    fn at(text: &str) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(utc(text).timestamp() as u64)
    }

    fn ended(phase: Phase, outcome: Outcome, start: &str, end: &str, actual: u64) -> Ended {
        Ended {
            phase,
            planned_sec: 1500,
            actual_sec: actual,
            started_at: at(start),
            ended_at: at(end),
            outcome,
        }
    }

    fn work(start: &str, end: &str) -> Ended {
        ended(Phase::Work, Outcome::Completed, start, end, 1500)
    }

    #[test]
    fn a_recorded_session_is_readable_again() {
        let (_dir, vault) = vault();
        let sessions = Sessions::new(&vault, jst());

        let written = sessions
            .record(&work("2026-07-23T05:00:00Z", "2026-07-23T05:25:00Z"))
            .expect("record");

        let found = sessions
            .range(utc("2026-07-22T15:00:00Z"), utc("2026-07-23T15:00:00Z"))
            .expect("range");

        assert_eq!(found, vec![written]);
    }

    /// The on-disk shape is documented for the user, so it is part of the
    /// contract rather than an implementation detail.
    #[test]
    fn a_written_line_has_the_documented_shape() {
        let (_dir, vault) = vault();
        Sessions::new(&vault, jst())
            .record(&work("2026-07-23T05:00:00Z", "2026-07-23T05:25:00Z"))
            .expect("record");

        let text = fs::read_to_string(vault.sessions_file(2026, 7, 23)).expect("read");
        let line = text.strip_suffix('\n').expect("terminated");

        assert!(line.contains(r#""kind":"work""#), "{line}");
        assert!(line.contains(r#""planned_sec":1500"#), "{line}");
        assert!(line.contains(r#""actual_sec":1500"#), "{line}");
        assert!(line.contains(r#""outcome":"completed""#), "{line}");
        assert!(line.contains(r#""label":null"#), "{line}");
        // RFC 3339 carrying the offset, not a bare local time and not UTC.
        assert!(
            line.contains(r#""started_at":"2026-07-23T14:00:00+09:00""#),
            "{line}"
        );
        assert!(
            line.contains(r#""ended_at":"2026-07-23T14:25:00+09:00""#),
            "{line}"
        );
    }

    /// The file is the day the user would go looking in, not the UTC one.
    #[test]
    fn a_session_lands_in_the_file_for_its_local_start_date() {
        let (_dir, vault) = vault();

        // 00:10 on 24 July in Tokyo, which is still 23 July in UTC.
        Sessions::new(&vault, jst())
            .record(&work("2026-07-23T15:10:00Z", "2026-07-23T15:35:00Z"))
            .expect("record");

        assert!(vault.sessions_file(2026, 7, 24).is_file());
        assert!(!vault.sessions_file(2026, 7, 23).exists());
    }

    #[test]
    fn a_session_running_through_midnight_is_found_from_the_day_it_ends_in() {
        let (_dir, vault) = vault();
        let sessions = Sessions::new(&vault, jst());

        // 23:50 to 00:15 Tokyo time, so the record sits in the 23 July file.
        sessions
            .record(&work("2026-07-23T14:50:00Z", "2026-07-23T15:15:00Z"))
            .expect("record");
        assert!(vault.sessions_file(2026, 7, 23).is_file());

        // A window covering only 24 July, local.
        let found = sessions
            .range(utc("2026-07-23T15:00:00Z"), utc("2026-07-24T15:00:00Z"))
            .expect("range");

        assert_eq!(found.len(), 1);
    }

    #[test]
    fn sessions_outside_the_window_are_left_out() {
        let (_dir, vault) = vault();
        let sessions = Sessions::new(&vault, jst());

        sessions
            .record(&work("2026-07-20T05:00:00Z", "2026-07-20T05:25:00Z"))
            .expect("before");
        let inside = sessions
            .record(&work("2026-07-23T05:00:00Z", "2026-07-23T05:25:00Z"))
            .expect("inside");
        sessions
            .record(&work("2026-07-30T05:00:00Z", "2026-07-30T05:25:00Z"))
            .expect("after");

        let found = sessions
            .range(utc("2026-07-22T15:00:00Z"), utc("2026-07-23T15:00:00Z"))
            .expect("range");

        assert_eq!(found, vec![inside]);
    }

    #[test]
    fn sessions_come_back_in_chronological_order() {
        let (_dir, vault) = vault();
        let sessions = Sessions::new(&vault, jst());

        for start in ["07:00", "05:00", "06:00"] {
            sessions
                .record(&work(
                    &format!("2026-07-23T{start}:00Z"),
                    &format!("2026-07-23T{start}:00Z"),
                ))
                .expect("record");
        }

        let found = sessions
            .range(utc("2026-07-22T15:00:00Z"), utc("2026-07-23T15:00:00Z"))
            .expect("range");

        let hours: Vec<_> = found.iter().map(|s| s.started_at.hour()).collect();
        assert_eq!(hours, vec![14, 15, 16]);
    }

    #[test]
    fn every_outcome_round_trips() {
        let (_dir, vault) = vault();
        let sessions = Sessions::new(&vault, jst());

        for (i, outcome) in [Outcome::Completed, Outcome::Aborted, Outcome::Invalidated]
            .into_iter()
            .enumerate()
        {
            sessions
                .record(&ended(
                    Phase::Break,
                    outcome,
                    &format!("2026-07-23T0{i}:00:00Z"),
                    &format!("2026-07-23T0{i}:05:00Z"),
                    300,
                ))
                .expect("record");
        }

        let found = sessions
            .range(utc("2026-07-22T15:00:00Z"), utc("2026-07-23T15:00:00Z"))
            .expect("range");

        let outcomes: Vec<_> = found.iter().map(|s| s.outcome).collect();
        assert_eq!(
            outcomes,
            vec![Outcome::Completed, Outcome::Aborted, Outcome::Invalidated]
        );
    }

    /// Invariant #3, from the reading side: one damaged line must not cost the
    /// rest of the day.
    #[test]
    fn a_corrupt_line_costs_only_itself() {
        let (_dir, vault) = vault();
        let sessions = Sessions::new(&vault, jst());

        sessions
            .record(&work("2026-07-23T05:00:00Z", "2026-07-23T05:25:00Z"))
            .expect("record");

        // What a hand-edit gone wrong leaves behind, followed by a good record.
        let path = vault.sessions_file(2026, 7, 23);
        let mut text = fs::read_to_string(&path).expect("read");
        text.push_str("{ half a record\n");
        fs::write(&path, text).expect("write");

        sessions
            .record(&work("2026-07-23T06:00:00Z", "2026-07-23T06:25:00Z"))
            .expect("record after damage");

        let found = sessions
            .range(utc("2026-07-22T15:00:00Z"), utc("2026-07-23T15:00:00Z"))
            .expect("range");

        assert_eq!(found.len(), 2);
        // And the bad line is still on disk: the write path never destroys bytes.
        assert!(fs::read_to_string(&path)
            .expect("read")
            .contains("{ half a record"));
    }

    /// Invariant #1: every path comes from `Vault`, and nothing here writes
    /// outside `sessions/`.
    #[test]
    fn recording_leaves_no_debris_and_touches_nothing_else() {
        let (_dir, vault) = vault();
        Sessions::new(&vault, jst())
            .record(&work("2026-07-23T05:00:00Z", "2026-07-23T05:25:00Z"))
            .expect("record");

        let strays: Vec<_> = fs::read_dir(vault.sessions_dir())
            .expect("readable")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp-"))
            .collect();
        assert!(strays.is_empty(), "left behind: {strays:?}");

        assert_eq!(
            fs::read_dir(vault.index_dir()).expect("readable").count(),
            0,
            "sessions must not need an index"
        );
    }

    #[test]
    fn a_day_that_was_never_written_reads_as_empty() {
        let (_dir, vault) = vault();

        let found = Sessions::new(&vault, jst())
            .range(utc("2026-07-22T15:00:00Z"), utc("2026-07-23T15:00:00Z"))
            .expect("range");

        assert!(found.is_empty());
    }
}
