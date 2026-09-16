//! The one piece of state that cannot live inside the vault: where the vault is.
//!
//! Stored in the OS application config directory, not in the vault itself, for
//! the obvious reason.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};
use crate::fs_atomic::atomic_write;
use crate::timer::{DEFAULT_BREAK, DEFAULT_WORK};

/// Free on all three platforms as far as we know, and mnemonic. If it collides
/// with something the user already runs, registration fails and the app says so
/// rather than fighting over the key.
pub const DEFAULT_SHORTCUT: &str = "CommandOrControl+Shift+P";

pub fn settings_path(config_dir: &Path) -> PathBuf {
    config_dir.join("settings.toml")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Absolute path to the user's vault; `None` until they pick one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vault_path: Option<PathBuf>,
    /// Accelerator for the global start/pause key. `None` means the user turned
    /// the shortcut off, which is different from it failing to register.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shortcut_toggle: Option<String>,
    pub autostart: bool,
    pub work_secs: u64,
    pub break_secs: u64,
}

// Hand-written because the interesting defaults are not the zero values: a
// derived `Default` would hand out a zero-second pomodoro.
impl Default for Settings {
    fn default() -> Self {
        Self {
            vault_path: None,
            shortcut_toggle: Some(DEFAULT_SHORTCUT.to_string()),
            autostart: false,
            work_secs: DEFAULT_WORK.as_secs(),
            break_secs: DEFAULT_BREAK.as_secs(),
        }
    }
}

impl Settings {
    /// A missing file means "first launch", not a failure.
    ///
    /// A *malformed* file is an error on purpose. Quietly falling back to the
    /// default would present the app as unconfigured, and the next directory
    /// pick would silently replace a vault the user still has.
    pub fn load(config_dir: &Path) -> Result<Self> {
        let path = settings_path(config_dir);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => return Err(AppError::Io { path, source }),
        };

        toml::from_str(&text).map_err(|source| AppError::TomlDecode {
            path,
            source: Box::new(source),
        })
    }

    pub fn save(&self, config_dir: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self)?;
        atomic_write(&settings_path(config_dir), text.as_bytes())
    }

    /// Load, change one thing, save.
    ///
    /// The only safe way to touch a single field: building a `Settings` literal
    /// and saving it resets every field the caller did not mention.
    pub fn update(config_dir: &Path, edit: impl FnOnce(&mut Self)) -> Result<Self> {
        let mut settings = Self::load(config_dir)?;
        edit(&mut settings);
        settings.save(config_dir)?;
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    #[test]
    fn absent_settings_read_as_unconfigured() {
        let dir = TempDir::new().expect("tempdir");

        let settings = Settings::load(dir.path()).expect("load");
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn settings_round_trip_through_disk() {
        let dir = TempDir::new().expect("tempdir");
        let settings = Settings {
            vault_path: Some(PathBuf::from("/Users/someone/MyVault")),
            shortcut_toggle: Some("Alt+F1".to_string()),
            autostart: true,
            work_secs: 3000,
            break_secs: 600,
        };

        settings.save(dir.path()).expect("save");

        assert_eq!(Settings::load(dir.path()).expect("load"), settings);
    }

    #[test]
    fn settings_are_created_even_if_the_config_dir_does_not_exist_yet() {
        let dir = TempDir::new().expect("tempdir");
        let config_dir = dir.path().join("com.hinoki.wabicalendar");

        Settings {
            vault_path: Some(PathBuf::from("/tmp/vault")),
            ..Settings::default()
        }
        .save(&config_dir)
        .expect("save");

        assert!(settings_path(&config_dir).exists());
    }

    #[test]
    fn a_file_written_by_an_older_build_keeps_the_defaults_for_new_keys() {
        let dir = TempDir::new().expect("tempdir");
        std::fs::write(settings_path(dir.path()), "vault_path = \"/tmp/vault\"\n").expect("write");

        let settings = Settings::load(dir.path()).expect("load");

        assert_eq!(settings.vault_path, Some(PathBuf::from("/tmp/vault")));
        assert_eq!(settings.work_secs, DEFAULT_WORK.as_secs());
        assert_eq!(settings.shortcut_toggle.as_deref(), Some(DEFAULT_SHORTCUT));
    }

    #[test]
    fn updating_one_field_leaves_the_others_alone() {
        let dir = TempDir::new().expect("tempdir");
        Settings {
            autostart: true,
            work_secs: 3000,
            ..Settings::default()
        }
        .save(dir.path())
        .expect("save");

        Settings::update(dir.path(), |s| {
            s.vault_path = Some(PathBuf::from("/tmp/vault"))
        })
        .expect("update");

        let settings = Settings::load(dir.path()).expect("load");
        assert_eq!(settings.vault_path, Some(PathBuf::from("/tmp/vault")));
        assert!(settings.autostart, "autostart was clobbered");
        assert_eq!(settings.work_secs, 3000, "work_secs was clobbered");
    }

    #[test]
    fn a_malformed_settings_file_is_reported_not_swallowed() {
        let dir = TempDir::new().expect("tempdir");
        std::fs::write(settings_path(dir.path()), "vault_path = [unclosed").expect("write");

        let err = Settings::load(dir.path()).expect_err("should fail");
        assert!(matches!(err, AppError::TomlDecode { .. }));
    }
}
