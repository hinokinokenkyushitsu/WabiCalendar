//! The vault holds the user's data. The files in it are the source of truth;
//! anything under `.index/` is a cache that has to be rebuildable from them.

mod layout;

pub use layout::{CALENDAR_DIR, CONFIG_FILE, INDEX_DIR, SESSIONS_DIR};

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, IoResultExt, Result};
use crate::fs_atomic::atomic_write;

/// Bumped when the on-disk layout changes in a way that needs migrating.
pub const SCHEMA_VERSION: u32 = 1;

const DEFAULT_CONFIG: &str = "\
# CalenPomo vault configuration.
# Everything in this vault is plain text on purpose — edit it by hand if you like.

schema_version = 1
";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultConfig {
    pub schema_version: u32,
}

/// What `ensure_skeleton` had to put back, as paths relative to the vault root.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct SkeletonReport {
    pub created: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vault {
    root: PathBuf,
}

impl Vault {
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Adopt `root` as the vault, rebuilding any missing part of the skeleton.
    ///
    /// The root directory itself must already exist. Creating it here would mean
    /// that an unmounted external drive quietly yields a brand new empty vault at
    /// the mount point, which to the user is indistinguishable from data loss.
    pub fn open(root: impl Into<PathBuf>) -> Result<(Self, SkeletonReport)> {
        let root = root.into();

        let meta = match fs::metadata(&root) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(AppError::VaultMissing(root))
            }
            Err(source) => return Err(AppError::Io { path: root, source }),
        };
        if !meta.is_dir() {
            return Err(AppError::NotADirectory(root));
        }

        let vault = Self { root };
        let report = vault.ensure_skeleton()?;
        // Fail loudly rather than overwrite a config we cannot understand.
        vault.read_config()?;

        Ok((vault, report))
    }

    /// Recreate whatever is missing, and only what is missing.
    pub fn ensure_skeleton(&self) -> Result<SkeletonReport> {
        let mut report = SkeletonReport::default();

        for name in [CALENDAR_DIR, SESSIONS_DIR, INDEX_DIR] {
            let dir = self.root.join(name);
            if !dir.exists() {
                fs::create_dir_all(&dir).at(&dir)?;
                report.created.push(name.to_string());
            }
        }

        let config = self.config_path();
        if !config.exists() {
            atomic_write(&config, DEFAULT_CONFIG.as_bytes())?;
            report.created.push(CONFIG_FILE.to_string());
        }

        Ok(report)
    }

    pub fn read_config(&self) -> Result<VaultConfig> {
        let path = self.config_path();
        let text = fs::read_to_string(&path).at(&path)?;
        toml::from_str(&text).map_err(|source| AppError::TomlDecode { path, source })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    #[test]
    fn opening_a_fresh_directory_builds_the_whole_skeleton() {
        let dir = TempDir::new().expect("tempdir");

        let (vault, report) = Vault::open(dir.path()).expect("open");

        assert!(vault.calendar_dir().is_dir());
        assert!(vault.sessions_dir().is_dir());
        assert!(vault.index_dir().is_dir());
        assert!(vault.config_path().is_file());
        assert_eq!(
            report.created,
            vec![CALENDAR_DIR, SESSIONS_DIR, INDEX_DIR, CONFIG_FILE]
        );
        assert_eq!(
            vault.read_config().expect("config").schema_version,
            SCHEMA_VERSION
        );
    }

    #[test]
    fn reopening_an_intact_vault_changes_nothing() {
        let dir = TempDir::new().expect("tempdir");
        let (vault, _) = Vault::open(dir.path()).expect("first open");
        fs::write(
            vault.config_path(),
            "schema_version = 1\nhand_edited = true\n",
        )
        .expect("edit config");

        let (_, report) = Vault::open(dir.path()).expect("second open");

        assert!(report.created.is_empty());
        let config = fs::read_to_string(vault.config_path()).expect("read");
        assert!(
            config.contains("hand_edited = true"),
            "config was clobbered"
        );
    }

    /// Invariant #1: `.index/` is disposable.
    #[test]
    fn deleting_the_index_directory_is_repaired_without_touching_user_data() {
        let dir = TempDir::new().expect("tempdir");
        let (vault, _) = Vault::open(dir.path()).expect("open");

        let session = vault.sessions_file(2026, 7, 23);
        fs::write(&session, "{\"id\":\"a\"}\n").expect("write session");
        let event = vault.calendar_file(2026, 7);
        fs::write(&event, "BEGIN:VCALENDAR\n").expect("write calendar");

        fs::remove_dir_all(vault.index_dir()).expect("nuke index");
        let (vault, report) = Vault::open(dir.path()).expect("reopen");

        assert_eq!(report.created, vec![INDEX_DIR]);
        assert!(vault.index_dir().is_dir());
        assert_eq!(
            fs::read_to_string(&session).expect("read"),
            "{\"id\":\"a\"}\n"
        );
        assert_eq!(
            fs::read_to_string(&event).expect("read"),
            "BEGIN:VCALENDAR\n"
        );
    }

    #[test]
    fn a_vault_path_that_no_longer_exists_is_reported_not_recreated() {
        let dir = TempDir::new().expect("tempdir");
        let gone = dir.path().join("unplugged-drive/MyVault");

        let err = Vault::open(&gone).expect_err("should fail");

        assert!(matches!(err, AppError::VaultMissing(_)));
        assert!(!gone.exists(), "the missing vault was silently created");
    }

    #[test]
    fn a_file_where_the_vault_should_be_is_rejected() {
        let dir = TempDir::new().expect("tempdir");
        let not_a_dir = dir.path().join("MyVault");
        fs::write(&not_a_dir, "i am a file").expect("write");

        let err = Vault::open(&not_a_dir).expect_err("should fail");

        assert!(matches!(err, AppError::NotADirectory(_)));
    }

    #[test]
    fn an_unparseable_config_is_reported_rather_than_overwritten() {
        let dir = TempDir::new().expect("tempdir");
        let (vault, _) = Vault::open(dir.path()).expect("open");
        fs::write(vault.config_path(), "schema_version = [oops").expect("corrupt config");

        let err = Vault::open(dir.path()).expect_err("should fail");

        assert!(matches!(err, AppError::TomlDecode { .. }));
        assert_eq!(
            fs::read_to_string(vault.config_path()).expect("read"),
            "schema_version = [oops"
        );
    }
}
