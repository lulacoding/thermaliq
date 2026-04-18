import { useEffect, useRef } from "react";
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { mapEventToToast } from "../lib/notificationMapper";
import type { NotificationSettings } from "./useNotificationSettings";

const WS_AGENT_URL = "ws://127.0.0.1:7272/ws/agent";

/** Shared permission state — checked once per app lifetime. */
let permissionChecked = false;
let permissionGranted = false;

async function ensurePermission(): Promise<boolean> {
  if (permissionChecked) return permissionGranted;
  permissionChecked = true;
  try {
    permissionGranted = await isPermissionGranted();
    if (!permissionGranted) {
      const result = await requestPermission();
      permissionGranted = result === "granted";
    }
  } catch {
    permissionGranted = false;
  }
  return permissionGranted;
}

/**
 * Dispatch a single OS toast.  Falls back to the HTML Notification API when
 * Tauri is not available (browser dev mode).
 */
async function dispatchToast(title: string, body: string): Promise<void> {
  try {
    const granted = await ensurePermission();
    if (!granted) return;
    sendNotification({ title, body });
    return;
  } catch {
    // Tauri not available — fall through to HTML Notification API
  }

  if ("Notification" in window && Notification.permission === "granted") {
    new Notification(title, { body });
  }
}

/**
 * Hook that listens to daemon agent events and displays both an in-app banner
 * (handled by the WS message consumers) and an OS toast (handled here).
 *
 * Accepts optional notification settings so categories can be toggled by the
 * user.  When settings are undefined all categories are shown.
 */
export function useNotifications(settings?: NotificationSettings) {
  const wsRef = useRef<WebSocket | null>(null);
  const unmountedRef = useRef(false);
  const connectTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Keep a ref so the WS callback always sees the latest prefs without needing
  // to be recreated.
  const settingsRef = useRef<NotificationSettings | undefined>(settings);
  settingsRef.current = settings;

  const connect = () => {
    if (unmountedRef.current) return;
    if (wsRef.current?.readyState === WebSocket.OPEN) return;

    const ws = new WebSocket(WS_AGENT_URL);
    wsRef.current = ws;

    ws.onopen = () => {
      if (unmountedRef.current) ws.close();
    };

    ws.onmessage = (event: MessageEvent<string>) => {
      try {
        const data = JSON.parse(event.data);
        const toast = mapEventToToast(data);
        if (!toast) return;

        const prefs = settingsRef.current;
        if (prefs !== undefined) {
          if (!prefs.enabled) return;
          if (!prefs.categories[toast.category]) return;
        }

        dispatchToast(toast.title, toast.body);
      } catch {
        // Malformed frame — ignore
      }
    };

    ws.onclose = () => {
      if (!unmountedRef.current) {
        connectTimeoutRef.current = setTimeout(connect, 5000);
      }
    };

    ws.onerror = () => {
      // Let onclose handle reconnection
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
}
