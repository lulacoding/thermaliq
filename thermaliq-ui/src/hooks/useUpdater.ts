import { useState, useCallback, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

export const THROTTLE_KEY = "thermaliq_last_update_check";
const THROTTLE_MS = 24 * 60 * 60 * 1000; // 24 hours

export type UpdaterError =
  | { kind: "network" }
  | { kind: "signature" }
  | { kind: "permissions" }
  | { kind: "unknown"; message: string };

export interface UpdateInfo {
  version: string;
  body: string | null;
}

export interface UseUpdaterReturn {
  updateAvailable: UpdateInfo | null;
  checking: boolean;
  installing: boolean;
  error: UpdaterError | null;
  checkNow: () => Promise<void>;
  install: () => Promise<void>;
  dismiss: () => void;
}

/** Returns true if 24 h have elapsed since last check (or never checked). */
export function shouldCheck(): boolean {
  const raw = localStorage.getItem(THROTTLE_KEY);
  if (!raw) return true;
  const last = parseInt(raw, 10);
  if (Number.isNaN(last)) return true;
  return Date.now() - last >= THROTTLE_MS;
}

/** Record the current timestamp as the last check time. */
export function recordCheck(): void {
  localStorage.setItem(THROTTLE_KEY, String(Date.now()));
}

/**
 * Compare two semver strings.
 * Returns true when `candidate` is strictly greater than `current`.
 */
export function isNewerVersion(current: string, candidate: string): boolean {
  const parse = (v: string) =>
    v
      .replace(/^v/, "")
      .split(".")
      .map((n) => parseInt(n, 10) || 0);
  const [cMaj, cMin, cPat] = parse(current);
  const [nMaj, nMin, nPat] = parse(candidate);
  if (nMaj !== cMaj) return nMaj > cMaj;
  if (nMin !== cMin) return nMin > cMin;
  return nPat > cPat;
}

function classifyError(e: unknown): UpdaterError {
  const msg = String(e).toLowerCase();
  if (
    msg.includes("network") ||
    msg.includes("connection") ||
    msg.includes("timeout") ||
    msg.includes("fetch")
  ) {
    return { kind: "network" };
  }
  if (msg.includes("signature") || msg.includes("verif")) {
    return { kind: "signature" };
  }
  if (msg.includes("permission") || msg.includes("privileged") || msg.includes("admin")) {
    return { kind: "permissions" };
  }
  return { kind: "unknown", message: String(e) };
}

export function useUpdater(): UseUpdaterReturn {
  const [updateAvailable, setUpdateAvailable] = useState<UpdateInfo | null>(null);
  const [checking, setChecking] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [error, setError] = useState<UpdaterError | null>(null);

  const checkNow = useCallback(async () => {
    setChecking(true);
    setError(null);
    try {
      recordCheck();
      // Delegates to the Rust `check_for_updates` command which calls the
      // tauri-plugin-updater. It returns { available: bool, version: string,
      // body: string | null } or throws on error.
      const result = await invoke<{ available: boolean; version: string; body: string | null }>(
        "check_update_info"
      );
      if (result.available) {
        setUpdateAvailable({ version: result.version, body: result.body });
      } else {
        setUpdateAvailable(null);
      }
    } catch (e) {
      const classified = classifyError(e);
      // Network failures are silent (retry next launch); others surface.
      if (classified.kind !== "network") {
        setError(classified);
      }
    } finally {
      setChecking(false);
    }
  }, []);

  const install = useCallback(async () => {
    setInstalling(true);
    setError(null);
    try {
      await invoke("install_update");
      // Plugin restarts the app after successful install; if we reach here
      // something went wrong.
    } catch (e) {
      setError(classifyError(e));
      setInstalling(false);
    }
  }, []);

  const dismiss = useCallback(() => {
    setUpdateAvailable(null);
    setError(null);
  }, []);

  // On mount: throttled startup check
  useEffect(() => {
    if (shouldCheck()) {
      checkNow();
    }
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  return { updateAvailable, checking, installing, error, checkNow, dismiss, install };
}
