//! Invariant #2: the countdown lives in Rust; the frontend only displays it.
//!
//! Two clocks are held at once. Elapsed time is *always* measured with the
//! monotonic clock, so moving the system clock or an NTP step cannot stretch or
//! shrink a pomodoro. The wall clock is never counted with -- it serves only as
//! a suspend detector. Whether `Instant` keeps running across a sleep differs by
//! platform (Linux's `CLOCK_MONOTONIC` does not advance while suspended), so the
//! one portable signal is the wall clock racing ahead of the monotonic one. When
//! it does, that pomodoro is void and the user is told, rather than the
//! countdown quietly carrying on as if nothing had happened.
//!
//! [`Timer::observe`] is the only method that moves the state machine, which is
//! what lets the tray ticker and the frontend's poll share one timer without
//! double-counting or firing the same notification twice.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::fs_atomic::atomic_write;

/// The wall clock running this much further than the monotonic clock over the
/// same stretch means the machine was suspended in between.
///
/// The cost of this threshold is that a large NTP *step* looks like a sleep and
/// voids the segment. That is the direction invariant #2 chooses: a false
/// "invalidated" is recoverable, a silently wrong pomodoro is not. Ordinary NTP
/// slew is far below this.
const SLEEP_TOLERANCE: Duration = Duration::from_secs(10);

/// How stale a persisted heartbeat may be and still be resumed from.
///
/// Beyond this we were not running to observe what happened, so we cannot rule
/// out a suspend and must not pretend otherwise.
const RESTORE_TOLERANCE: Duration = Duration::from_secs(30);

/// How often a running timer rewrites its heartbeat.
///
/// Must stay comfortably under [`RESTORE_TOLERANCE`], or a clean restart would
/// look like a gap.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

pub const DEFAULT_WORK: Duration = Duration::from_secs(25 * 60);
pub const DEFAULT_BREAK: Duration = Duration::from_secs(5 * 60);

/// The two readings, taken together, that everything else is derived from.
///
/// `Instant` cannot be constructed at an arbitrary value, so this trait hands
/// back a `Duration` since some fixed base rather than an `Instant`. That is
/// what makes the sleep detection testable: a fake can advance its monotonic and
/// wall readings independently, which is precisely what a suspend looks like.
pub trait Clock: Send + Sync + 'static {
    /// Monotonic reading. Only ever compared against another reading from the
    /// same instance.
    fn monotonic(&self) -> Duration;
    fn wall(&self) -> SystemTime;
}

#[derive(Debug)]
pub struct RealClock {
    base: Instant,
}

impl RealClock {
    pub fn new() -> Self {
        Self {
            base: Instant::now(),
        }
    }
}

impl Default for RealClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for RealClock {
    fn monotonic(&self) -> Duration {
        self.base.elapsed()
    }

    fn wall(&self) -> SystemTime {
        SystemTime::now()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Work,
    Break,
}

impl Phase {
    pub fn next(self) -> Self {
        match self {
            Phase::Work => Phase::Break,
            Phase::Break => Phase::Work,
        }
    }

    /// Whether the hand-over *into* this phase begins counting on its own.
    ///
    /// Only the break does. The whole point of a break is that the user has got
    /// up and walked away, so a break that waits to be pressed is a break that
    /// silently never happens. Work is the other way round: a pomodoro nobody
    /// chose to begin is not a pomodoro, and if both directions ran on their own
    /// the app would never once come to rest.
    ///
    /// Two pieces of wording follow this rule and have to move with it: the
    /// notification body in `integrations::notify::announce`, and the
    /// announcement line in `TimerPanel.vue`.
    fn auto_starts(self) -> bool {
        matches!(self, Phase::Break)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunState {
    /// Nothing running; `phase` names what will start.
    Idle,
    Running,
    Paused,
    /// The previous segment ran to its planned length. `phase` has already moved
    /// on to what is queued next -- this differs from `Idle` only so the UI can
    /// announce the hand-over.
    ///
    /// Only reached when the queued phase does not start itself
    /// ([`Phase::auto_starts`]), which today means only at the end of a break.
    Finished,
    /// The machine slept mid-segment, so it does not count. Cleared by starting
    /// anything.
    Invalidated,
}

/// What the frontend and the tray both render. Mirrored by hand in
/// `src/types/timer.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimerState {
    pub phase: Phase,
    pub run: RunState,
    pub planned_sec: u64,
    pub elapsed_sec: u64,
    pub remaining_sec: u64,
}

/// Something that just changed, reported once and only once.
///
/// Side effects (the notification, the tray text) hang off these rather than off
/// the polled state, so that two readers of the same timer cannot fire them
/// twice.
// `rename_all` only touches the variant names; the fields inside them need
// `rename_all_fields` as well, or the wire would mix `finished` with `planned_sec`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Transition {
    /// A segment reached its planned length.
    Finished {
        /// The segment that just ended.
        phase: Phase,
        /// What is queued now.
        next: Phase,
        planned_sec: u64,
    },
    /// The machine slept through part of a segment.
    Invalidated {
        phase: Phase,
        /// How far the segment had got before it was voided.
        elapsed_sec: u64,
        /// How much wall time went missing.
        slept_sec: u64,
    },
}

/// How a segment stopped counting. Written straight into the session record.
///
/// Lives here rather than in [`crate::sessions`] because the timer is what
/// decides it — the session file only carries the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    /// Ran to its planned length.
    Completed,
    /// The user cut it short.
    Aborted,
    /// The machine slept through part of it, so it does not count.
    Invalidated,
}

/// A segment that ended, ready to be written to `sessions/`.
///
/// Deliberately *not* a [`Transition`]: transitions drive the notification and
/// the tray, and pressing reset on a half-run pomodoro has to be recorded
/// without anything being announced out loud. Keeping the two apart is what
/// lets [`Outcome::Aborted`] exist without a notification attached to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ended {
    pub phase: Phase,
    pub planned_sec: u64,
    /// What the monotonic clock counted, which is *not* the gap between the two
    /// stamps below whenever the segment spent time paused.
    pub actual_sec: u64,
    pub started_at: SystemTime,
    pub ended_at: SystemTime,
    pub outcome: Outcome,
}

/// The clock readings at the moment the current run stretch began.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Mark {
    mono: Duration,
    wall: SystemTime,
}

/// The part of the timer that survives a restart.
///
/// Monotonic readings are meaningless across processes, so what crosses the
/// restart boundary is banked time plus a wall-clock heartbeat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Persisted {
    phase: Phase,
    run: RunState,
    /// Whole seconds: a pomodoro does not care about the sub-second remainder,
    /// and this keeps the file legible.
    banked_sec: u64,
    /// Wall clock at the last write, as seconds since the Unix epoch. Present
    /// only while running.
    heartbeat_unix: Option<u64>,
    /// When the current segment first began counting, as seconds since the Unix
    /// epoch. Without this a restart could finish a pomodoro it has no way to
    /// date, and the session record would have to invent a start.
    ///
    /// `default` so that a state file written before sessions existed still
    /// loads; the segment it describes simply goes unrecorded.
    #[serde(default)]
    started_at_unix: Option<u64>,
}

pub struct Timer<C: Clock> {
    clock: C,
    path: PathBuf,
    phase: Phase,
    run: RunState,
    work: Duration,
    brk: Duration,
    /// Time banked from earlier run stretches of the current segment.
    banked: Duration,
    /// Set while running; `None` otherwise.
    since: Option<Mark>,
    /// Wall clock at the moment the current segment first began counting, kept
    /// across pauses. `None` when nothing is under way.
    ///
    /// The wall clock rather than the monotonic one because this is the only
    /// reading that means anything to a calendar: it is where the block gets
    /// drawn. Elapsed time is still counted monotonically — this stamp is never
    /// subtracted from anything.
    started_at: Option<SystemTime>,
    /// Segments that have ended and not yet been written to `sessions/`.
    ended: Vec<Ended>,
    /// Monotonic reading of the last successful persist, for heartbeat pacing.
    last_persist: Option<Duration>,
    /// The state moved in a way a cold start would need to know about.
    ///
    /// Transitions alone are not enough to drive persistence: pausing produces
    /// no transition at all, yet it changes everything about how a restart
    /// should read the file. Without this, a pause would leave `running` on disk
    /// and the next launch would credit -- or invalidate -- time that never ran.
    dirty: bool,
}

impl<C: Clock> Timer<C> {
    /// Restore a timer from disk, reporting anything that happened while we were
    /// not running.
    ///
    /// Deliberately infallible, unlike [`crate::settings::Settings::load`]: this
    /// file is disposable runtime state, so a missing or damaged one costs the
    /// user nothing, and refusing to start over it would be far worse than
    /// starting idle.
    pub fn load(clock: C, path: PathBuf, work: Duration, brk: Duration) -> (Self, Vec<Transition>) {
        let mut timer = Self {
            clock,
            path,
            phase: Phase::Work,
            run: RunState::Idle,
            work,
            brk,
            banked: Duration::ZERO,
            since: None,
            started_at: None,
            ended: Vec::new(),
            last_persist: None,
            dirty: false,
        };

        let Some(saved) = timer.read_persisted() else {
            return (timer, Vec::new());
        };

        timer.phase = saved.phase;
        timer.run = saved.run;
        timer.banked = Duration::from_secs(saved.banked_sec);
        timer.started_at = saved
            .started_at_unix
            .map(|secs| UNIX_EPOCH + Duration::from_secs(secs));

        // Only a timer that was running has a gap to account for.
        if saved.run != RunState::Running {
            return (timer, Vec::new());
        }

        let heartbeat = saved
            .heartbeat_unix
            .map(|secs| UNIX_EPOCH + Duration::from_secs(secs));
        let gap = heartbeat.and_then(|hb| timer.clock.wall().duration_since(hb).ok());

        // No usable heartbeat means a hand-edited or truncated file: we cannot
        // date the gap, which is itself reason enough not to trust the segment.
        if gap.is_none_or(|g| g > RESTORE_TOLERANCE) {
            // We were not there to watch, so a suspend cannot be ruled out.
            let elapsed_sec = timer.banked.as_secs();
            let phase = timer.phase;
            // The heartbeat is the last moment we know the timer was alive, so
            // that is where the segment stopped counting. Dating it "now"
            // instead would paint a block across however long the machine was
            // away, which is precisely the time nothing was happening.
            timer.close(Outcome::Invalidated, timer.banked, heartbeat);
            timer.invalidate();
            return (
                timer,
                vec![Transition::Invalidated {
                    phase,
                    elapsed_sec,
                    slept_sec: gap.unwrap_or(Duration::ZERO).as_secs(),
                }],
            );
        }
        let gap = gap.unwrap_or(Duration::ZERO);

        // A quick restart: the app was down, but the wall clock says only moments
        // passed, so that time genuinely belongs to the segment.
        timer.banked += gap;
        timer.mark_running();
        let (_, transitions) = timer.observe();
        (timer, transitions)
    }

    fn read_persisted(&self) -> Option<Persisted> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn planned(&self) -> Duration {
        match self.phase {
            Phase::Work => self.work,
            Phase::Break => self.brk,
        }
    }

    fn mark_running(&mut self) {
        self.run = RunState::Running;
        self.since = Some(Mark {
            mono: self.clock.monotonic(),
            wall: self.clock.wall(),
        });
        self.dirty = true;
    }

    fn invalidate(&mut self) {
        self.run = RunState::Invalidated;
        self.since = None;
        self.banked = Duration::ZERO;
        self.dirty = true;
    }

    /// Bank a segment that has ended, for [`Timer::take_ended`] to hand on to
    /// `sessions/`.
    ///
    /// `ended_at` of `None` means we cannot observe the moment it stopped and
    /// have to place it at `started_at` plus what was counted. Must be called
    /// *before* `phase` moves on, since the planned length is read off it.
    ///
    /// Records nothing at all when there is no start stamp. A hand-edited state
    /// file can leave a running timer with no idea when it began, and a record
    /// whose `started_at` we invented would be worse than no record.
    fn close(&mut self, outcome: Outcome, actual: Duration, ended_at: Option<SystemTime>) {
        let Some(started_at) = self.started_at.take() else {
            return;
        };
        self.dirty = true;

        let ended_at = ended_at.unwrap_or(started_at + actual);
        self.ended.push(Ended {
            phase: self.phase,
            planned_sec: self.planned().as_secs(),
            actual_sec: actual.as_secs(),
            started_at,
            // A wall clock dragged backwards mid-segment would otherwise write a
            // record that ends before it starts, and draw a block of negative
            // height.
            ended_at: ended_at.max(started_at),
            outcome,
        });
    }

    /// Close the segment under way, if there is one, as abandoned.
    ///
    /// Sub-second segments leave nothing behind: pressing start and immediately
    /// resetting is a slip, not a pomodoro. `Finished` and `Invalidated` have
    /// already been recorded and banked nothing, so they cannot reach the guard.
    fn abandon(&mut self) {
        let elapsed = self.elapsed().min(self.planned());
        if elapsed.as_secs() == 0 {
            self.started_at = None;
            return;
        }
        let now = self.clock.wall();
        self.close(Outcome::Aborted, elapsed, Some(now));
    }

    /// Segments that ended since the last call. Empty almost every time.
    pub fn take_ended(&mut self) -> Vec<Ended> {
        std::mem::take(&mut self.ended)
    }

    /// Elapsed time in the current segment, without advancing anything.
    fn elapsed(&self) -> Duration {
        match self.since {
            Some(mark) if self.run == RunState::Running => {
                self.banked + self.clock.monotonic().saturating_sub(mark.mono)
            }
            _ => self.banked,
        }
    }

    /// A read-only snapshot. Does not detect a finished segment or a suspend --
    /// [`Timer::observe`] does that.
    pub fn state(&self) -> TimerState {
        let planned = self.planned();
        let elapsed = self.elapsed().min(planned);
        let remaining = planned.saturating_sub(elapsed);
        TimerState {
            phase: self.phase,
            run: self.run,
            planned_sec: planned.as_secs(),
            elapsed_sec: elapsed.as_secs(),
            // Rounded up, so a 25 minute segment reads "25:00" for its whole
            // first second and only reaches "00:00" when it is actually over.
            // Truncating would show "24:59" the instant the user pressed start.
            remaining_sec: remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0),
        }
    }

    /// Advance the timer to now and report what just changed.
    ///
    /// Idempotent with respect to time: called twice without the clock moving it
    /// returns the same state and an empty transition list the second time.
    pub fn observe(&mut self) -> (TimerState, Vec<Transition>) {
        if self.run != RunState::Running {
            return (self.state(), Vec::new());
        }

        // A running timer without a mark can only come from a hand-edited state
        // file; re-mark rather than lose the segment.
        let Some(mark) = self.since else {
            self.mark_running();
            return (self.state(), Vec::new());
        };

        let d_mono = self.clock.monotonic().saturating_sub(mark.mono);
        // A wall clock that went *backwards* needs no special case: elapsed time
        // comes from the monotonic clock, so the count is unaffected, and a
        // zero delta simply fails the divergence test below.
        let d_wall = self
            .clock
            .wall()
            .duration_since(mark.wall)
            .unwrap_or(Duration::ZERO);

        if d_wall > d_mono + SLEEP_TOLERANCE {
            let slept = d_wall - d_mono;
            let counted = (self.banked + d_mono).min(self.planned());
            let elapsed_sec = counted.as_secs();
            let phase = self.phase;
            // Only `d_mono` of that stretch was spent awake, so the machine went
            // under at roughly `mark.wall + d_mono`. That is where the block
            // stops; carrying it to now would draw the sleep as if it were work.
            self.close(Outcome::Invalidated, counted, Some(mark.wall + d_mono));
            self.invalidate();
            return (
                self.state(),
                vec![Transition::Invalidated {
                    phase,
                    elapsed_sec,
                    slept_sec: slept.as_secs(),
                }],
            );
        }

        let planned = self.planned();
        if self.banked + d_mono < planned {
            return (self.state(), Vec::new());
        }

        // The segment is what was planned, not however long we happened to take
        // to notice it had ended. `close` reads the planned length off the
        // current phase, so it has to run before the hand-over below.
        let now = self.clock.wall();
        let finished = self.phase;
        self.close(Outcome::Completed, planned, Some(now));

        let next = finished.next();
        self.phase = next;
        self.banked = Duration::ZERO;
        self.since = None;
        self.dirty = true;

        if next.auto_starts() {
            // The same wall reading `close` just wrote as the old segment's
            // `ended_at`, so the two records meet exactly rather than
            // overlapping or leaving a gap.
            //
            // The overshoot -- however far past `planned` we were by the time
            // anyone looked -- is deliberately *not* carried over. It belongs to
            // neither segment, and banking it here would let a single `observe`
            // finish two segments at once (a restart may credit up to
            // `RESTORE_TOLERANCE` of it), which would report one transition and
            // swallow the other.
            self.started_at = Some(now);
            self.mark_running();
        } else {
            self.run = RunState::Finished;
        }

        (
            self.state(),
            vec![Transition::Finished {
                phase: finished,
                next,
                planned_sec: planned.as_secs(),
            }],
        )
    }

    /// Start the queued segment from zero.
    ///
    /// Starting over a segment that was still under way abandons it, and an
    /// abandoned segment is still something that happened to the user's day.
    pub fn start(&mut self) {
        self.abandon();
        self.banked = Duration::ZERO;
        self.started_at = Some(self.clock.wall());
        self.mark_running();
    }

    /// Space-bar semantics: running pauses, anything else starts or resumes.
    pub fn toggle(&mut self) {
        match self.run {
            RunState::Running => {
                // Bank what has run so far, then stop counting.
                self.banked = self.elapsed().min(self.planned());
                self.run = RunState::Paused;
                self.since = None;
                self.dirty = true;
            }
            RunState::Paused => self.mark_running(),
            // Idle, Finished and Invalidated all begin a fresh segment.
            _ => self.start(),
        }
    }

    /// Back to the top of the current segment, not running.
    pub fn reset(&mut self) {
        self.abandon();
        self.run = RunState::Idle;
        self.banked = Duration::ZERO;
        self.since = None;
        self.dirty = true;
    }

    pub fn set_durations(&mut self, work: Duration, brk: Duration) {
        self.work = work;
        self.brk = brk;
        self.dirty = true;
    }

    pub fn durations(&self) -> (Duration, Duration) {
        (self.work, self.brk)
    }

    /// True when the state file no longer matches what is in memory.
    ///
    /// Covers both halves: a state change that has not been written yet, and a
    /// running timer whose heartbeat has gone stale.
    pub fn persist_due(&self) -> bool {
        self.dirty || self.heartbeat_due()
    }

    /// True when a running timer's heartbeat is stale enough to rewrite.
    fn heartbeat_due(&self) -> bool {
        if self.run != RunState::Running {
            return false;
        }
        match self.last_persist {
            None => true,
            Some(at) => self.clock.monotonic().saturating_sub(at) >= HEARTBEAT_INTERVAL,
        }
    }

    /// Write the timer's state where a cold start can find it.
    ///
    /// Goes through `fs_atomic` like every other write in the app (invariant #3).
    pub fn persist(&mut self) -> Result<()> {
        let saved = Persisted {
            phase: self.phase,
            run: self.run,
            banked_sec: self.elapsed().min(self.planned()).as_secs(),
            heartbeat_unix: match self.run {
                RunState::Running => self
                    .clock
                    .wall()
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs()),
                _ => None,
            },
            started_at_unix: self
                .started_at
                .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs()),
        };

        let json = serde_json::to_vec_pretty(&saved)?;
        atomic_write(&self.path, &json)?;
        self.last_persist = Some(self.clock.monotonic());
        self.dirty = false;
        Ok(())
    }
}

/// Where the timer parks its state. Machine-local runtime state, not user data,
/// so it lives beside `settings.toml` rather than in the vault.
pub fn state_path(config_dir: &Path) -> PathBuf {
    config_dir.join("timer.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    use tempfile::TempDir;

    /// A clock whose two readings advance independently -- which is exactly what
    /// a suspend, or a system-clock change, looks like from inside the process.
    struct FakeClock {
        inner: Mutex<(Duration, SystemTime)>,
    }

    impl FakeClock {
        fn new() -> Self {
            Self {
                inner: Mutex::new((
                    Duration::ZERO,
                    UNIX_EPOCH + Duration::from_secs(1_700_000_000),
                )),
            }
        }

        /// Ordinary time passing: both clocks move together.
        fn advance(&self, by: Duration) {
            self.advance_split(by, by);
        }

        fn advance_split(&self, mono: Duration, wall: Duration) {
            let mut g = self.inner.lock().expect("fake clock");
            g.0 += mono;
            g.1 += wall;
        }

        fn rewind_wall(&self, by: Duration) {
            let mut g = self.inner.lock().expect("fake clock");
            g.1 -= by;
        }
    }

    impl Clock for FakeClock {
        fn monotonic(&self) -> Duration {
            self.inner.lock().expect("fake clock").0
        }

        fn wall(&self) -> SystemTime {
            self.inner.lock().expect("fake clock").1
        }
    }

    const WORK: Duration = Duration::from_secs(60);
    const BREAK: Duration = Duration::from_secs(30);

    fn timer(dir: &TempDir) -> Timer<FakeClock> {
        let (t, _) = Timer::load(FakeClock::new(), state_path(dir.path()), WORK, BREAK);
        t
    }

    #[test]
    fn a_running_segment_counts_down_by_the_monotonic_clock() {
        let dir = TempDir::new().expect("tempdir");
        let mut t = timer(&dir);

        t.start();
        t.clock.advance(Duration::from_secs(20));
        let (state, transitions) = t.observe();

        assert_eq!(state.run, RunState::Running);
        assert_eq!(state.elapsed_sec, 20);
        assert_eq!(state.remaining_sec, 40);
        assert!(transitions.is_empty());
    }

    #[test]
    fn the_remaining_time_reads_full_until_the_first_second_is_actually_gone() {
        let dir = TempDir::new().expect("tempdir");
        let mut t = timer(&dir);

        t.start();
        assert_eq!(t.state().remaining_sec, 60);

        t.clock.advance(Duration::from_millis(1));
        assert_eq!(t.state().remaining_sec, 60, "a millisecond is not a second");

        t.clock.advance(Duration::from_millis(999));
        assert_eq!(t.state().remaining_sec, 59);
    }

    #[test]
    fn a_wall_clock_jump_far_beyond_the_monotonic_delta_invalidates_the_segment() {
        let dir = TempDir::new().expect("tempdir");
        let mut t = timer(&dir);

        t.start();
        // The lid was shut for half an hour; the monotonic clock barely moved.
        t.clock
            .advance_split(Duration::from_secs(1), Duration::from_secs(1800));
        let (state, transitions) = t.observe();

        assert_eq!(state.run, RunState::Invalidated);
        assert_eq!(
            transitions,
            vec![Transition::Invalidated {
                phase: Phase::Work,
                elapsed_sec: 1,
                slept_sec: 1799,
            }]
        );
    }

    #[test]
    fn a_small_wall_clock_correction_within_tolerance_does_not_invalidate() {
        let dir = TempDir::new().expect("tempdir");
        let mut t = timer(&dir);

        t.start();
        // An NTP nudge: the wall clock gains a few seconds on the monotonic one.
        t.clock
            .advance_split(Duration::from_secs(10), Duration::from_secs(14));
        let (state, transitions) = t.observe();

        assert_eq!(state.run, RunState::Running);
        // Counted by the monotonic clock, not the one that jumped.
        assert_eq!(state.elapsed_sec, 10);
        assert!(transitions.is_empty());
    }

    #[test]
    fn a_backwards_wall_clock_does_not_corrupt_the_count() {
        let dir = TempDir::new().expect("tempdir");
        let mut t = timer(&dir);

        t.start();
        t.clock.advance(Duration::from_secs(15));
        t.clock.rewind_wall(Duration::from_secs(3600));
        let (state, transitions) = t.observe();

        assert_eq!(state.run, RunState::Running);
        assert_eq!(state.elapsed_sec, 15);
        assert!(transitions.is_empty());
    }

    #[test]
    fn pausing_stops_the_count_and_resuming_continues_from_the_same_elapsed() {
        let dir = TempDir::new().expect("tempdir");
        let mut t = timer(&dir);

        t.start();
        t.clock.advance(Duration::from_secs(10));
        t.toggle();
        assert_eq!(t.state().run, RunState::Paused);

        // Time passes while paused and must not count.
        t.clock.advance(Duration::from_secs(600));
        assert_eq!(t.state().elapsed_sec, 10);

        t.toggle();
        t.clock.advance(Duration::from_secs(5));
        let (state, _) = t.observe();

        assert_eq!(state.run, RunState::Running);
        assert_eq!(state.elapsed_sec, 15);
    }

    #[test]
    fn a_finished_segment_reports_its_transition_exactly_once() {
        let dir = TempDir::new().expect("tempdir");
        let mut t = timer(&dir);

        t.start();
        t.clock.advance(WORK);

        let (_, first) = t.observe();
        assert_eq!(
            first,
            vec![Transition::Finished {
                phase: Phase::Work,
                next: Phase::Break,
                planned_sec: 60,
            }]
        );

        // The second reader -- the frontend poll, say -- must get nothing.
        let (state, second) = t.observe();
        assert!(second.is_empty());
        assert_eq!(state.run, RunState::Running, "the break carries on");
        assert_eq!(state.phase, Phase::Break);
    }

    #[test]
    fn observing_twice_after_a_long_jump_still_finishes_the_segment_only_once() {
        let dir = TempDir::new().expect("tempdir");
        let mut t = timer(&dir);

        t.start();
        // Well past the planned length, but monotonic and wall agree, so this is
        // a busy machine rather than a suspend.
        t.clock.advance(Duration::from_secs(300));

        let (state, first) = t.observe();
        assert_eq!(first.len(), 1);
        // The overshoot is not carried into the break. Were it banked, a single
        // late look would finish the work segment *and* run the whole break out
        // on the next call, reporting one hand-over and swallowing the other.
        assert_eq!(state.elapsed_sec, 0);
        let (_, second) = t.observe();
        assert!(second.is_empty());
    }

    /// The break is the one hand-over nobody is at the keyboard for, so it has
    /// to begin on its own.
    #[test]
    fn finishing_a_work_segment_starts_the_break_without_a_press() {
        let dir = TempDir::new().expect("tempdir");
        let mut t = timer(&dir);

        t.start();
        t.clock.advance(WORK);
        let (state, _) = t.observe();

        assert_eq!(state.phase, Phase::Break);
        assert_eq!(state.run, RunState::Running);
        assert_eq!(state.planned_sec, 30);
        assert_eq!(state.remaining_sec, 30);
        assert_eq!(state.elapsed_sec, 0, "the break starts from the top");

        // And it really is counting, not merely labelled as running.
        t.clock.advance(Duration::from_secs(10));
        assert_eq!(t.state().remaining_sec, 20);
    }

    /// The other direction stays manual: a pomodoro nobody chose to begin is not
    /// a pomodoro, and two automatic hand-overs would never come to rest.
    #[test]
    fn finishing_a_break_waits_for_the_user() {
        let dir = TempDir::new().expect("tempdir");
        let mut t = timer(&dir);

        t.start();
        t.clock.advance(WORK);
        t.observe();

        t.clock.advance(BREAK);
        let (state, transitions) = t.observe();

        assert_eq!(state.phase, Phase::Work);
        assert_eq!(state.run, RunState::Finished);
        assert_eq!(
            transitions,
            vec![Transition::Finished {
                phase: Phase::Break,
                next: Phase::Work,
                planned_sec: 30,
            }]
        );
    }

    #[test]
    fn a_cold_start_within_the_heartbeat_window_restores_the_running_segment() {
        let dir = TempDir::new().expect("tempdir");
        let path = state_path(dir.path());

        let mut first = timer(&dir);
        first.start();
        first.clock.advance(Duration::from_secs(20));
        first.persist().expect("persist");

        // A fresh process: new monotonic base, wall clock a few seconds on.
        let restarted = FakeClock::new();
        restarted.advance_split(Duration::ZERO, Duration::from_secs(20 + 5));
        let (t, transitions) = Timer::load(restarted, path, WORK, BREAK);

        assert!(transitions.is_empty());
        let state = t.state();
        assert_eq!(state.run, RunState::Running);
        // 20 banked before the crash, plus the 5 second gap.
        assert_eq!(state.elapsed_sec, 25);
    }

    #[test]
    fn a_cold_start_after_a_long_gap_invalidates_instead_of_pretending() {
        let dir = TempDir::new().expect("tempdir");
        let path = state_path(dir.path());

        let mut first = timer(&dir);
        first.start();
        first.clock.advance(Duration::from_secs(20));
        first.persist().expect("persist");

        let restarted = FakeClock::new();
        restarted.advance_split(Duration::ZERO, Duration::from_secs(20 + 600));
        let (t, transitions) = Timer::load(restarted, path, WORK, BREAK);

        assert_eq!(t.state().run, RunState::Invalidated);
        assert_eq!(
            transitions,
            vec![Transition::Invalidated {
                phase: Phase::Work,
                elapsed_sec: 20,
                slept_sec: 600,
            }]
        );
    }

    #[test]
    fn a_paused_timer_survives_a_restart_without_gaining_time() {
        let dir = TempDir::new().expect("tempdir");
        let path = state_path(dir.path());

        let mut first = timer(&dir);
        first.start();
        first.clock.advance(Duration::from_secs(12));
        first.toggle();
        first.persist().expect("persist");

        // Hours pass with the app shut; a paused segment owes nothing to them.
        let restarted = FakeClock::new();
        restarted.advance_split(Duration::ZERO, Duration::from_secs(60 * 60 * 5));
        let (t, transitions) = Timer::load(restarted, path, WORK, BREAK);

        assert!(transitions.is_empty());
        assert_eq!(t.state().run, RunState::Paused);
        assert_eq!(t.state().elapsed_sec, 12);
    }

    #[test]
    fn timer_state_round_trips_through_disk() {
        let dir = TempDir::new().expect("tempdir");
        let path = state_path(dir.path());

        // Through both hand-overs, so what lands on disk is a `Finished` waiting
        // on the user rather than a running break.
        let mut first = timer(&dir);
        first.start();
        first.clock.advance(WORK);
        first.observe();
        first.clock.advance(BREAK);
        first.observe();
        first.persist().expect("persist");

        let (t, transitions) = Timer::load(FakeClock::new(), path, WORK, BREAK);

        assert!(transitions.is_empty());
        assert_eq!(t.state().phase, Phase::Work);
        assert_eq!(t.state().run, RunState::Finished);
    }

    /// A break that began by itself is an ordinary running segment: it is
    /// persisted and restored like any other, rather than being lost because
    /// nobody pressed anything to create it.
    #[test]
    fn an_auto_started_break_survives_a_restart() {
        let dir = TempDir::new().expect("tempdir");
        let path = state_path(dir.path());

        let mut first = timer(&dir);
        first.start();
        first.clock.advance(WORK);
        first.observe();
        assert!(first.persist_due(), "the hand-over must reach the disk");
        first.persist().expect("persist");

        let restarted = FakeClock::new();
        restarted.advance_split(Duration::ZERO, WORK + Duration::from_secs(5));
        let (t, transitions) = Timer::load(restarted, path, WORK, BREAK);

        assert!(transitions.is_empty());
        assert_eq!(t.state().phase, Phase::Break);
        assert_eq!(t.state().run, RunState::Running);
        assert_eq!(t.state().elapsed_sec, 5, "the gap belongs to the break");
    }

    #[test]
    fn a_damaged_state_file_starts_idle_rather_than_refusing_to_run() {
        let dir = TempDir::new().expect("tempdir");
        let path = state_path(dir.path());
        std::fs::write(&path, "{ not json").expect("write");

        let (t, transitions) = Timer::load(FakeClock::new(), path, WORK, BREAK);

        assert!(transitions.is_empty());
        assert_eq!(t.state().run, RunState::Idle);
        assert_eq!(t.state().phase, Phase::Work);
    }

    /// Seconds since the epoch, which is how the tests talk about wall time.
    fn unix(at: SystemTime) -> u64 {
        at.duration_since(UNIX_EPOCH)
            .expect("after the epoch")
            .as_secs()
    }

    /// The wall clock a `FakeClock` starts at.
    const BASE: u64 = 1_700_000_000;

    #[test]
    fn a_completed_segment_is_recorded_at_its_planned_length() {
        let dir = TempDir::new().expect("tempdir");
        let mut t = timer(&dir);

        t.clock.advance(Duration::from_secs(100));
        t.start();
        t.clock.advance(WORK);
        t.observe();

        let ended = t.take_ended();
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].outcome, Outcome::Completed);
        assert_eq!(ended[0].phase, Phase::Work);
        assert_eq!(ended[0].planned_sec, 60);
        assert_eq!(ended[0].actual_sec, 60);
        assert_eq!(unix(ended[0].started_at), BASE + 100);
        assert_eq!(unix(ended[0].ended_at), BASE + 100 + 60);

        // Taken once and once only, exactly like a transition.
        assert!(t.take_ended().is_empty());
    }

    /// A break nobody pressed start on is still a segment of the user's day, and
    /// it begins on the same instant the work segment stopped -- so the week view
    /// draws the two touching rather than overlapping or with a hole between.
    #[test]
    fn the_auto_started_break_is_recorded_as_a_segment_of_its_own() {
        let dir = TempDir::new().expect("tempdir");
        let mut t = timer(&dir);

        t.start();
        t.clock.advance(WORK);
        t.observe();
        let work = t.take_ended();

        t.clock.advance(BREAK);
        t.observe();
        let brk = t.take_ended();

        assert_eq!(work.len(), 1);
        assert_eq!(brk.len(), 1);
        assert_eq!(brk[0].phase, Phase::Break);
        assert_eq!(brk[0].outcome, Outcome::Completed);
        assert_eq!(brk[0].actual_sec, 30);
        assert_eq!(
            unix(brk[0].started_at),
            unix(work[0].ended_at),
            "the break must begin where the work segment stopped"
        );
        assert_eq!(unix(brk[0].ended_at), BASE + 60 + 30);
    }

    /// The block is drawn between the two stamps but measured by `actual_sec`,
    /// so a segment that sat paused has to say both things at once.
    #[test]
    fn a_paused_segment_spans_more_wall_time_than_it_counted() {
        let dir = TempDir::new().expect("tempdir");
        let mut t = timer(&dir);

        t.start();
        t.clock.advance(Duration::from_secs(30));
        t.toggle();
        t.clock.advance(Duration::from_secs(600));
        t.toggle();
        t.clock.advance(Duration::from_secs(30));
        t.observe();

        let ended = t.take_ended();
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].actual_sec, 60, "the pause does not count");
        assert_eq!(
            unix(ended[0].ended_at) - unix(ended[0].started_at),
            660,
            "but it did take that long on the wall"
        );
    }

    #[test]
    fn resetting_a_half_run_segment_records_it_as_abandoned() {
        let dir = TempDir::new().expect("tempdir");
        let mut t = timer(&dir);

        t.start();
        t.clock.advance(Duration::from_secs(17));
        t.reset();

        let ended = t.take_ended();
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].outcome, Outcome::Aborted);
        assert_eq!(ended[0].actual_sec, 17);
        assert_eq!(ended[0].planned_sec, 60);
    }

    #[test]
    fn starting_over_a_running_segment_abandons_it_rather_than_losing_it() {
        let dir = TempDir::new().expect("tempdir");
        let mut t = timer(&dir);

        t.start();
        t.clock.advance(Duration::from_secs(20));
        t.start();

        let ended = t.take_ended();
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].outcome, Outcome::Aborted);
        assert_eq!(ended[0].actual_sec, 20);
    }

    #[test]
    fn a_segment_that_never_got_going_leaves_no_record() {
        let dir = TempDir::new().expect("tempdir");
        let mut t = timer(&dir);

        t.start();
        t.clock.advance(Duration::from_millis(300));
        t.reset();
        assert!(t.take_ended().is_empty(), "a slip is not a pomodoro");

        // Nor does resetting an idle timer, or one that already finished.
        t.reset();
        t.start();
        t.clock.advance(WORK);
        t.observe();
        assert_eq!(t.take_ended().len(), 1);
        t.reset();
        assert!(t.take_ended().is_empty());
    }

    /// Where the block stops is the moment the machine went under, not the
    /// moment we noticed -- otherwise the sleep gets painted as work.
    #[test]
    fn a_voided_segment_ends_where_the_counting_stopped() {
        let dir = TempDir::new().expect("tempdir");
        let mut t = timer(&dir);

        t.start();
        t.clock.advance(Duration::from_secs(12));
        // The lid was shut: half an hour of wall clock, no monotonic time.
        t.clock
            .advance_split(Duration::ZERO, Duration::from_secs(1800));
        t.observe();

        let ended = t.take_ended();
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].outcome, Outcome::Invalidated);
        assert_eq!(ended[0].actual_sec, 12);
        assert_eq!(unix(ended[0].started_at), BASE);
        assert_eq!(
            unix(ended[0].ended_at),
            BASE + 12,
            "the sleep must not be drawn as part of the segment"
        );
    }

    #[test]
    fn a_segment_survives_a_restart_knowing_when_it_began() {
        let dir = TempDir::new().expect("tempdir");
        let path = state_path(dir.path());

        let mut first = timer(&dir);
        first.clock.advance(Duration::from_secs(40));
        first.start();
        first.clock.advance(Duration::from_secs(20));
        first.persist().expect("persist");

        let restarted = FakeClock::new();
        restarted.advance_split(Duration::ZERO, Duration::from_secs(40 + 20 + 5));
        let (mut t, _) = Timer::load(restarted, path, WORK, BREAK);

        t.clock.advance(Duration::from_secs(35));
        t.observe();

        let ended = t.take_ended();
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].outcome, Outcome::Completed);
        assert_eq!(
            unix(ended[0].started_at),
            BASE + 40,
            "the restart forgot when the segment began"
        );
    }

    /// A cold start that cannot rule out a suspend voids the segment, and the
    /// last heartbeat is the last moment we know it was still counting.
    #[test]
    fn a_segment_voided_by_a_cold_start_ends_at_its_last_heartbeat() {
        let dir = TempDir::new().expect("tempdir");
        let path = state_path(dir.path());

        let mut first = timer(&dir);
        first.start();
        first.clock.advance(Duration::from_secs(20));
        first.persist().expect("persist");

        let restarted = FakeClock::new();
        restarted.advance_split(Duration::ZERO, Duration::from_secs(20 + 600));
        let (mut t, _) = Timer::load(restarted, path, WORK, BREAK);

        let ended = t.take_ended();
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].outcome, Outcome::Invalidated);
        assert_eq!(ended[0].actual_sec, 20);
        assert_eq!(unix(ended[0].started_at), BASE);
        assert_eq!(unix(ended[0].ended_at), BASE + 20);
    }

    /// A state file from before sessions existed still loads; the segment it
    /// describes simply goes unrecorded rather than being given an invented
    /// start.
    #[test]
    fn a_segment_with_no_start_stamp_is_not_given_one() {
        let dir = TempDir::new().expect("tempdir");
        let path = state_path(dir.path());
        std::fs::write(
            &path,
            r#"{"phase":"work","run":"paused","bankedSec":30,"heartbeatUnix":null}"#,
        )
        .expect("write");

        let (mut t, transitions) = Timer::load(FakeClock::new(), path, WORK, BREAK);

        assert!(transitions.is_empty());
        assert_eq!(t.state().elapsed_sec, 30);
        t.reset();
        assert!(t.take_ended().is_empty());
    }

    #[test]
    fn the_heartbeat_is_due_only_while_running_and_only_once_stale() {
        let dir = TempDir::new().expect("tempdir");
        let mut t = timer(&dir);

        assert!(!t.persist_due(), "an idle timer has nothing to record");

        t.start();
        assert!(t.persist_due(), "nothing written yet");
        t.persist().expect("persist");
        assert!(!t.persist_due());

        t.clock.advance(HEARTBEAT_INTERVAL);
        assert!(t.persist_due(), "the heartbeat has gone stale");
    }

    #[test]
    fn pausing_reaches_the_disk_even_though_it_produces_no_transition() {
        let dir = TempDir::new().expect("tempdir");
        let mut t = timer(&dir);

        t.start();
        t.persist().expect("persist");
        t.clock.advance(Duration::from_secs(5));

        let (_, transitions) = t.observe();
        assert!(transitions.is_empty(), "a pause is not a transition");

        t.toggle();
        assert!(
            t.persist_due(),
            "a pause the disk never hears about would come back as a running timer"
        );
    }

    #[test]
    fn a_reset_is_not_left_on_disk_looking_like_a_running_timer() {
        let dir = TempDir::new().expect("tempdir");
        let path = state_path(dir.path());

        let mut t = timer(&dir);
        t.start();
        t.persist().expect("persist");
        t.clock.advance(Duration::from_secs(5));
        t.reset();

        assert!(t.persist_due(), "a reset must reach the disk");
        t.persist().expect("persist");

        // Long enough that a stale "running" record would have been invalidated.
        let restarted = FakeClock::new();
        restarted.advance_split(Duration::ZERO, Duration::from_secs(600));
        let (restored, transitions) = Timer::load(restarted, path, WORK, BREAK);

        assert_eq!(restored.state().run, RunState::Idle);
        assert!(
            transitions.is_empty(),
            "an idle timer cannot have been interrupted"
        );
    }
}
