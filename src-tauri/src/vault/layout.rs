//! Where things live inside a vault. Nothing here touches the filesystem.

use std::path::PathBuf;

use super::Vault;

pub const CALENDAR_DIR: &str = "calendar";
pub const SESSIONS_DIR: &str = "sessions";
pub const INDEX_DIR: &str = ".index";
pub const CONFIG_FILE: &str = "config.toml";
/// Lives inside `.index/`, so it is covered by "deleting that directory loses
/// nothing" and never appears beside the user's own files.
pub const LOCK_FILE: &str = "write.lock";

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

    /// The file every writer takes an exclusive lock on before touching this
    /// vault.
    ///
    /// Under `.index/` because it is exactly as disposable as the rest of it:
    /// the file carries no contents, so losing it costs nothing, and it is
    /// recreated on demand. It belongs to the vault rather than to the machine
    /// so that the lock covers whoever is writing to *these files*, not whoever
    /// happens to share a config directory.
    pub fn lock_path(&self) -> PathBuf {
        self.index_dir().join(LOCK_FILE)
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
