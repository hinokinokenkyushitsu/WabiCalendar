//! The one socket in the app: how `wabi start` hands a pomodoro to a running
//! WabiCalendar.
//!
//! This is not a network. It is a Unix domain socket beside `settings.toml`, or
//! a named pipe on Windows — a local IPC primitive with no address that anything
//! off this machine could reach. Nothing here opens a port and nothing here
//! resolves a host.
//!
//! It exists because the vault's write lock cannot solve this half of the
//! problem. The lock keeps two processes from writing the same file at the same
//! moment, but the *timer* is not a file: while the app runs it lives in memory
//! and rewrites `timer.json` every ten seconds, so a second process that started
//! its own countdown would simply be overwritten and forgotten. The only way for
//! `wabi start` to mean anything while the app is up is to ask the app to do
//! it.
//!
//! One connection carries one request and one reply, each a single line of JSON,
//! and then closes. Both ends are built from this crate, so the two types below
//! are the whole protocol; a version mismatch shows up as a request the other
//! side cannot parse, which is answered with [`Response::Refused`] rather than
//! silence.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use interprocess::local_socket::traits::{Listener as _, Stream as _};
use interprocess::local_socket::{ListenerOptions, Name, Stream};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

/// How long either side waits on the other before giving up.
///
/// Generous for what this is — the app answers in the time it takes to start a
/// timer — but the reply is written after the app has touched its tray, which
/// on a busy machine means waiting for the main thread.
const IO_TIMEOUT: Duration = Duration::from_secs(5);

/// How many connections may fail in a row before the server gives up listening.
///
/// A single failed accept is nothing (a client that changed its mind), but a
/// socket that fails every time would otherwise spin this thread at full tilt
/// for as long as the app is open.
const MAX_CONSECUTIVE_FAILURES: usize = 16;

/// What the CLI asks the app to do.
///
/// Field names stay snake_case: both ends of this are the same crate, so there
/// is no frontend convention to meet, and matching the session file's spelling
/// makes the two easier to read side by side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd")]
pub enum Request {
    /// Start a segment now, exactly as pressing start in the app would.
    #[serde(rename = "start")]
    Start {
        /// Written into the session record.
        label: Option<String>,
        /// This one segment's length; `None` means the app's configured one.
        planned_sec: Option<u64>,
        /// The vault the caller believes it is talking about.
        ///
        /// The app writes the session, so it decides where it lands. If the two
        /// disagree the answer is [`Response::Refused`] and not a guess:
        /// `wabi --vault /elsewhere start` silently recording into the vault
        /// the app happens to have open is the one outcome nobody could debug.
        vault: Option<PathBuf>,
    },
}

/// What the app answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reply")]
pub enum Response {
    /// The app is now counting this segment.
    #[serde(rename = "started")]
    Started {
        planned_sec: u64,
        label: Option<String>,
    },
    /// The app heard the request and declined it. Never a fallback: something
    /// is listening, so running a second timer alongside it would be worse than
    /// telling the user why.
    #[serde(rename = "refused")]
    Refused { reason: String },
}

/// Where the socket file lives: beside `timer.json`, for the same reason.
///
/// Machine-local runtime state, not user data, so it has no business in the
/// vault. The protection on it is the config directory's own — every platform
/// puts that under the user's home — because the portable half of
/// `interprocess` cannot set a socket's mode on macOS.
#[cfg(unix)]
pub fn socket_path(config_dir: &Path) -> PathBuf {
    config_dir.join("cli.sock")
}

/// The socket's name, in whatever form this platform's IPC primitive takes.
///
/// Unix gets a path under the config directory. Windows has no such thing for
/// named pipes — they live in one machine-wide namespace — so the name carries a
/// fingerprint of that same directory instead. Without it two accounts logged in
/// at once would fight over a single pipe, and the second one to start the app
/// would find the name taken.
fn endpoint(config_dir: &Path) -> Result<(Name<'static>, String)> {
    #[cfg(unix)]
    {
        use interprocess::local_socket::{GenericFilePath, ToFsName};

        let path = socket_path(config_dir);
        let shown = path.display().to_string();
        let name = path
            .to_fs_name::<GenericFilePath>()
            .map_err(|source| AppError::Ipc {
                endpoint: shown.clone(),
                source,
            })?;
        Ok((name, shown))
    }
    #[cfg(windows)]
    {
        use interprocess::local_socket::{GenericNamespaced, ToNsName};

        let shown = format!("wabicalendar-{:016x}", fingerprint(config_dir));
        let name = shown
            .clone()
            .to_ns_name::<GenericNamespaced>()
            .map_err(|source| AppError::Ipc {
                endpoint: shown.clone(),
                source,
            })?;
        Ok((name, shown))
    }
}

/// FNV-1a over the config directory, to give the Windows pipe a per-user name.
///
/// Any stable hash would do; this one is four lines and needs no dependency.
/// Nothing security-sensitive rests on it — the point is only that two
/// directories should not collide.
#[cfg(windows)]
fn fingerprint(config_dir: &Path) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    config_dir
        .to_string_lossy()
        .bytes()
        .fold(OFFSET, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(PRIME)
        })
}

/// Ask a running app to do something, or find out that there is not one.
///
/// `Ok(None)` means nobody was listening, which is the ordinary case: the app is
/// simply not open. Every other failure is an error, because the difference
/// between "no app" and "an app we could not reach" decides whether the caller
/// may start a second timer of its own.
pub fn send(config_dir: &Path, request: &Request) -> Result<Option<Response>> {
    let (name, shown) = endpoint(config_dir)?;

    let stream = match Stream::connect(name) {
        Ok(stream) => stream,
        // Unix gives one of these for a socket file that is not there and for
        // one whose owner has died; Windows gives `NotFound` for a pipe nobody
        // has created. All of them mean the same thing: no app.
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(None)
        }
        Err(source) => {
            return Err(AppError::Ipc {
                endpoint: shown,
                source,
            })
        }
    };
    let _ = stream.set_send_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_recv_timeout(Some(IO_TIMEOUT));

    let at = |source| AppError::Ipc {
        endpoint: shown.clone(),
        source,
    };

    let mut line = serde_json::to_vec(request)?;
    line.push(b'\n');
    (&stream).write_all(&line).map_err(at)?;
    (&stream).flush().map_err(at)?;

    let mut reply = String::new();
    BufReader::new(&stream).read_line(&mut reply).map_err(at)?;
    if reply.trim().is_empty() {
        return Err(AppError::IpcProtocol(
            "the app closed the connection without answering".to_string(),
        ));
    }

    serde_json::from_str(&reply)
        .map(Some)
        .map_err(|e| AppError::IpcProtocol(e.to_string()))
}

/// The app's end of the socket.
#[derive(Debug)]
pub struct Server {
    listener: interprocess::local_socket::Listener,
    /// The socket path or pipe name, for error messages.
    endpoint: String,
}

/// Claim the socket, so that `wabi start` reaches this process.
///
/// A socket file left behind by an app that crashed would otherwise make this
/// fail forever, so `AddrInUse` is answered by trying to *connect*: if something
/// answers, another app really is running and this one keeps its hands off; if
/// nothing does, the file is debris and gets overwritten.
pub fn listen(config_dir: &Path) -> Result<Server> {
    let (name, endpoint) = endpoint(config_dir)?;

    let bind = |name: Name<'static>, overwrite: bool| {
        ListenerOptions::new()
            .name(name)
            .try_overwrite(overwrite)
            .create_sync()
    };

    let listener = match bind(name.clone(), false) {
        Ok(listener) => listener,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            if Stream::connect(name.clone()).is_ok() {
                return Err(AppError::Ipc {
                    endpoint,
                    source: e,
                });
            }
            bind(name, true).map_err(|source| AppError::Ipc {
                endpoint: endpoint.clone(),
                source,
            })?
        }
        Err(source) => return Err(AppError::Ipc { endpoint, source }),
    };

    Ok(Server { listener, endpoint })
}

impl Server {
    /// The socket path or pipe name this server answers on.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Answer one caller, blocking until one arrives.
    pub fn serve_one(&self, handler: &impl Fn(Request) -> Response) -> Result<()> {
        let stream = self.listener.accept().map_err(|source| AppError::Ipc {
            endpoint: self.endpoint.clone(),
            source,
        })?;
        let _ = stream.set_send_timeout(Some(IO_TIMEOUT));
        let _ = stream.set_recv_timeout(Some(IO_TIMEOUT));

        let mut line = String::new();
        if BufReader::new(&stream).read_line(&mut line).is_err() {
            // A caller that hung up mid-sentence is not this app's problem.
            return Ok(());
        }

        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => handler(request),
            // Answered rather than dropped: a `wabi` newer than this app would
            // otherwise see silence, read it as "no app running", and start a
            // second timer beside the one already going.
            Err(e) => Response::Refused {
                reason: format!("this WabiCalendar does not understand the request ({e})"),
            },
        };

        let mut reply = serde_json::to_vec(&response)?;
        reply.push(b'\n');
        let _ = (&stream).write_all(&reply);
        let _ = (&stream).flush();
        Ok(())
    }

    /// Answer callers until the socket stops producing them.
    ///
    /// Deliberately infallible and deliberately not fatal: this runs on a thread
    /// of its own, and a socket that has gone bad costs the CLI's shortcut, not
    /// the app. `wabi start` finding nobody home runs its own timer instead.
    pub fn serve(self, handler: impl Fn(Request) -> Response) {
        let mut failures = 0;
        while failures < MAX_CONSECUTIVE_FAILURES {
            match self.serve_one(&handler) {
                Ok(()) => failures = 0,
                Err(_) => failures += 1,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::mpsc;
    use std::thread;

    use tempfile::TempDir;

    fn start_request() -> Request {
        Request::Start {
            label: Some("写论文".to_string()),
            planned_sec: Some(1500),
            vault: Some(PathBuf::from("/tmp/vault")),
        }
    }

    /// Serve exactly one caller on a thread, handing back what it was asked.
    fn serve_once(
        config_dir: &Path,
        response: Response,
    ) -> (thread::JoinHandle<()>, mpsc::Receiver<Request>) {
        let server = listen(config_dir).expect("listen");
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            server
                .serve_one(&move |request| {
                    tx.send(request).expect("record the request");
                    response.clone()
                })
                .expect("serve");
        });
        (handle, rx)
    }

    #[test]
    fn a_request_reaches_the_app_and_the_reply_comes_back() {
        let dir = TempDir::new().expect("tempdir");
        let reply = Response::Started {
            planned_sec: 1500,
            label: Some("写论文".to_string()),
        };
        let (server, received) = serve_once(dir.path(), reply.clone());

        let answer = send(dir.path(), &start_request()).expect("send");

        server.join().expect("server thread");
        assert_eq!(answer, Some(reply));
        assert_eq!(received.recv().expect("a request"), start_request());
    }

    /// The ordinary case: the app is simply not open, which is not a failure and
    /// must be distinguishable from one.
    #[test]
    fn nothing_listening_is_an_answer_rather_than_an_error() {
        let dir = TempDir::new().expect("tempdir");

        let answer = send(dir.path(), &start_request()).expect("send");

        assert_eq!(answer, None);
    }

    /// A refusal is a real answer. The CLI must not read it as "no app" and go
    /// on to start a second timer.
    #[test]
    fn a_refusal_comes_back_as_a_refusal() {
        let dir = TempDir::new().expect("tempdir");
        let reply = Response::Refused {
            reason: "a different vault is open".to_string(),
        };
        let (server, _received) = serve_once(dir.path(), reply.clone());

        let answer = send(dir.path(), &start_request()).expect("send");

        server.join().expect("server thread");
        assert_eq!(answer, Some(reply));
    }

    /// Version skew, from the app's side: a request it cannot parse is answered,
    /// not dropped, because silence is what "no app is running" looks like.
    #[test]
    fn a_request_the_app_cannot_understand_is_refused_out_loud() {
        let dir = TempDir::new().expect("tempdir");
        let server = listen(dir.path()).expect("listen");
        let thread = thread::spawn(move || {
            server
                .serve_one(&|_| panic!("the handler must not be reached"))
                .expect("serve");
        });

        let (name, _) = endpoint(dir.path()).expect("endpoint");
        let stream = Stream::connect(name).expect("connect");
        (&stream)
            .write_all(b"{\"cmd\":\"teleport\"}\n")
            .expect("write");
        let mut reply = String::new();
        BufReader::new(&stream)
            .read_line(&mut reply)
            .expect("read the reply");

        thread.join().expect("server thread");
        let answer: Response = serde_json::from_str(&reply).expect("a well-formed reply");
        assert!(
            matches!(answer, Response::Refused { .. }),
            "{answer:?} should have been a refusal"
        );
    }

    /// A socket file left behind by an app that crashed must not lock the next
    /// one out of its own socket for good.
    #[cfg(unix)]
    #[test]
    fn debris_from_a_crashed_app_is_reclaimed() {
        let dir = TempDir::new().expect("tempdir");
        // What a kill -9 leaves behind: the socket file, with nobody answering.
        // Dropping a listener normally takes its file with it, so the file has
        // to be orphaned deliberately.
        let (name, _) = endpoint(dir.path()).expect("endpoint");
        let mut orphan = ListenerOptions::new()
            .name(name)
            .create_sync()
            .expect("bind");
        orphan.do_not_reclaim_name_on_drop();
        drop(orphan);
        assert!(socket_path(dir.path()).exists());

        let reply = Response::Started {
            planned_sec: 60,
            label: None,
        };
        let (server, _received) = serve_once(dir.path(), reply.clone());
        let answer = send(dir.path(), &start_request()).expect("send");

        server.join().expect("server thread");
        assert_eq!(answer, Some(reply));
    }

    /// Two apps at once would each write the same session file, so the second
    /// one has to lose the socket rather than take it away from the first.
    #[cfg(unix)]
    #[test]
    fn a_socket_someone_is_really_answering_on_is_left_alone() {
        let dir = TempDir::new().expect("tempdir");
        let _first = listen(dir.path()).expect("listen");

        let err = listen(dir.path()).expect_err("the second should be shut out");

        assert!(matches!(err, AppError::Ipc { .. }), "{err:?}");
    }
}
