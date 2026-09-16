//! The CLI half of WabiCalendar.
//!
//! Deliberately thin: everything it does lives in `wabicalendar_lib::cli`, so
//! that the commands are reachable from the test suite without going through a
//! process boundary.

use std::process::ExitCode;

fn main() -> ExitCode {
    wabicalendar_lib::cli::main()
}
