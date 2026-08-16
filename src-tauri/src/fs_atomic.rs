//! Every write that touches user data goes through this module.
//!
//! Invariant #3: a reader must never observe a half-written file. Overwrites are
//! staged in a sibling temp file and renamed into place; appends are a single
//! `write` of one newline-terminated record. A record counts as committed only
//! once its terminating newline is on disk, so a process that dies mid-append
//! leaves a tail that readers can identify and skip.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, IoResultExt, Result};

/// Replace `path`'s contents with `bytes`, atomically.
///
/// A concurrent reader sees either the old file or the new one, never a blend of
/// the two, and the target is never truncated.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).at(parent)?;
        }
    }

    let tmp = temp_path(path);

    if let Err(e) = stage(&tmp, bytes) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }

    if let Err(e) = fs::rename(&tmp, path).at(path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }

    // Without this the rename itself can be lost in a power cut even though the
    // file's bytes made it to disk.
    sync_parent_dir(path)
}

/// Write the new contents to the temp file and get them onto stable storage.
///
/// The handle is closed when this returns, before the caller renames: Windows
/// refuses to rename a file that is still open.
fn stage(tmp: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = File::create(tmp).at(tmp)?;
    file.write_all(bytes).at(tmp)?;
    sync_file(&file, tmp)
}

/// Append one record as a single JSONL line.
pub fn append_jsonl<T: Serialize>(path: &Path, record: &T) -> Result<()> {
    let mut line = serde_json::to_string(record)?;
    // `serde_json::to_string` never emits a raw newline, so one record is always
    // exactly one line. Guard the assumption in debug builds anyway.
    debug_assert!(!line.contains('\n'), "record serialized across lines");
    line.push('\n');

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).at(parent)?;
        }
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(path)
        .at(path)?;

    // If a previous run died mid-append, the file ends in a partial record.
    // Writing straight onto it would splice the two together, so break the line
    // first. The partial bytes stay where they are -- `read_jsonl` skips them --
    // because nothing on the write path may destroy data the user might want.
    if ends_mid_line(&file, path)? {
        file.write_all(b"\n").at(path)?;
    }

    file.write_all(line.as_bytes()).at(path)?;
    file.flush().at(path)?;
    sync_file(&file, path)
}

/// A line that was on disk, terminated, and still could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorruptLine {
    /// 1-based, counting terminated lines from the top of the file.
    pub line_no: usize,
    pub reason: String,
}

#[derive(Debug)]
pub struct JsonlRead<T> {
    pub records: Vec<T>,
    /// Damaged lines are reported rather than fatal: one bad line in the middle
    /// must not cost the user a whole day of records.
    pub corrupt: Vec<CorruptLine>,
    /// The file did not end in a newline, so its last record was still being
    /// written when the process died. Those bytes are ignored.
    pub truncated_tail: bool,
}

// Hand-written rather than derived: an empty read is meaningful for any `T`,
// but `#[derive(Default)]` would demand `T: Default` as well.
impl<T> Default for JsonlRead<T> {
    fn default() -> Self {
        Self {
            records: Vec::new(),
            corrupt: Vec::new(),
            truncated_tail: false,
        }
    }
}

/// Read a JSONL file, tolerating the damage a crash can leave behind.
///
/// A missing file reads as empty -- the first session of the day has simply not
/// been written yet.
pub fn read_jsonl<T: DeserializeOwned>(path: &Path) -> Result<JsonlRead<T>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(JsonlRead::default()),
        Err(source) => {
            return Err(AppError::Io {
                path: path.to_path_buf(),
                source,
            })
        }
    };

    let mut out = JsonlRead {
        truncated_tail: bytes.last().is_some_and(|b| *b != b'\n'),
        ..JsonlRead::default()
    };

    // Splitting on the terminator yields a trailing empty slice for a properly
    // terminated file and the uncommitted tail otherwise. Either way the final
    // element is never a committed record, so it is dropped.
    let mut parts = bytes.split(|b| *b == b'\n').peekable();
    let mut line_no = 0;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            break;
        }
        line_no += 1;

        // Tolerate CRLF in case the user edited the file on Windows.
        let part = part.strip_suffix(b"\r").unwrap_or(part);
        if part.iter().all(u8::is_ascii_whitespace) {
            continue;
        }

        match serde_json::from_slice(part) {
            Ok(record) => out.records.push(record),
            Err(e) => out.corrupt.push(CorruptLine {
                line_no,
                reason: e.to_string(),
            }),
        }
    }

    Ok(out)
}

/// Is the last byte of the file something other than a newline?
fn ends_mid_line(mut file: &File, path: &Path) -> Result<bool> {
    let len = file.metadata().at(path)?.len();
    if len == 0 {
        return Ok(false);
    }

    // Safe to seek even though the handle is in append mode: O_APPEND forces
    // every write back to the end regardless of the read cursor.
    file.seek(SeekFrom::End(-1)).at(path)?;
    let mut last = [0u8; 1];
    file.read_exact(&mut last).at(path)?;
    Ok(last[0] != b'\n')
}

/// A sibling of `target`, so the rename stays inside one filesystem and is
/// therefore atomic.
///
/// The name is deliberately dotted and does not end in `.ics`/`.jsonl`, so a
/// temp file abandoned by a crash is never mistaken for vault data by code that
/// scans a directory by extension.
fn temp_path(target: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);

    target.with_file_name(format!(".{name}.tmp-{}-{nanos}-{seq}", std::process::id()))
}

/// Push a file's contents all the way to stable storage.
fn sync_file(file: &File, path: &Path) -> Result<()> {
    file.sync_all().at(path)?;
    #[cfg(target_os = "macos")]
    full_fsync(file);
    Ok(())
}

/// On macOS `fsync` only hands the data to the drive's write cache; `F_FULLFSYNC`
/// is what makes the device itself flush.
///
/// Best effort: some filesystems (network mounts, a few VM setups) reject it, and
/// `sync_all` has already run by this point.
#[cfg(target_os = "macos")]
fn full_fsync(file: &File) {
    use std::os::unix::io::AsRawFd;

    // SAFETY: `fcntl` with F_FULLFSYNC takes no extra argument and only reads the
    // descriptor, which stays alive for the duration of the call.
    unsafe {
        libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC);
    }
}

/// Persist the *directory entry* created by a rename.
#[cfg(unix)]
fn sync_parent_dir(path: &Path) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    let dir = File::open(parent).at(parent)?;
    dir.sync_all().at(parent)
}

/// Windows exposes no directory handle through `std::fs`, so durability of the
/// rename is left to the filesystem.
#[cfg(not(unix))]
fn sync_parent_dir(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde::{Deserialize, Serialize};
    use tempfile::TempDir;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Rec {
        id: u32,
        label: String,
    }

    fn rec(id: u32, label: &str) -> Rec {
        Rec {
            id,
            label: label.to_string(),
        }
    }

    /// Temp files are dotted siblings; this counts the ones left behind.
    fn stray_temp_files(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .expect("readable dir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp-"))
            .collect()
    }

    #[test]
    fn atomic_write_stores_exact_bytes_and_cleans_up() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");

        atomic_write(&path, b"schema_version = 1\n").expect("write");

        assert_eq!(fs::read(&path).expect("read"), b"schema_version = 1\n");
        assert!(stray_temp_files(dir.path()).is_empty());
    }

    #[test]
    fn atomic_write_replaces_previous_contents_entirely() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");

        atomic_write(&path, b"a-much-longer-first-version").expect("write");
        atomic_write(&path, b"short").expect("overwrite");

        // A truncate-then-write would have left the tail of the old version here.
        assert_eq!(fs::read(&path).expect("read"), b"short");
    }

    #[test]
    fn a_temp_file_abandoned_before_rename_leaves_the_target_intact() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        atomic_write(&path, b"good = 1").expect("write");

        // What a crash between "stage" and "rename" looks like on disk.
        fs::write(dir.path().join(".config.toml.tmp-9999-0-0"), b"half-writ").expect("stray");

        assert_eq!(fs::read(&path).expect("read"), b"good = 1");

        // And the next write still succeeds, ignoring the debris.
        atomic_write(&path, b"good = 2").expect("write again");
        assert_eq!(fs::read(&path).expect("read"), b"good = 2");
    }

    #[test]
    fn append_jsonl_round_trips_records_in_order() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("2026-07-23.jsonl");

        append_jsonl(&path, &rec(1, "写论文")).expect("append");
        append_jsonl(&path, &rec(2, "休息")).expect("append");

        let read = read_jsonl::<Rec>(&path).expect("read");
        assert_eq!(read.records, vec![rec(1, "写论文"), rec(2, "休息")]);
        assert!(read.corrupt.is_empty());
        assert!(!read.truncated_tail);
    }

    /// The headline case: a process killed mid-append must cost at most the
    /// record it was writing.
    #[test]
    fn a_record_cut_off_mid_write_is_skipped_not_fatal() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("2026-07-23.jsonl");

        append_jsonl(&path, &rec(1, "写论文")).expect("append");
        append_jsonl(&path, &rec(2, "休息")).expect("append");

        // Third record, cut off before its terminating newline.
        let mut f = OpenOptions::new().append(true).open(&path).expect("open");
        f.write_all(br#"{"id":3,"lab"#).expect("partial write");
        drop(f);

        let read = read_jsonl::<Rec>(&path).expect("read");
        assert_eq!(read.records, vec![rec(1, "写论文"), rec(2, "休息")]);
        assert!(read.truncated_tail);
        assert!(read.corrupt.is_empty());
    }

    /// A record that happens to be valid JSON at the point of truncation is
    /// still incomplete: the newline, not parseability, is the commit marker.
    #[test]
    fn a_truncated_record_that_parses_is_still_discarded() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("2026-07-23.jsonl");

        append_jsonl(&path, &rec(1, "写论文")).expect("append");
        let mut f = OpenOptions::new().append(true).open(&path).expect("open");
        f.write_all(r#"{"id":2,"label":"休息"}"#.as_bytes())
            .expect("write");
        drop(f);

        let read = read_jsonl::<Rec>(&path).expect("read");
        assert_eq!(read.records, vec![rec(1, "写论文")]);
        assert!(read.truncated_tail);
    }

    #[test]
    fn appending_after_a_truncated_tail_does_not_splice_records() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("2026-07-23.jsonl");

        append_jsonl(&path, &rec(1, "写论文")).expect("append");
        let mut f = OpenOptions::new().append(true).open(&path).expect("open");
        f.write_all(br#"{"id":3,"lab"#).expect("partial write");
        drop(f);

        append_jsonl(&path, &rec(4, "继续")).expect("append after crash");

        let read = read_jsonl::<Rec>(&path).expect("read");
        assert_eq!(read.records, vec![rec(1, "写论文"), rec(4, "继续")]);
        assert!(!read.truncated_tail);

        // The orphaned fragment is now a terminated, unparseable line: reported,
        // not fatal.
        assert_eq!(read.corrupt.len(), 1);
        assert_eq!(read.corrupt[0].line_no, 2);

        // And it is still on disk. The write path never destroys bytes.
        let raw = fs::read_to_string(&path).expect("read raw");
        assert!(
            raw.contains(r#"{"id":3,"lab"#),
            "fragment was destroyed: {raw}"
        );
    }

    #[test]
    fn a_corrupt_line_in_the_middle_costs_only_that_line() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("2026-07-23.jsonl");

        fs::write(
            &path,
            "{\"id\":1,\"label\":\"a\"}\n!!! not json !!!\n{\"id\":3,\"label\":\"c\"}\n",
        )
        .expect("write");

        let read = read_jsonl::<Rec>(&path).expect("read");
        assert_eq!(read.records, vec![rec(1, "a"), rec(3, "c")]);
        assert_eq!(read.corrupt.len(), 1);
        assert_eq!(read.corrupt[0].line_no, 2);
        assert!(!read.truncated_tail);
    }

    #[test]
    fn blank_lines_are_ignored_rather_than_reported() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("2026-07-23.jsonl");

        fs::write(&path, "{\"id\":1,\"label\":\"a\"}\n\n   \n").expect("write");

        let read = read_jsonl::<Rec>(&path).expect("read");
        assert_eq!(read.records, vec![rec(1, "a")]);
        assert!(read.corrupt.is_empty());
    }

    #[test]
    fn a_file_that_does_not_exist_reads_as_empty() {
        let dir = TempDir::new().expect("tempdir");

        let read = read_jsonl::<Rec>(&dir.path().join("nope.jsonl")).expect("read");
        assert!(read.records.is_empty());
        assert!(read.corrupt.is_empty());
        assert!(!read.truncated_tail);
    }
}
