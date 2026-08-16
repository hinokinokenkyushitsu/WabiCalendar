/**
 * Mirrors `VaultStatus` in `src-tauri/src/commands.rs`.
 * Maintained by hand — change both sides together.
 */
export type VaultStatus =
  | { state: "unconfigured" }
  | { state: "missing"; path: string }
  | { state: "ready"; path: string; rebuilt: string[] };
