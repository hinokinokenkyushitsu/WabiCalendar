//! Where things live inside a vault. Nothing here touches the filesystem.

use std::path::PathBuf;

use super::Vault;

pub const CALENDAR_DIR: &str = "calendar";
pub const SESSIONS_DIR: &str = "sessions";
pub const INDEX_DIR: &str = ".index";
pub const CONFIG_FILE: &str = "config.toml";

impl Vault {
    pub fn calendar_dir(&self) -> PathBuf {
        self.root().join(CALENDAR_DIR)
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.root().join(SESSIONS_DIR)
    }

    /// Derived data only. Deleting this directory must never lose anything.
    pub fn index_dir(&self) -> PathBuf {
        self.root().join(INDEX_DIR)
    }

    pub fn config_path(&self) -> PathBuf {
        self.root().join(CONFIG_FILE)
    }

    /// `calendar/YYYY-MM.ics` — one iCalendar file per month.
    pub fn calendar_file(&self, year: i32, month: u32) -> PathBuf {
        self.calendar_dir()
            .join(format!("{year:04}-{month:02}.ics"))
    }

    /// `sessions/YYYY-MM-DD.jsonl` — one append-only file per day.
    pub fn sessions_file(&self, year: i32, month: u32, day: u32) -> PathBuf {
        self.sessions_dir()
            .join(format!("{year:04}-{month:02}-{day:02}.jsonl"))
    }
}
