//! One error type for the whole backend.

use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("vault directory is missing or unreadable: {}", .0.display())]
    VaultMissing(PathBuf),

    #[error("not a directory: {}", .0.display())]
    NotADirectory(PathBuf),

    /// The decoder's own error is boxed because it is 96 bytes by itself --
    /// four fifths of everything `AppError` is -- and this enum is returned by
    /// value from nearly every function in the backend. `PathBuf` is eight
    /// bytes wider on Windows than on unix, which is what put the unboxed
    /// version over clippy's line there and nowhere else.
    #[error("{} is not valid TOML: {source}", path.display())]
    TomlDecode {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },

    #[error("could not encode TOML: {0}")]
    TomlEncode(#[from] toml::ser::Error),

    #[error("could not encode JSON: {0}")]
    JsonEncode(#[from] serde_json::Error),

    #[error("{} is not valid iCalendar: {reason}", path.display())]
    IcsDecode { path: PathBuf, reason: String },

    /// No vault has been chosen yet, or the one we had went away. The frontend
    /// already renders that as a prompt rather than a failure.
    #[error("no vault is open")]
    NoVault,

    /// The CLI equivalent of [`AppError::NoVault`]. Separate because it is the
    /// one place the answer is a sentence the user can act on rather than a
    /// state the UI draws.
    #[error("no vault configured — open WabiCalendar and choose one, or pass --vault <PATH>")]
    VaultUnset,

    /// The platform gave us nowhere to look for `settings.toml`.
    #[error("could not determine this platform's configuration directory")]
    NoConfigDir,

    #[error("no event with uid {0:?}")]
    EventNotFound(String),

    #[error("an event has to end after it starts")]
    BackwardsEvent,

    /// v1 renders occurrences of a repeating event but will not edit one: doing
    /// that properly means writing `RECURRENCE-ID` overrides.
    #[error("a repeating event can only be changed by editing its RRULE in the .ics file")]
    RecurringNotEditable,

    /// The local socket between `wabi` and the running app would not
    /// cooperate. `endpoint` is a socket path on unix and a pipe name on
    /// Windows.
    #[error("{endpoint}: {source}")]
    Ipc {
        endpoint: String,
        #[source]
        source: std::io::Error,
    },

    /// Something answered on that socket but did not speak the protocol —
    /// realistically a `wabi` and an app from different versions.
    #[error("unexpected answer from the WabiCalendar app: {0}")]
    IpcProtocol(String),

    /// The app understood the request and said no.
    ///
    /// Never a reason to fall back to doing the thing ourselves: something *is*
    /// listening, so a second timer beside it would be exactly the collision the
    /// socket exists to prevent.
    #[error("WabiCalendar declined: {0}")]
    IpcRefused(String),

    /// Another process is holding the vault's write lock.
    ///
    /// Always transient — the holder releases it as soon as its write finishes —
    /// so this is worth retrying, unlike every other variant here.
    #[error("another WabiCalendar process is writing to {}; try again", .0.display())]
    VaultBusy(PathBuf),

    #[cfg(feature = "gui")]
    #[error("{0}")]
    Tauri(#[from] tauri::Error),

    /// The update check or the download it leads to. Boxed for the reason
    /// `TomlDecode` is: the plugin's own error carries a `reqwest::Error`
    /// inside it, and this enum is returned by value nearly everywhere.
    #[cfg(feature = "gui")]
    #[error("{0}")]
    Update(Box<tauri_plugin_updater::Error>),

    #[error("{0:?} is not a key combination we can register")]
    InvalidShortcut(String),

    /// An OS integration refused. Never fatal: the caller records it and the app
    /// carries on without that one capability.
    #[error("{feature} is unavailable: {reason}")]
    Integration {
        feature: &'static str,
        reason: String,
    },
}

/// Written out rather than derived with `#[from]` so that `?` still works on
/// the plugin's own error type despite the box.
#[cfg(feature = "gui")]
impl From<tauri_plugin_updater::Error> for AppError {
    fn from(source: tauri_plugin_updater::Error) -> Self {
        Self::Update(Box::new(source))
    }
}

/// Tauri commands hand their error back to the frontend as a string.
impl serde::Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

/// `std::io::Error` does not carry the path it failed on, which makes bare I/O
/// failures nearly impossible to act on. This puts it back.
pub trait IoResultExt<T> {
    fn at(self, path: impl AsRef<Path>) -> Result<T>;
}

impl<T> IoResultExt<T> for std::io::Result<T> {
    fn at(self, path: impl AsRef<Path>) -> Result<T> {
        self.map_err(|source| AppError::Io {
            path: path.as_ref().to_path_buf(),
            source,
        })
    }
}
