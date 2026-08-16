import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

import type { VaultStatus } from "../types/vault";

/**
 * What `useVault` hands back. Named so it can be passed down as one prop —
 * every call builds fresh refs, so two callers would drift apart.
 */
export type VaultController = ReturnType<typeof useVault>;

export function useVault() {
  const status = ref<VaultStatus | null>(null);
  const error = ref<string | null>(null);
  const busy = ref(false);

  async function attempt(op: () => Promise<VaultStatus | null>): Promise<void> {
    busy.value = true;
    error.value = null;
    try {
      const next = await op();
      if (next !== null) {
        status.value = next;
      }
    } catch (e) {
      error.value = String(e);
    } finally {
      busy.value = false;
    }
  }

  function refresh(): Promise<void> {
    return attempt(() => invoke<VaultStatus>("vault_status"));
  }

  /** Ask the user for a directory, then hand it to the backend to adopt. */
  function choose(): Promise<void> {
    return attempt(async () => {
      const picked = await open({
        directory: true,
        multiple: false,
        title: "Choose vault directory",
      });
      // Cancelled — leave the current status alone.
      if (typeof picked !== "string") {
        return null;
      }
      return invoke<VaultStatus>("set_vault", { path: picked });
    });
  }

  return { status, error, busy, refresh, choose };
}
