import { describe, it, expect } from "vitest";
import { mapEventToToast } from "./notificationMapper";

describe("mapEventToToast", () => {
  it("returns null for unknown event types with no notification envelope", () => {
    expect(mapEventToToast({ type: "heartbeat" })).toBeNull();
    expect(mapEventToToast({})).toBeNull();
  });

  describe("safety_emergency", () => {
    it("maps to alert category, persistent, with fallback body", () => {
      const result = mapEventToToast({ type: "safety_emergency" });
      expect(result).not.toBeNull();
      expect(result!.category).toBe("alert");
      expect(result!.persistent).toBe(true);
      expect(result!.title).toBe("Thermal Alert");
      expect(result!.body).toContain("100%");
    });

    it("uses reason field when present", () => {
      const result = mapEventToToast({ type: "safety_emergency", reason: "CPU at 98°C" });
      expect(result!.body).toBe("CPU at 98°C");
    });
  });

  describe("profile_switched", () => {
    it("maps to profile_switch category, non-persistent", () => {
      const result = mapEventToToast({ type: "profile_switched", profile_name: "Gaming" });
      expect(result).not.toBeNull();
      expect(result!.category).toBe("profile_switch");
      expect(result!.persistent).toBe(false);
      expect(result!.body).toContain("Gaming");
    });

    it("falls back to generic text when profile_name missing", () => {
      const result = mapEventToToast({ type: "profile_switched" });
      expect(result!.body).toContain("new profile");
    });
  });

  describe("ai_decision / curve_changed", () => {
    it("maps to intervention category, non-persistent", () => {
      const result = mapEventToToast({ type: "ai_decision", reason: "Temp spike" });
      expect(result!.category).toBe("intervention");
      expect(result!.persistent).toBe(false);
      expect(result!.body).toBe("Temp spike");
    });

    it("falls back to description field", () => {
      const result = mapEventToToast({ type: "curve_changed", description: "Adjusted curves" });
      expect(result!.body).toBe("Adjusted curves");
    });

    it("uses default body when no reason or description", () => {
      const result = mapEventToToast({ type: "ai_decision" });
      expect(result!.body).toBe("Adjusted fan curves");
    });
  });

  describe("agent_intervention", () => {
    it("maps to intervention category, persistent", () => {
      const result = mapEventToToast({ type: "agent_intervention", reason: "Review needed" });
      expect(result!.category).toBe("intervention");
      expect(result!.persistent).toBe(true);
      expect(result!.body).toBe("Review needed");
    });
  });

  describe("thermal_alert", () => {
    it("maps to alert category, persistent", () => {
      const result = mapEventToToast({ type: "thermal_alert", reason: "GPU over threshold" });
      expect(result!.category).toBe("alert");
      expect(result!.persistent).toBe(true);
      expect(result!.body).toBe("GPU over threshold");
    });
  });

  describe("notification envelope", () => {
    it("uses envelope data over event type", () => {
      const result = mapEventToToast({
        type: "something_else",
        notification: { title: "Custom Title", message: "Custom body", severity: "error" },
      });
      expect(result).not.toBeNull();
      expect(result!.title).toBe("Custom Title");
      expect(result!.body).toBe("Custom body");
      expect(result!.category).toBe("alert");
      expect(result!.persistent).toBe(true);
    });

    it("defaults title to ThermalIQ when missing", () => {
      const result = mapEventToToast({
        notification: { message: "Hello" },
      });
      expect(result!.title).toBe("ThermalIQ");
    });

    it("info severity → intervention category, non-persistent", () => {
      const result = mapEventToToast({
        notification: { title: "Info", message: "FYI", severity: "info" },
      });
      expect(result!.category).toBe("intervention");
      expect(result!.persistent).toBe(false);
    });
  });
});
