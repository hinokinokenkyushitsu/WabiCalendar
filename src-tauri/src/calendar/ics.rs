//! Between the VEVENTs on disk and the flat blocks the week view draws.
//!
//! We *write* only RFC 5545 form #2 (`20260723T050000Z`), because that is the
//! form every importer agrees on. We *read* all four, because the file belongs
//! to the user: it can be hand-edited or come from somewhere else entirely.

use chrono::{DateTime, NaiveDateTime, TimeDelta, TimeZone, Utc};
use icalendar::{CalendarDateTime, Component, DatePerhapsTime, Event, EventLike, Tz as RruleTz};
use serde::{Deserialize, Serialize};

/// What a VEVENT without a DTEND is worth. RFC 5545 leaves a missing DTEND on a
/// DATE-TIME event meaning "an instant"; a zero-height block is unusable, so we
/// draw the length a calendar app conventionally assumes instead.
const IMPLIED_DURATION: TimeDelta = TimeDelta::hours(1);

/// The same, for a DATE-valued (all-day) event: RFC 5545 does define this one.
const IMPLIED_ALL_DAY_DURATION: TimeDelta = TimeDelta::days(1);

/// Enough occurrences for any one week. `all()` stops as soon as the rule walks
/// past the end of the window, so this only bites on a rule that repeats every
/// few seconds.
const OCCURRENCE_LIMIT: u16 = 2000;

/// How far back a recurrence search may reach to catch an occurrence that starts
/// before the window and runs into it. Also what keeps a nonsense DTEND far in
/// the future from overflowing the search bound.
const MAX_WIDEN: TimeDelta = TimeDelta::days(366);

/// One block on the grid. Mirrored by hand in `src/types/calendar.ts`.
///
/// This is deliberately flatter than a VEVENT: an occurrence of a repeating
/// event is its own `CalEvent`, and the recurrence rule that produced it never
/// reaches the frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalEvent {
    /// Unique among the blocks of one query — the frontend keys on it. For a
    /// repeating event this is the UID plus the occurrence's start, so it is
    /// *not* interchangeable with `uid`.
    pub id: String,
    /// The VEVENT's UID, and the handle every edit is addressed by. Empty when
    /// the VEVENT had none, which makes the block read-only: we would have no
    /// way to find it again.
    pub uid: String,
    pub summary: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    /// Produced by an RRULE or RDATE. Read-only in v1 (`AppError::RecurringNotEditable`).
    pub recurring: bool,
    /// A DATE-valued event. Drawn in the strip above the grid, not in it.
    pub all_day: bool,
}

/// The fields a drag can change. Everything else a VEVENT carries is left alone.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventDraft {
    pub summary: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// A brand new VEVENT.
pub fn build(uid: &str, draft: &EventDraft) -> Event {
    Event::new()
        .uid(uid)
        .summary(&draft.summary)
        .starts(draft.start)
        .ends(draft.end)
        // Required by RFC 5545, and some importers do reject a VEVENT without it.
        .timestamp(Utc::now())
        .done()
}

/// Change only what the drag changed.
///
/// In place rather than rebuilt from the draft: a VEVENT the user wrote by hand
/// or imported from elsewhere can carry DESCRIPTION, LOCATION, CATEGORIES, a
/// VALARM, arbitrary X- properties. None of that is ours to drop because a block
/// moved by fifteen minutes.
pub fn apply(event: &mut Event, draft: &EventDraft) {
    event.summary(&draft.summary);
    event.starts(draft.start);
    event.ends(draft.end);
    event.sequence(event.get_sequence().unwrap_or(0).saturating_add(1));
    event.last_modified(Utc::now());
}

/// How recent a VEVENT claims to be, for choosing between two copies of one UID.
///
/// RFC 5545's own answer: SEQUENCE counts revisions and LAST-MODIFIED breaks a
/// tie, with DTSTAMP standing in when the event was never edited. Since [`apply`]
/// bumps both, the copy a cross-shard move wrote always outranks the one a crash
/// left behind -- which the shard an event sits in cannot tell us, because the
/// stale copy still has the old start and so still looks at home.
pub fn revision(event: &Event) -> (u32, Option<DateTime<Utc>>) {
    (
        event.get_sequence().unwrap_or(0),
        event.get_last_modified().or_else(|| event.get_timestamp()),
    )
}

/// The instant a VEVENT begins, for deciding which shard it belongs in.
pub fn start_of<Z: TimeZone>(event: &Event, tz: &Z) -> Option<DateTime<Utc>> {
    instant(&event.get_start()?, tz)
}

/// Does this VEVENT repeat?
pub fn is_recurring(event: &Event) -> bool {
    event.property_value("RRULE").is_some() || event.multi_properties().contains_key("RDATE")
}

/// Every block `event` contributes to `[from, to)`, appended to `out`.
///
/// `fallback_key` is used in place of the UID for a VEVENT that has none, so
/// that even an unaddressable event still gets a stable, unique `id` to render
/// under. `tz` interprets the forms that carry no zone of their own.
pub fn occurrences<Z: TimeZone>(
    event: &Event,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    tz: &Z,
    fallback_key: &str,
    out: &mut Vec<CalEvent>,
) {
    let Some(start_value) = event.get_start() else {
        return;
    };
    let Some(start) = instant(&start_value, tz) else {
        return;
    };
    let all_day = matches!(start_value, DatePerhapsTime::Date(_));

    let implied = if all_day {
        IMPLIED_ALL_DAY_DURATION
    } else {
        IMPLIED_DURATION
    };
    let end = event
        .get_end()
        .and_then(|value| instant(&value, tz))
        // A DTEND at or before DTSTART is not something we can draw, so fall
        // back to the implied length rather than render nothing.
        .filter(|end| *end > start)
        .unwrap_or(start + implied);
    let duration = end - start;

    let uid = event.get_uid().unwrap_or_default();
    let key = if uid.is_empty() { fallback_key } else { uid };
    let summary = event.get_summary().unwrap_or_default().to_string();

    let mut push = |id: String, start: DateTime<Utc>, end: DateTime<Utc>, recurring: bool| {
        if start < to && end > from {
            out.push(CalEvent {
                id,
                uid: uid.to_string(),
                summary: summary.clone(),
                start,
                end,
                recurring,
                all_day,
            });
        }
    };

    if !is_recurring(event) {
        push(key.to_string(), start, end, false);
        return;
    }

    let Ok(set) = event.get_recurrence() else {
        // A malformed RRULE must not cost the user the event it is attached to.
        // The block still counts as recurring, so it stays read-only and the
        // rule cannot be silently dropped by an edit from this side.
        push(key.to_string(), start, end, true);
        return;
    };

    // An occurrence can begin before the window and reach into it, so search
    // from one duration earlier and filter on the way out.
    let widen = duration.min(MAX_WIDEN);
    let search_from = from.checked_sub_signed(widen).unwrap_or(from);

    for at in set
        .after(to_rrule(search_from))
        .before(to_rrule(to))
        .all(OCCURRENCE_LIMIT)
        .dates
    {
        let at = at.with_timezone(&Utc);
        push(format!("{key}#{}", at.timestamp()), at, at + duration, true);
    }
}

/// Resolve one of the four RFC 5545 date-time forms to an instant.
fn instant<Z: TimeZone>(value: &DatePerhapsTime, tz: &Z) -> Option<DateTime<Utc>> {
    match value {
        DatePerhapsTime::DateTime(CalendarDateTime::Utc(at)) => Some(*at),
        DatePerhapsTime::DateTime(CalendarDateTime::WithTimezone { date_time, tzid }) => {
            let zone: chrono_tz::Tz = tzid.parse().ok()?;
            resolve(*date_time, &zone)
        }
        // No zone at all: RFC 5545 says such a time floats to whoever is reading
        // it, and that is us.
        DatePerhapsTime::DateTime(CalendarDateTime::Floating(date_time)) => resolve(*date_time, tz),
        DatePerhapsTime::Date(date) => resolve(date.and_hms_opt(0, 0, 0)?, tz),
    }
}

/// A wall-clock reading in `tz`, as an instant.
///
/// `earliest` because a time in the hour a DST change repeats is genuinely two
/// instants and the first is the better guess. A time in the hour a DST change
/// *skips* is no instant at all; rather than drop the event we shift it past the
/// gap, since a block drawn an hour late beats a block the user cannot see.
fn resolve<Z: TimeZone>(naive: NaiveDateTime, tz: &Z) -> Option<DateTime<Utc>> {
    if let Some(at) = tz.from_local_datetime(&naive).earliest() {
        return Some(at.with_timezone(&Utc));
    }
    tz.from_local_datetime(&(naive + TimeDelta::hours(1)))
        .earliest()
        .map(|at| at.with_timezone(&Utc))
}

fn to_rrule(at: DateTime<Utc>) -> DateTime<RruleTz> {
    RruleTz::UTC.from_utc_datetime(&at.naive_utc())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::str::FromStr;

    use chrono::FixedOffset;
    use icalendar::Calendar;

    fn jst() -> FixedOffset {
        FixedOffset::east_opt(9 * 3600).expect("valid offset")
    }

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("valid timestamp")
            .with_timezone(&Utc)
    }

    /// Wrap `body` in a VEVENT and parse it back out, so the tests exercise the
    /// same path a hand-edited file takes.
    fn event(body: &str) -> Event {
        let text =
            format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n{body}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n");
        Calendar::from_str(&text)
            .expect("parses")
            .components
            .into_iter()
            .find_map(|c| c.as_event().cloned())
            .expect("one event")
    }

    fn blocks(event: &Event, from: &str, to: &str) -> Vec<CalEvent> {
        let mut out = Vec::new();
        occurrences(event, utc(from), utc(to), &jst(), "anon", &mut out);
        out
    }

    #[test]
    fn a_utc_datetime_is_taken_as_written() {
        let found = blocks(
            &event("UID:a\r\nSUMMARY:写论文\r\nDTSTART:20260723T050000Z\r\nDTEND:20260723T060000Z"),
            "2026-07-23T00:00:00Z",
            "2026-07-24T00:00:00Z",
        );

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].uid, "a");
        assert_eq!(found[0].summary, "写论文");
        assert_eq!(found[0].start, utc("2026-07-23T05:00:00Z"));
        assert_eq!(found[0].end, utc("2026-07-23T06:00:00Z"));
        assert!(!found[0].recurring);
        assert!(!found[0].all_day);
    }

    #[test]
    fn a_tzid_datetime_is_resolved_through_its_own_zone() {
        let found = blocks(
            &event("UID:a\r\nDTSTART;TZID=Asia/Tokyo:20260723T140000\r\nDTEND;TZID=Asia/Tokyo:20260723T150000"),
            "2026-07-23T00:00:00Z",
            "2026-07-24T00:00:00Z",
        );

        assert_eq!(found[0].start, utc("2026-07-23T05:00:00Z"));
    }

    /// Form #1 carries no zone, so it means whatever the reader's clock says.
    #[test]
    fn a_floating_datetime_is_read_in_the_local_zone() {
        let found = blocks(
            &event("UID:a\r\nDTSTART:20260723T140000\r\nDTEND:20260723T150000"),
            "2026-07-23T00:00:00Z",
            "2026-07-24T00:00:00Z",
        );

        // 14:00 in the +09:00 zone the test passes in.
        assert_eq!(found[0].start, utc("2026-07-23T05:00:00Z"));
    }

    #[test]
    fn a_date_valued_event_is_marked_all_day_and_lasts_a_day() {
        let found = blocks(
            &event("UID:a\r\nDTSTART;VALUE=DATE:20260723"),
            "2026-07-22T00:00:00Z",
            "2026-07-25T00:00:00Z",
        );

        assert_eq!(found.len(), 1);
        assert!(found[0].all_day);
        // Midnight in the local zone, not in UTC.
        assert_eq!(found[0].start, utc("2026-07-22T15:00:00Z"));
        assert_eq!(found[0].end, utc("2026-07-23T15:00:00Z"));
    }

    #[test]
    fn a_missing_dtend_becomes_an_hour() {
        let found = blocks(
            &event("UID:a\r\nDTSTART:20260723T050000Z"),
            "2026-07-23T00:00:00Z",
            "2026-07-24T00:00:00Z",
        );

        assert_eq!(found[0].end, utc("2026-07-23T06:00:00Z"));
    }

    #[test]
    fn a_dtend_before_its_dtstart_falls_back_rather_than_vanishing() {
        let found = blocks(
            &event("UID:a\r\nDTSTART:20260723T050000Z\r\nDTEND:20260723T040000Z"),
            "2026-07-23T00:00:00Z",
            "2026-07-24T00:00:00Z",
        );

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].end, utc("2026-07-23T06:00:00Z"));
    }

    /// The window is half-open at both ends.
    #[test]
    fn an_event_that_ends_exactly_when_the_window_opens_is_outside_it() {
        let ends_at_open = event("UID:a\r\nDTSTART:20260722T230000Z\r\nDTEND:20260723T000000Z");
        assert!(blocks(
            &ends_at_open,
            "2026-07-23T00:00:00Z",
            "2026-07-24T00:00:00Z"
        )
        .is_empty());

        let straddles = event("UID:b\r\nDTSTART:20260722T230000Z\r\nDTEND:20260723T010000Z");
        assert_eq!(
            blocks(&straddles, "2026-07-23T00:00:00Z", "2026-07-24T00:00:00Z").len(),
            1
        );
    }

    #[test]
    fn a_weekly_rule_is_expanded_by_the_library() {
        let weekly = event(
            "UID:a\r\nSUMMARY:组会\r\nDTSTART:20260701T010000Z\r\nDTEND:20260701T020000Z\r\nRRULE:FREQ=WEEKLY;BYDAY=WE",
        );

        let found = blocks(&weekly, "2026-07-01T00:00:00Z", "2026-08-01T00:00:00Z");

        let starts: Vec<_> = found.iter().map(|e| e.start).collect();
        assert_eq!(
            starts,
            vec![
                utc("2026-07-01T01:00:00Z"),
                utc("2026-07-08T01:00:00Z"),
                utc("2026-07-15T01:00:00Z"),
                utc("2026-07-22T01:00:00Z"),
                utc("2026-07-29T01:00:00Z"),
            ]
        );
        assert!(found.iter().all(|e| e.recurring));
        assert!(found.iter().all(|e| e.uid == "a"));
        // One UID, but ids have to be distinct or the frontend cannot key on them.
        assert_eq!(
            found.iter().map(|e| e.id.clone()).collect::<Vec<_>>().len(),
            5
        );
        assert_ne!(found[0].id, found[1].id);
    }

    /// The occurrence started yesterday, so a naive "starts inside the window"
    /// query would miss it.
    #[test]
    fn an_occurrence_running_into_the_window_is_still_found() {
        let nightly = event(
            "UID:a\r\nDTSTART:20260701T220000Z\r\nDTEND:20260702T020000Z\r\nRRULE:FREQ=DAILY",
        );

        let found = blocks(&nightly, "2026-07-05T00:00:00Z", "2026-07-05T12:00:00Z");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].start, utc("2026-07-04T22:00:00Z"));
        assert_eq!(found[0].end, utc("2026-07-05T02:00:00Z"));
    }

    #[test]
    fn a_malformed_rrule_costs_the_rule_but_not_the_event() {
        let broken = event(
            "UID:a\r\nDTSTART:20260723T050000Z\r\nDTEND:20260723T060000Z\r\nRRULE:FREQ=NONSENSE",
        );

        let found = blocks(&broken, "2026-07-23T00:00:00Z", "2026-07-24T00:00:00Z");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].start, utc("2026-07-23T05:00:00Z"));
        // Still flagged, so no edit from this side can quietly drop the rule.
        assert!(found[0].recurring);
    }

    #[test]
    fn an_event_with_no_uid_renders_but_is_left_unaddressable() {
        let found = blocks(
            &event("SUMMARY:手写的\r\nDTSTART:20260723T050000Z\r\nDTEND:20260723T060000Z"),
            "2026-07-23T00:00:00Z",
            "2026-07-24T00:00:00Z",
        );

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].uid, "");
        assert_eq!(found[0].id, "anon");
    }

    #[test]
    fn rdate_alone_counts_as_recurring() {
        let with_rdate = event(
            "UID:a\r\nDTSTART:20260723T050000Z\r\nDTEND:20260723T060000Z\r\nRDATE:20260724T050000Z",
        );

        assert!(is_recurring(&with_rdate));
    }

    #[test]
    fn editing_leaves_everything_it_was_not_asked_to_change() {
        let mut original = event(
            "UID:a\r\nSUMMARY:旧标题\r\nDESCRIPTION:手写的备注\r\nLOCATION:研究室\r\nX-MY-FIELD:keep me\r\nDTSTART:20260723T050000Z\r\nDTEND:20260723T060000Z",
        );

        apply(
            &mut original,
            &EventDraft {
                summary: "新标题".to_string(),
                start: utc("2026-07-23T07:00:00Z"),
                end: utc("2026-07-23T08:00:00Z"),
            },
        );

        assert_eq!(original.get_summary(), Some("新标题"));
        assert_eq!(original.get_description(), Some("手写的备注"));
        assert_eq!(original.get_location(), Some("研究室"));
        assert_eq!(original.property_value("X-MY-FIELD"), Some("keep me"));

        let found = blocks(&original, "2026-07-23T00:00:00Z", "2026-07-24T00:00:00Z");
        assert_eq!(found[0].start, utc("2026-07-23T07:00:00Z"));
    }

    /// We only ever write form #2, whatever the event carried before.
    #[test]
    fn writing_always_produces_utc_timestamps() {
        let mut tz_flavoured = event("UID:a\r\nDTSTART;TZID=Asia/Tokyo:20260723T140000\r\nDTEND;TZID=Asia/Tokyo:20260723T150000");

        apply(
            &mut tz_flavoured,
            &EventDraft {
                summary: "x".to_string(),
                start: utc("2026-07-23T07:00:00Z"),
                end: utc("2026-07-23T08:00:00Z"),
            },
        );

        let text = tz_flavoured.to_string();
        assert!(text.contains("DTSTART:20260723T070000Z"), "{text}");
        assert!(!text.contains("TZID"), "{text}");
    }
}
