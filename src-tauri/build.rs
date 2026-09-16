fn main() {
    // Only the app needs Tauri's codegen. Guarding it is what allows
    // `--no-default-features` to build the `wabi` binary without `tauri-build`
    // -- and therefore without Tauri's system dependencies -- in the way.
    #[cfg(feature = "gui")]
    tauri_build::build()
}
