# WabiCalendar

A local-first desktop app that puts a week calendar and a pomodoro timer side by
side, so you can see what you planned next to what you actually did.

Everything lives in plain files in a directory you choose. The app never talks to
the network — there is no account, no sync, and no server.

## The two halves

**Week calendar.** Drag on empty space to create a block, drag a block to move
it, drag its edge to change its length. Monday-start, week view only.

**Pomodoro timer.** Work and break lengths are yours to set. It runs in the tray
with a live countdown and keeps going whether or not a calendar is open.

They are independent: either one is useful on its own. What connects them is the
disk. Every finished pomodoro is recorded, and each day column in the week view
is split — planned blocks on the left, recorded sessions on the right — with a
summary across the top (planned total, focused total, completion rate). The
recorded half is read-only; it is a report, not something to drag around.

## Your data

Point the app at a directory — the *vault* — and it will keep this shape:

```
your-vault/
├── config.toml                 # schema version; yours to edit
├── calendar/
│   └── 2026-08.ics             # one iCalendar file per month
├── sessions/
│   └── 2026-08-16.jsonl        # one append-only file per day
└── .index/                     # derived cache, safe to delete
```

Two rules hold this together:

- **The files are the truth.** `.index/` is a cache. Delete the whole directory,
  restart, and nothing is lost — that is a tested invariant, not an aspiration.
- **Writes are atomic.** Every write goes through a staged temp file, an fsync
  (`F_FULLFSYNC` on macOS), and a rename. A crash mid-write leaves the previous
  version intact, never a half-written one.

The calendar files are standard iCalendar, parsed and written with the
`icalendar` crate — no hand-rolled RFC 5545. You can drop them straight into
Google Calendar or Apple Calendar, or edit them in a text editor. Recurring
events (`RRULE`) are expanded and drawn, but editing a recurrence rule from the
UI is refused on purpose; edit the `.ics` by hand.

Sessions are JSONL, one record per line:

```json
{"id":"...","kind":"work","planned_sec":1500,"actual_sec":1500,"started_at":"2026-08-16T14:00:00+09:00","ended_at":"2026-08-16T14:25:00+09:00","outcome":"completed","label":"thesis"}
```

`outcome` is `completed`, `aborted`, or `invalidated`. The last one means the
machine slept through the pomodoro — the timer holds a monotonic clock and a
wall clock at once and compares them, so moving the system clock, an NTP step,
or a suspend cannot quietly produce a bogus 25 minutes.

Which vault you picked is *not* stored in the vault, for the obvious reason. It
lives in `settings.toml` in the OS config directory, alongside `timer.json`, the
heartbeat that lets a running pomodoro survive a restart.

## System integration

Four optional pieces, all driven from Rust:

| | |
|---|---|
| Tray icon | Live countdown plus start/pause and reset |
| Notification | Fires when a work or break segment ends |
| Global shortcut | `Cmd/Ctrl+Shift+P` toggles the timer; rebindable, or off |
| Autostart | Launch at login |

Every one of them can fail and none of them is allowed to take the app down. A
tray that will not build, a hotkey another app already owns, a notification
permission the user refused, an autostart directory that is not writable — each
is reported in the settings dialog with a reason, and the app keeps running.

## Development

You will need a Rust toolchain, Node 20+, and your platform's
[Tauri prerequisites](https://tauri.app/start/prerequisites/).

```bash
npm install
npm run tauri dev       # starts Vite itself — do not run a second dev server
npm run tauri build
```

There is no `Cargo.toml` at the repository root, so Cargo commands need a
manifest path:

```bash
cargo test --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings

npm run typecheck       # vue-tsc; the frontend has no ESLint/Prettier
npm test                # vitest, over the pure functions in src/lib/
```

Layout: `src-tauri/src/` holds the vault, calendar, sessions, timer, and OS
integrations; `src/` holds the Vue frontend, with business logic in
`src/composables/` and the types shared with Rust hand-maintained in
`src/types/`. `CLAUDE.md` records the architecture invariants and the reasoning
behind the decisions already made — read it before changing anything structural.

### Not built yet

Watching the vault for outside edits (so hand-editing an `.ics` updates the
window live) is specified but unimplemented. Changes made behind the app's back
show up on the next restart.

## Deliberately out of scope for v1

Accounts, sync, or any network request. Day, month, or agenda views — week only.
A reminder system beyond the pomodoro's own notification. Tags, kanban, notes,
task dependencies. Themes. CRDTs or conflict merging. Creating or editing
recurring events from the UI.

## License

MIT — see [LICENSE](LICENSE).
