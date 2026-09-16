//! The calendar half of the vault: `calendar/YYYY-MM.ics`, one file per month.
//!
//! An event lives in the shard its start falls in, read in the local zone. That
//! is a storage detail: reads span whatever shards a window touches, and a drag
//! that carries an event across a month boundary moves it between files.

mod ics;
mod store;

pub use ics::{CalEvent, EventDraft};

use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local, TimeZone, Utc};
use icalendar::{Calendar, CalendarComponent, Component, Event};

use crate::error::{AppError, Result};
use crate::vault::Vault;
use store::YearMonth;

/// Reads and writes the `.ics` shards of one vault.
///
/// Generic over the zone so that sharding is testable without depending on the
/// machine the tests run on; everything in the app proper uses [`Calendars::local`].
pub struct Calendars<'a, Z: TimeZone> {
    vault: &'a Vault,
    /// Decides which month file an event belongs in, and interprets the
    /// timestamps in a hand-written file that carry no zone of their own.
    tz: Z,
}

impl<'a> Calendars<'a, Local> {
    pub fn local(vault: &'a Vault) -> Self {
        Self { vault, tz: Local }
    }
}

/// A VEVENT we found, and the shard it was sitting in.
struct Located {
    shard: YearMonth,
    calendar: Calendar,
    index: usize,
    event: Event,
}

/// How good a claim one copy of a UID has to being *the* copy: its revision
/// first, then whether it is sitting in the shard its own start belongs to.
type Rank = ((u32, Option<DateTime<Utc>>), bool);

impl<'a, Z: TimeZone> Calendars<'a, Z> {
    pub fn new(vault: &'a Vault, tz: Z) -> Self {
        Self { vault, tz }
    }

    /// Every block that falls inside `[from, to)`, earliest first.
    pub fn range(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<CalEvent>> {
        // Nothing that starts after the window can reach back into it. Shards
        // *before* it are still read: a repeating event's rule lives with its
        // DTSTART, which may be years earlier.
        let last = YearMonth::of(to, &self.tz);

        let mut loaded = Vec::new();
        for shard in store::shards(self.vault)? {
            if shard > last {
                continue;
            }
            if let Some(calendar) = store::read(self.vault, shard)? {
                loaded.push((shard, calendar));
            }
        }

        let mut out = Vec::new();
        for (key, event) in self.canonical(&loaded) {
            ics::occurrences(event, from, to, &self.tz, &key, &mut out);
        }

        out.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.id.cmp(&b.id)));
        Ok(out)
    }

    pub fn create(&self, draft: &EventDraft) -> Result<CalEvent> {
        check(draft)?;

        let uid = new_uid();
        let shard = YearMonth::of(draft.start, &self.tz);
        let mut calendar = store::read(self.vault, shard)?.unwrap_or_else(store::blank);
        calendar.push(ics::build(&uid, draft));
        store::write(self.vault, shard, &calendar)?;

        Ok(CalEvent {
            id: uid.clone(),
            uid,
            summary: draft.summary.clone(),
            start: draft.start,
            end: draft.end,
            // True by construction: we just wrote a one-off, timed VEVENT.
            recurring: false,
            all_day: false,
        })
    }

    pub fn update(&self, uid: &str, draft: &EventDraft) -> Result<CalEvent> {
        check(draft)?;

        let Some(mut found) = self.locate(uid)? else {
            return Err(AppError::EventNotFound(uid.to_string()));
        };
        if ics::is_recurring(&found.event) {
            return Err(AppError::RecurringNotEditable);
        }

        ics::apply(&mut found.event, draft);
        let target = YearMonth::of(draft.start, &self.tz);

        if target == found.shard {
            // Back in its old slot rather than appended, so a file the user also
            // reads by hand does not shuffle every time a block is nudged.
            found.calendar.components[found.index] = found.event.into();
            store::write(self.vault, found.shard, &found.calendar)?;
        } else {
            found.calendar.components.remove(found.index);

            let mut destination = store::read(self.vault, target)?.unwrap_or_else(store::blank);
            destination.components.retain(|c| uid_of(c) != Some(uid));
            destination.push(found.event);

            // Destination first. A crash between the two writes leaves the event
            // in both shards, which `canonical` resolves; the other order would
            // lose it outright.
            store::write(self.vault, target, &destination)?;
            store::write(self.vault, found.shard, &found.calendar)?;
        }

        Ok(CalEvent {
            id: uid.to_string(),
            uid: uid.to_string(),
            summary: draft.summary.clone(),
            start: draft.start,
            end: draft.end,
            recurring: false,
            all_day: false,
        })
    }

    /// Remove the event from every shard that holds it.
    ///
    /// Every shard, not just the canonical one: a move interrupted between its
    /// two writes leaves a copy behind, and a delete the user asked for should
    /// not leave one of them standing.
    pub fn delete(&self, uid: &str) -> Result<()> {
        if uid.is_empty() {
            return Err(AppError::EventNotFound(uid.to_string()));
        }

        let mut holding = Vec::new();
        for shard in store::shards(self.vault)? {
            let Some(calendar) = store::read(self.vault, shard)? else {
                continue;
            };
            let mut hits = calendar
                .components
                .iter()
                .filter(|c| uid_of(c) == Some(uid))
                .peekable();
            if hits.peek().is_none() {
                continue;
            }
            if hits.any(|c| c.as_event().is_some_and(ics::is_recurring)) {
                return Err(AppError::RecurringNotEditable);
            }
            holding.push((shard, calendar));
        }

        if holding.is_empty() {
            return Err(AppError::EventNotFound(uid.to_string()));
        }

        // Decided in full before anything is written, so a refusal never lands
        // half-applied.
        for (shard, mut calendar) in holding {
            calendar.components.retain(|c| uid_of(c) != Some(uid));
            store::write(self.vault, shard, &calendar)?;
        }
        Ok(())
    }

    /// One VEVENT per UID, paired with the id to render it under.
    ///
    /// A move across a month boundary writes the destination before rewriting
    /// the source, so a crash in between leaves the event in two shards. The
    /// newer revision wins (see [`ics::revision`]); ties go to the oldest shard.
    fn canonical<'c>(&self, loaded: &'c [(YearMonth, Calendar)]) -> Vec<(String, &'c Event)> {
        let mut by_uid: BTreeMap<&'c str, (Rank, String, &'c Event)> = BTreeMap::new();
        // A VEVENT with no UID cannot be deduplicated -- there is nothing to
        // compare -- so each one stands on its own.
        let mut anonymous = Vec::new();

        for (shard, calendar) in loaded {
            for (index, component) in calendar.components.iter().enumerate() {
                let Some(event) = component.as_event() else {
                    continue;
                };
                let key = format!("{:04}-{:02}#{index}", shard.year, shard.month);

                let Some(uid) = event.get_uid().filter(|uid| !uid.is_empty()) else {
                    anonymous.push((key, event));
                    continue;
                };

                let rank = self.rank(event, *shard);
                match by_uid.entry(uid) {
                    Entry::Vacant(slot) => {
                        slot.insert((rank, key, event));
                    }
                    Entry::Occupied(mut slot) if rank > slot.get().0 => {
                        slot.insert((rank, key, event));
                    }
                    Entry::Occupied(_) => {}
                }
            }
        }

        by_uid
            .into_values()
            .map(|(_, key, event)| (key, event))
            .chain(anonymous)
            .collect()
    }

    /// Find the canonical VEVENT with this UID, along with the shard it is in.
    ///
    /// Same ranking as [`Self::canonical`], so an edit always lands on the copy
    /// the week view was showing.
    fn locate(&self, uid: &str) -> Result<Option<Located>> {
        if uid.is_empty() {
            return Ok(None);
        }

        let mut best: Option<(Rank, Located)> = None;
        for shard in store::shards(self.vault)? {
            let Some(calendar) = store::read(self.vault, shard)? else {
                continue;
            };
            let Some(index) = calendar
                .components
                .iter()
                .position(|c| uid_of(c) == Some(uid))
            else {
                continue;
            };
            let Some(event) = calendar.components[index].as_event().cloned() else {
                continue;
            };

            let rank = self.rank(&event, shard);
            if best.as_ref().is_none_or(|(best_rank, _)| rank > *best_rank) {
                best = Some((
                    rank,
                    Located {
                        shard,
                        calendar,
                        index,
                        event,
                    },
                ));
            }
        }

        Ok(best.map(|(_, located)| located))
    }

    fn rank(&self, event: &Event, shard: YearMonth) -> Rank {
        let at_home = ics::start_of(event, &self.tz)
            .is_some_and(|start| YearMonth::of(start, &self.tz) == shard);
        (ics::revision(event), at_home)
    }
}

fn uid_of(component: &CalendarComponent) -> Option<&str> {
    component
        .as_event()
        .and_then(|event| event.get_uid())
        .filter(|uid| !uid.is_empty())
}

fn check(draft: &EventDraft) -> Result<()> {
    if draft.end <= draft.start {
        return Err(AppError::BackwardsEvent);
    }
    Ok(())
}

/// Unique enough for a single-user local calendar, and stable once written.
///
/// Same shape as `fs_atomic::temp_path` -- wall clock, process, counter -- and
/// for the same reason: a UUID dependency buys nothing for a string that is only
/// ever compared for equality.
fn new_uid() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);

    format!("{nanos}-{}-{seq}@wabicalendar", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use chrono::FixedOffset;
    use tempfile::TempDir;

    fn jst() -> FixedOffset {
        FixedOffset::east_opt(9 * 3600).expect("valid offset")
    }

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("valid timestamp")
            .with_timezone(&Utc)
    }

    fn draft(summary: &str, start: &str, end: &str) -> EventDraft {
        EventDraft {
            summary: summary.to_string(),
            start: utc(start),
            end: utc(end),
        }
    }

    fn vault() -> (TempDir, Vault) {
        let dir = TempDir::new().expect("tempdir");
        let (vault, _) = Vault::open(dir.path()).expect("open");
        (dir, vault)
    }

    fn shard_text(vault: &Vault, year: i32, month: u32) -> String {
        fs::read_to_string(vault.calendar_file(year, month)).unwrap_or_default()
    }

    /// How many VEVENTs sit in one shard, counted straight off the disk.
    fn vevents(vault: &Vault, year: i32, month: u32) -> usize {
        shard_text(vault, year, month)
            .matches("BEGIN:VEVENT")
            .count()
    }

    #[test]
    fn a_created_event_is_readable_again() {
        let (_dir, vault) = vault();
        let calendars = Calendars::new(&vault, jst());

        let made = calendars
            .create(&draft(
                "写论文",
                "2026-07-23T05:00:00Z",
                "2026-07-23T06:00:00Z",
            ))
            .expect("create");

        let found = calendars
            .range(utc("2026-07-23T00:00:00Z"), utc("2026-07-24T00:00:00Z"))
            .expect("range");

        assert_eq!(found, vec![made]);
    }

    /// The file is meant to be handed straight to Google or Apple Calendar, so
    /// the shape of what we emit is part of the contract, not an implementation
    /// detail.
    #[test]
    fn a_written_shard_is_a_well_formed_icalendar_file() {
        let (_dir, vault) = vault();
        let calendars = Calendars::new(&vault, jst());

        calendars
            .create(&draft(
                "写论文",
                "2026-07-23T05:00:00Z",
                "2026-07-23T06:00:00Z",
            ))
            .expect("create");

        let text = shard_text(&vault, 2026, 7);

        assert!(text.starts_with("BEGIN:VCALENDAR\r\n"), "{text}");
        assert!(text.ends_with("END:VCALENDAR\r\n"), "{text}");
        assert!(text.contains("VERSION:2.0\r\n"), "{text}");
        // Ours, not the library's.
        assert!(
            text.contains("PRODID:-//Hinoki//WabiCalendar//EN\r\n"),
            "{text}"
        );
        assert_eq!(text.matches("PRODID:").count(), 1, "{text}");
        // Form #2 throughout, which is the form every importer agrees on.
        assert!(text.contains("DTSTART:20260723T050000Z\r\n"), "{text}");
        assert!(text.contains("DTEND:20260723T060000Z\r\n"), "{text}");
        assert!(text.contains("DTSTAMP:"), "{text}");
        assert!(text.contains("SUMMARY:写论文\r\n"), "{text}");
        // CRLF everywhere, as RFC 5545 requires -- no bare newlines.
        assert_eq!(text.matches('\n').count(), text.matches("\r\n").count());
    }

    /// The shard is the month the user would go looking in.
    #[test]
    fn an_event_lands_in_the_shard_for_its_local_month() {
        let (_dir, vault) = vault();
        let calendars = Calendars::new(&vault, jst());

        // 00:30 on 1 July in Tokyo, which is still June in UTC.
        calendars
            .create(&draft("早", "2026-06-30T15:30:00Z", "2026-06-30T16:30:00Z"))
            .expect("create");

        assert_eq!(vevents(&vault, 2026, 7), 1);
        assert!(!vault.calendar_file(2026, 6).exists());
    }

    #[test]
    fn a_week_spanning_two_months_reads_both_shards() {
        let (_dir, vault) = vault();
        let calendars = Calendars::new(&vault, jst());

        calendars
            .create(&draft(
                "七月",
                "2026-07-30T05:00:00Z",
                "2026-07-30T06:00:00Z",
            ))
            .expect("create");
        calendars
            .create(&draft(
                "八月",
                "2026-08-01T05:00:00Z",
                "2026-08-01T06:00:00Z",
            ))
            .expect("create");

        // Monday 27 July through Monday 3 August, exclusive.
        let found = calendars
            .range(utc("2026-07-27T00:00:00Z"), utc("2026-08-03T00:00:00Z"))
            .expect("range");

        let summaries: Vec<_> = found.iter().map(|e| e.summary.as_str()).collect();
        assert_eq!(summaries, vec!["七月", "八月"]);
    }

    #[test]
    fn writing_one_event_leaves_a_hand_written_neighbour_alone() {
        let (_dir, vault) = vault();
        let calendars = Calendars::new(&vault, jst());

        fs::write(
            vault.calendar_file(2026, 7),
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//hand//EN\r\n\
             BEGIN:VEVENT\r\nUID:mine\r\nSUMMARY:手写的\r\nDESCRIPTION:别动我\r\n\
             X-MY-FIELD:keep\r\nDTSTART:20260723T010000Z\r\nDTEND:20260723T020000Z\r\n\
             END:VEVENT\r\nEND:VCALENDAR\r\n",
        )
        .expect("seed");

        calendars
            .create(&draft(
                "新的",
                "2026-07-23T05:00:00Z",
                "2026-07-23T06:00:00Z",
            ))
            .expect("create");

        let text = shard_text(&vault, 2026, 7);
        assert!(text.contains("UID:mine"), "{text}");
        assert!(text.contains("DESCRIPTION:别动我"), "{text}");
        assert!(text.contains("X-MY-FIELD:keep"), "{text}");
        assert_eq!(vevents(&vault, 2026, 7), 2);
    }

    #[test]
    fn moving_an_event_across_a_month_leaves_exactly_one_copy() {
        let (_dir, vault) = vault();
        let calendars = Calendars::new(&vault, jst());

        let made = calendars
            .create(&draft(
                "搬家",
                "2026-07-30T05:00:00Z",
                "2026-07-30T06:00:00Z",
            ))
            .expect("create");

        calendars
            .update(
                &made.uid,
                &draft("搬家", "2026-08-04T05:00:00Z", "2026-08-04T06:00:00Z"),
            )
            .expect("update");

        assert_eq!(vevents(&vault, 2026, 7), 0);
        assert_eq!(vevents(&vault, 2026, 8), 1);

        let found = calendars
            .range(utc("2026-07-01T00:00:00Z"), utc("2026-09-01T00:00:00Z"))
            .expect("range");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].start, utc("2026-08-04T05:00:00Z"));
    }

    /// What a crash between the two writes of a cross-month move looks like.
    ///
    /// Note that the stale copy still carries the *old* start, so it is sitting
    /// in the shard it belongs to just as much as the new one is. Only the
    /// revision tells them apart.
    #[test]
    fn an_event_left_in_two_shards_is_shown_once() {
        let (_dir, vault) = vault();
        let calendars = Calendars::new(&vault, jst());

        let stale = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
             BEGIN:VEVENT\r\nUID:moved\r\nSUMMARY:旧位置\r\nSEQUENCE:0\r\n\
             DTSTAMP:20260720T000000Z\r\n\
             DTSTART:20260730T050000Z\r\nDTEND:20260730T060000Z\r\n\
             END:VEVENT\r\nEND:VCALENDAR\r\n";
        let fresh = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
             BEGIN:VEVENT\r\nUID:moved\r\nSUMMARY:新位置\r\nSEQUENCE:1\r\n\
             DTSTAMP:20260720T000000Z\r\nLAST-MODIFIED:20260725T120000Z\r\n\
             DTSTART:20260804T050000Z\r\nDTEND:20260804T060000Z\r\n\
             END:VEVENT\r\nEND:VCALENDAR\r\n";
        fs::write(vault.calendar_file(2026, 7), stale).expect("seed");
        fs::write(vault.calendar_file(2026, 8), fresh).expect("seed");

        let found = calendars
            .range(utc("2026-07-01T00:00:00Z"), utc("2026-09-01T00:00:00Z"))
            .expect("range");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].summary, "新位置");

        // And an edit addressed to that UID lands on the copy that was shown.
        calendars
            .update(
                "moved",
                &draft("再动一次", "2026-08-05T05:00:00Z", "2026-08-05T06:00:00Z"),
            )
            .expect("update");
        assert!(shard_text(&vault, 2026, 8).contains("SUMMARY:再动一次"));
    }

    /// The revision is what makes the duplicate above resolvable, so a plain
    /// in-place edit has to move it.
    #[test]
    fn editing_bumps_the_revision() {
        let (_dir, vault) = vault();
        let calendars = Calendars::new(&vault, jst());

        let made = calendars
            .create(&draft("a", "2026-07-23T05:00:00Z", "2026-07-23T06:00:00Z"))
            .expect("create");
        calendars
            .update(
                &made.uid,
                &draft("a", "2026-07-23T07:00:00Z", "2026-07-23T08:00:00Z"),
            )
            .expect("update");

        let text = shard_text(&vault, 2026, 7);
        assert!(text.contains("SEQUENCE:1"), "{text}");
        assert!(text.contains("LAST-MODIFIED:"), "{text}");
    }

    #[test]
    fn deleting_sweeps_every_shard_that_still_holds_it() {
        let (_dir, vault) = vault();
        let calendars = Calendars::new(&vault, jst());

        for (month, day) in [(7, "30"), (8, "04")] {
            fs::write(
                vault.calendar_file(2026, month),
                format!(
                    "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
                     BEGIN:VEVENT\r\nUID:moved\r\nSUMMARY:重复\r\n\
                     DTSTART:2026{month:02}{day}T050000Z\r\nDTEND:2026{month:02}{day}T060000Z\r\n\
                     END:VEVENT\r\nEND:VCALENDAR\r\n"
                ),
            )
            .expect("seed");
        }

        calendars.delete("moved").expect("delete");

        assert_eq!(vevents(&vault, 2026, 7), 0);
        assert_eq!(vevents(&vault, 2026, 8), 0);
    }

    #[test]
    fn editing_through_a_move_keeps_the_properties_it_was_not_asked_about() {
        let (_dir, vault) = vault();
        let calendars = Calendars::new(&vault, jst());

        fs::write(
            vault.calendar_file(2026, 7),
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
             BEGIN:VEVENT\r\nUID:mine\r\nSUMMARY:旧\r\nDESCRIPTION:备注\r\n\
             DTSTART:20260730T050000Z\r\nDTEND:20260730T060000Z\r\n\
             END:VEVENT\r\nEND:VCALENDAR\r\n",
        )
        .expect("seed");

        calendars
            .update(
                "mine",
                &draft("新", "2026-08-04T05:00:00Z", "2026-08-04T06:00:00Z"),
            )
            .expect("update");

        let text = shard_text(&vault, 2026, 8);
        assert!(text.contains("DESCRIPTION:备注"), "{text}");
        assert!(text.contains("SUMMARY:新"), "{text}");
    }

    #[test]
    fn a_repeating_event_is_shown_but_refuses_to_be_edited() {
        let (_dir, vault) = vault();
        let calendars = Calendars::new(&vault, jst());

        fs::write(
            vault.calendar_file(2026, 7),
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
             BEGIN:VEVENT\r\nUID:weekly\r\nSUMMARY:组会\r\n\
             DTSTART:20260701T010000Z\r\nDTEND:20260701T020000Z\r\n\
             RRULE:FREQ=WEEKLY;BYDAY=WE\r\n\
             END:VEVENT\r\nEND:VCALENDAR\r\n",
        )
        .expect("seed");

        let found = calendars
            .range(utc("2026-07-20T00:00:00Z"), utc("2026-07-27T00:00:00Z"))
            .expect("range");
        assert_eq!(found.len(), 1);
        assert!(found[0].recurring);
        assert_eq!(found[0].start, utc("2026-07-22T01:00:00Z"));

        assert!(matches!(
            calendars.update(
                "weekly",
                &draft("组会", "2026-07-22T02:00:00Z", "2026-07-22T03:00:00Z")
            ),
            Err(AppError::RecurringNotEditable)
        ));
        assert!(matches!(
            calendars.delete("weekly"),
            Err(AppError::RecurringNotEditable)
        ));

        // And the refusal really did leave the file alone.
        assert!(shard_text(&vault, 2026, 7).contains("RRULE:FREQ=WEEKLY"));
    }

    #[test]
    fn an_event_that_is_not_there_is_reported_rather_than_created() {
        let (_dir, vault) = vault();
        let calendars = Calendars::new(&vault, jst());

        assert!(matches!(
            calendars.update(
                "nope",
                &draft("x", "2026-07-23T05:00:00Z", "2026-07-23T06:00:00Z")
            ),
            Err(AppError::EventNotFound(_))
        ));
        assert!(matches!(
            calendars.delete("nope"),
            Err(AppError::EventNotFound(_))
        ));
    }

    #[test]
    fn an_event_that_ends_before_it_starts_is_refused() {
        let (_dir, vault) = vault();
        let calendars = Calendars::new(&vault, jst());

        assert!(matches!(
            calendars.create(&draft("x", "2026-07-23T06:00:00Z", "2026-07-23T05:00:00Z")),
            Err(AppError::BackwardsEvent)
        ));
        assert!(matches!(
            calendars.create(&draft("x", "2026-07-23T05:00:00Z", "2026-07-23T05:00:00Z")),
            Err(AppError::BackwardsEvent)
        ));
    }

    /// Invariant #3: writes are staged and renamed, and clean up after
    /// themselves. A leftover temp file must also never read as a shard.
    #[test]
    fn writing_leaves_no_debris_behind() {
        let (_dir, vault) = vault();
        let calendars = Calendars::new(&vault, jst());

        let made = calendars
            .create(&draft("a", "2026-07-23T05:00:00Z", "2026-07-23T06:00:00Z"))
            .expect("create");
        calendars
            .update(
                &made.uid,
                &draft("a", "2026-08-23T05:00:00Z", "2026-08-23T06:00:00Z"),
            )
            .expect("update");

        let strays: Vec<_> = fs::read_dir(vault.calendar_dir())
            .expect("readable")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp-"))
            .collect();
        assert!(strays.is_empty(), "left behind: {strays:?}");
    }

    #[test]
    fn debris_from_a_crashed_write_is_not_read_as_a_shard() {
        let (_dir, vault) = vault();
        let calendars = Calendars::new(&vault, jst());

        calendars
            .create(&draft(
                "真的",
                "2026-07-23T05:00:00Z",
                "2026-07-23T06:00:00Z",
            ))
            .expect("create");
        fs::write(
            vault.calendar_dir().join(".2026-07.ics.tmp-9999-0-0"),
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
             BEGIN:VEVENT\r\nUID:ghost\r\nSUMMARY:半个\r\n\
             DTSTART:20260723T050000Z\r\nDTEND:20260723T060000Z\r\n\
             END:VEVENT\r\nEND:VCALENDAR\r\n",
        )
        .expect("debris");

        let found = calendars
            .range(utc("2026-07-23T00:00:00Z"), utc("2026-07-24T00:00:00Z"))
            .expect("range");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].summary, "真的");
    }

    #[test]
    fn a_shard_that_is_not_valid_icalendar_is_reported_not_ignored() {
        let (_dir, vault) = vault();
        let calendars = Calendars::new(&vault, jst());

        fs::write(vault.calendar_file(2026, 7), "this is not a calendar").expect("seed");

        assert!(matches!(
            calendars.range(utc("2026-07-23T00:00:00Z"), utc("2026-07-24T00:00:00Z")),
            Err(AppError::IcsDecode { .. })
        ));
    }
}
