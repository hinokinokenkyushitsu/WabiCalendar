//! The CLI half of CalenPomo.
//!
//! Deliberately thin: everything it does lives in `calenpomo_lib::cli`, so that
//! the commands are reachable from the test suite without going through a
//! process boundary.

use std::process::ExitCode;

fn main() -> ExitCode {
    calenpomo_lib::cli::main()
}
