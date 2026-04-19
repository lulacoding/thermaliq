/**
 * Pure event → toast payload mapper.
 * No side-effects; all I/O lives in useNotifications.ts.
 * Exported separately so it can be unit-tested without a DOM or Tauri runtime.
 */

import type { NotificationCategory } from "../hooks/useNotificationSettings";

export interface WsEvent {
  type?: string;
  notification?: { title?: string; message?: string; severity?: string };
  reason?: string;
  description?: string;
  profile_name?: string;
  [key: string]: unknown;
}

export interface ToastPayload {
  title: string;
  body: string;
  /** Maps to a NotificationCategory for pref-gating. */
  category: NotificationCategory;
  /** Whether the toast should stay until dismissed (persistent) or auto-dismiss. */
  persistent: boolean;
}

/**
 * Map a raw WebSocket event frame to a ToastPayload.
 * Returns null when the frame carries no actionable notification data.
 */
export function mapEventToToast(data: WsEvent): ToastPayload | null {
  // Explicit notification envelope takes precedence.
  if (data.notification) {
    const severity = data.notification.severity ?? "info";
    const category = severityToCategory(severity, data.type);
    return {
      title: data.notification.title ?? "ThermalIQ",
      body: data.notification.message ?? "",
      category,
      persistent: category === "alert",
    };
  }

  switch (data.type) {
    case "safety_emergency":
      return {
        title: "Thermal Alert",
        body: data.reason ?? "Critical temperature reached — all fans to 100%",
        category: "alert",
        persistent: true,
      };

    case "profile_switched":
      return {
        title: "Profile Changed",
        body: `Switched to ${data.profile_name ?? "new profile"}`,
        category: "profile_switch",
        persistent: false,
      };

    case "ai_decision":
    case "curve_changed":
      return {
        title: "AI Agent",
        body: (data.reason as string | undefined) ?? (data.description as string | undefined) ?? "Adjusted fan curves",
        category: "intervention",
        persistent: false,
      };

    case "agent_intervention":
      return {
        title: "Agent Intervention",
        body: (data.reason as string | undefined) ?? "Agent requests review",
        category: "intervention",
        persistent: true,
      };

    case "thermal_alert":
      return {
        title: "Thermal Alert",
        body: (data.reason as string | undefined) ?? "Temperature threshold exceeded",
        category: "alert",
        persistent: true,
      };

    default:
      return null;
  }
}

function severityToCategory(
  severity: string,
  type: string | undefined,
): NotificationCategory {
  if (type === "agent_intervention" || type === "ai_decision" || type === "curve_changed") {
    return "intervention";
  }
  if (type === "profile_switched") return "profile_switch";
  if (severity === "error" || severity === "warning") return "alert";
  return "intervention";
}
