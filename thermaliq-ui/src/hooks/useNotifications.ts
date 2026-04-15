import { useEffect, useRef } from "react";

interface NotificationEvent {
  type: "safety_emergency" | "profile_switched" | "curve_changed" | "ai_decision" | "thermal_alert";
  title: string;
  message: string;
  severity?: "info" | "warning" | "error";
}

const WS_AGENT_URL = "ws://127.0.0.1:7272/ws/agent";

/**
 * Hook that listens to daemon agent events and displays notifications.
 * Calls Tauri's native notification API on Windows, falls back to HTML notifications.
 */
export function useNotifications() {
  const wsRef = useRef<WebSocket | null>(null);
  const unmountedRef = useRef(false);
  const connectTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const showNotification = async (event: NotificationEvent) => {
    try {
      // Try Tauri notification API first (Windows native toast)
      const { sendNotification } = await import("@tauri-apps/plugin-notification");
      if (sendNotification) {
        sendNotification({
          title: event.title,
          body: event.message,
        });
        return;
      }
    } catch (e) {
      // Tauri not available, fall through to HTML notification
    }

    // Fallback to HTML Notification API
    if ("Notification" in window && Notification.permission === "granted") {
      new Notification(event.title, {
        body: event.message,
        tag: event.type,
      });
    }
  };

  const connect = () => {
    if (unmountedRef.current) return;
    if (wsRef.current?.readyState === WebSocket.OPEN) return; // Already connected

    const ws = new WebSocket(WS_AGENT_URL);
    wsRef.current = ws;

    ws.onopen = () => {
      if (unmountedRef.current) {
        ws.close();
        return;
      }
      // Connected successfully
    };

    ws.onmessage = (event: MessageEvent<string>) => {
      try {
        const data = JSON.parse(event.data);

        // Handle notification events from daemon
        if (data.notification) {
          const notif: NotificationEvent = {
            type: data.type || "ai_decision",
            title: data.notification.title || "ThermalIQ",
            message: data.notification.message || "",
            severity: data.notification.severity || "info",
          };
          showNotification(notif);
        }

        // Also handle specific event types
        if (data.type === "safety_emergency") {
          showNotification({
            type: "safety_emergency",
            title: "🚨 Thermal Alert",
            message: data.reason || "Critical temperature reached — all fans to 100%",
            severity: "error",
          });
        }

        if (data.type === "profile_switched") {
          showNotification({
            type: "profile_switched",
            title: "Profile Changed",
            message: `Switched to ${data.profile_name || "new profile"}`,
            severity: "info",
          });
        }

        if (data.type === "ai_decision" || data.type === "curve_changed") {
          showNotification({
            type: data.type,
            title: "AI Agent",
            message: data.reason || data.description || "Adjusted fan curves",
            severity: "info",
          });
        }
      } catch (e) {
        // Malformed frame — ignore
      }
    };

    ws.onclose = () => {
      if (!unmountedRef.current) {
        // Reconnect after delay (handles React.StrictMode double-mount)
        connectTimeoutRef.current = setTimeout(connect, 5000);
      }
    };

    ws.onerror = () => {
      // Don't close immediately; let onclose handler manage reconnection
      // This prevents error spam during development
    };
  };

  useEffect(() => {
    unmountedRef.current = false;
    connect();

    return () => {
      unmountedRef.current = true;
      wsRef.current?.close();
      if (connectTimeoutRef.current) {
        clearTimeout(connectTimeoutRef.current);
      }
    };
  }, []);
}
