import { describe, it, expect, beforeEach } from "vitest";
import { shouldCheck, recordCheck, THROTTLE_KEY } from "./useUpdater";

// ── shouldCheck / recordCheck ─────────────────────────────────────────────────

describe("shouldCheck", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("returns true when key is absent", () => {
    expect(shouldCheck()).toBe(true);
  });

  it("returns true when stored value is not a number", () => {
    localStorage.setItem(THROTTLE_KEY, "not-a-number");
    expect(shouldCheck()).toBe(true);
  });

  it("returns true when 24 h have elapsed", () => {
    const past = Date.now() - 25 * 60 * 60 * 1000; // 25 h ago
    localStorage.setItem(THROTTLE_KEY, String(past));
    expect(shouldCheck()).toBe(true);
  });

  it("returns false when checked less than 24 h ago", () => {
    const recent = Date.now() - 60 * 1000; // 1 min ago
    localStorage.setItem(THROTTLE_KEY, String(recent));
    expect(shouldCheck()).toBe(false);
  });
});

describe("recordCheck", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("writes a numeric timestamp to localStorage", () => {
    const before = Date.now();
    recordCheck();
    const after = Date.now();
    const stored = parseInt(localStorage.getItem(THROTTLE_KEY) ?? "0", 10);
    expect(stored).toBeGreaterThanOrEqual(before);
    expect(stored).toBeLessThanOrEqual(after);
  });

  it("makes shouldCheck return false immediately after recording", () => {
    recordCheck();
    expect(shouldCheck()).toBe(false);
  });
});

// Semver comparison is delegated to the Tauri updater plugin (Rust side),
// which performs the canonical check against the signed `latest.json`.
// No client-side re-implementation is needed.
