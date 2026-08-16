/**
 * Mirrors `FeatureStatus` / `IntegrationStatus` in
 * `src-tauri/src/integrations/mod.rs` and `IntegrationReport` /
 * `CountdownSupport` in `src-tauri/src/commands.rs`.
 * Maintained by hand — change both sides together.
 */

export type FeatureStatus =
  | { state: "ready" }
  /** The user switched it off. Not a failure. */
  | { state: "off" }
  /** This platform or display server cannot do it at all; retrying will not help. */
  | { state: "unsupported"; reason: string }
  /** The OS or the user said no, or something else owns it. Worth a retry. */
  | { state: "denied"; reason: string };

export interface IntegrationStatus {
  tray: FeatureStatus;
  shortcut: FeatureStatus;
  notification: FeatureStatus;
  autostart: FeatureStatus;
}

/** Where the countdown is actually visible on this platform. */
export interface CountdownSupport {
  /** Text beside the tray icon. Unsupported on Windows. */
  title: boolean;
  /** Hover tooltip. Unsupported on Linux. */
  tooltip: boolean;
  /** The disabled first menu entry — the one that works everywhere. */
  menuItem: boolean;
}

export interface IntegrationReport {
  status: IntegrationStatus;
  shortcutToggle: string | null;
  autostart: boolean;
  workSecs: number;
  breakSecs: number;
  countdown: CountdownSupport;
}
