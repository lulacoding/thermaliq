import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { shouldCheck, recordCheck, isNewerVersion, THROTTLE_KEY } from "./useUpdater";

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

// ── isNewerVersion ────────────────────────────────────────────────────────────

describe("isNewerVersion", () => {
  it("returns true when candidate has higher major", () => {
    expect(isNewerVersion("1.0.0", "2.0.0")).toBe(true);
  });

  it("returns true when candidate has higher minor", () => {
    expect(isNewerVersion("1.0.0", "1.1.0")).toBe(true);
  });

  it("returns true when candidate has higher patch", () => {
    expect(isNewerVersion("1.0.0", "1.0.1")).toBe(true);
  });

  it("returns false for identical versions", () => {
    expect(isNewerVersion("1.2.3", "1.2.3")).toBe(false);
  });

  it("returns false when candidate is older (major)", () => {
    expect(isNewerVersion("2.0.0", "1.9.9")).toBe(false);
  });

  it("returns false when candidate is older (minor)", () => {
    expect(isNewerVersion("1.5.0", "1.4.9")).toBe(false);
  });

  it("returns false when candidate is older (patch)", () => {
    expect(isNewerVersion("1.0.5", "1.0.4")).toBe(false);
  });

  it("handles v-prefixed strings", () => {
    expect(isNewerVersion("v1.0.0", "v1.0.1")).toBe(true);
    expect(isNewerVersion("v1.0.1", "v1.0.0")).toBe(false);
  });

  it("handles two-part versions gracefully", () => {
    expect(isNewerVersion("1.0", "1.1")).toBe(true);
    expect(isNewerVersion("2.0", "1.9")).toBe(false);
  });
});
