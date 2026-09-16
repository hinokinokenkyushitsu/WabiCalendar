//! The `calendar/YYYY-MM.ics` shards, and nothing above the level of a file.
//!
//! Every path here comes out of `Vault::calendar_file` or `Vault::calendar_dir`
//! (invariant #1) and every write goes through `fs_atomic` (invariant #3).

use std::fs;
use std::str::FromStr;

use chrono::{DateTime, Datelike, TimeZone, Utc};
use icalendar::{Calendar, Property};

use crate::error::{AppError, IoResultExt, Result};
use crate::fs_atomic::atomic_write;
use crate::vault::Vault;

/// Which month file an event lives in.
///
/// Ordering is derived, which on `(year, month)` in this order is chronological.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct YearMonth {
    pub year: i32,
    pub month: u32,
}

impl YearMonth {
    /// The shard an instant belongs to, read in `tz`.
    ///
    /// Deliberately the *local* month rather than the UTC one: a 00:30 event on
    /// the first of the month belongs in the file the user would go looking in.
    pub fn of<Z: TimeZone>(dt: DateTime<Utc>, tz: &Z) -> Self {
        let local = dt.with_timezone(tz);
        Self {
            year: local.year(),
            month: local.month(),
        }
    }

    /// `2026-07.ics` -> July 2026; anything else -> `None`.
    ///
    /// Non-matching names are simply not shards, which is what keeps the temp
    /// files `fs_atomic` leaves behind after a crash (`.2026-07.ics.tmp-…`) out
    /// of the calendar: they neither start with a digit nor end in `.ics`.
    fn parse(file_name: &str) -> Option<Self> {
        let stem = file_name.strip_suffix(".ics")?;
        let (year, month) = stem.split_once('-')?;
        if year.len() != 4 || month.len() != 2 {
            return None;
        }
        let year = year.parse().ok()?;
        let month = month.parse().ok()?;
        if !(1..=12).contains(&month) {
            return None;
        }
        Some(Self { year, month })
    }
}

/// Every shard present in the vault, oldest first.
///
/// A missing `calendar/` directory is an error rather than an empty calendar:
/// `Vault::open` guarantees it exists, so its absence means the vault went away
/// underneath us, and showing an empty week would look exactly like data loss.
pub fn shards(vault: &Vault) -> Result<Vec<YearMonth>> {
    let dir = vault.calendar_dir();
    let mut found = Vec::new();

    for entry in fs::read_dir(&dir).at(&dir)? {
        let entry = entry.at(&dir)?;
        if let Some(ym) = entry.file_name().to_str().and_then(YearMonth::parse) {
            found.push(ym);
        }
    }

    found.sort_unstable();
    Ok(found)
}

/// RFC 5545 asks the product that wrote a file to identify itself, and a file we
/// hand to the user should not claim to have come from the library we happen to
/// build it with.
const PRODID: &str = "-//Hinoki//WabiCalendar//EN";

/// A new, empty shard of ours.
///
/// Only for a shard that does not exist yet: an existing file keeps whatever
/// PRODID it arrived with, because rewriting one event is no reason to claim
/// authorship of a calendar someone else made.
pub fn blank() -> Calendar {
    let mut calendar = Calendar::new();
    // `append_property` appends rather than replaces, and two PRODIDs is not a
    // valid calendar.
    calendar.properties.retain(|p| p.key() != "PRODID");
    calendar.append_property(Property::new("PRODID", PRODID));
    calendar
}

/// Read one shard. A shard that is not there yet reads as `None`.
pub fn read(vault: &Vault, ym: YearMonth) -> Result<Option<Calendar>> {
    let path = vault.calendar_file(ym.year, ym.month);

    // Read-only, so it is one of the documented exceptions to invariant #3.
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(AppError::Io { path, source }),
    };

    Calendar::from_str(&text)
        .map(Some)
        .map_err(|reason| AppError::IcsDecode { path, reason })
}

/// Replace one shard with `calendar`, atomically.
pub fn write(vault: &Vault, ym: YearMonth, calendar: &Calendar) -> Result<()> {
    atomic_write(
        &vault.calendar_file(ym.year, ym.month),
        calendar.to_string().as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::FixedOffset;

    fn jst() -> FixedOffset {
        FixedOffset::east_opt(9 * 3600).expect("valid offset")
    }

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("valid timestamp")
            .with_timezone(&Utc)
    }

    #[test]
    fn a_shard_name_round_trips() {
        assert_eq!(
            YearMonth::parse("2026-07.ics"),
            Some(YearMonth {
                year: 2026,
                month: 7
            })
        );
    }

    #[test]
    fn names_that_are_not_shards_are_not_mistaken_for_one() {
        for name in [
            "2026-7.ics",                     // unpadded
            "2026-13.ics",                    // no such month
            "2026-00.ics",                    // nor this one
            "2026-07.txt",                    // not a calendar
            "2026-07",                        // no extension
            "notes.ics",                      // not dated
            ".2026-07.ics.tmp-123-456-0",     // fs_atomic debris
            ".2026-07.ics.tmp-123-456-0.ics", // debris that ends in .ics anyway
        ] {
            assert_eq!(YearMonth::parse(name), None, "{name} parsed as a shard");
        }
    }

    /// The shard is the month the *user* sees, not the month in UTC.
    #[test]
    fn an_early_morning_event_lands_in_the_local_month() {
        // 00:30 on 1 July in Tokyo is still 15:30 on 30 June in UTC.
        let dt = utc("2026-06-30T15:30:00Z");

        assert_eq!(
            YearMonth::of(dt, &jst()),
            YearMonth {
                year: 2026,
                month: 7
            }
        );
        assert_eq!(
            YearMonth::of(dt, &Utc),
            YearMonth {
                year: 2026,
                month: 6
            }
        );
    }

    #[test]
    fn shards_come_back_in_chronological_order() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let (vault, _) = Vault::open(dir.path()).expect("open");

        for name in ["2026-01.ics", "2025-12.ics", "2026-02.ics", "readme.txt"] {
            fs::write(
                vault.calendar_dir().join(name),
                "BEGIN:VCALENDAR\nEND:VCALENDAR\n",
            )
            .expect("write");
        }

        assert_eq!(
            shards(&vault).expect("shards"),
            vec![
                YearMonth {
                    year: 2025,
                    month: 12
                },
                YearMonth {
                    year: 2026,
                    month: 1
                },
                YearMonth {
                    year: 2026,
                    month: 2
                },
            ]
        );
    }

    #[test]
    fn a_shard_that_does_not_exist_reads_as_nothing() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let (vault, _) = Vault::open(dir.path()).expect("open");

        let read = read(
            &vault,
            YearMonth {
                year: 2026,
                month: 7,
            },
        )
        .expect("read");

        assert!(read.is_none());
    }
}
