import { useState, useCallback } from "react";

export type NotificationCategory = "intervention" | "profile_switch" | "alert";

export interface NotificationSettings {
  enabled: boolean;
  categories: Record<NotificationCategory, boolean>;
}

const STORAGE_KEY = "thermaliq_notification_settings";

const DEFAULT_SETTINGS: NotificationSettings = {
  enabled: true,
  categories: {
    intervention: true,
    profile_switch: true,
    alert: true,
  },
};

function loadSettings(): NotificationSettings {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULT_SETTINGS;
    const parsed = JSON.parse(raw) as Partial<NotificationSettings>;
    return {
      enabled: parsed.enabled ?? DEFAULT_SETTINGS.enabled,
      categories: {
        intervention: parsed.categories?.intervention ?? DEFAULT_SETTINGS.categories.intervention,
        profile_switch: parsed.categories?.profile_switch ?? DEFAULT_SETTINGS.categories.profile_switch,
        alert: parsed.categories?.alert ?? DEFAULT_SETTINGS.categories.alert,
      },
    };
  } catch {
    return DEFAULT_SETTINGS;
  }
}

function saveSettings(settings: NotificationSettings): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(settings));
  } catch {
    // localStorage unavailable (e.g. private mode) — ignore silently
  }
}

/**
 * Hook that provides notification preference state backed by localStorage.
 * Changes are persisted immediately on every update.
 */
export function useNotificationSettings() {
  const [settings, setSettings] = useState<NotificationSettings>(loadSettings);

  const setEnabled = useCallback((enabled: boolean) => {
    setSettings((prev) => {
      const next = { ...prev, enabled };
      saveSettings(next);
      return next;
    });
  }, []);

  const setCategoryEnabled = useCallback(
    (category: NotificationCategory, enabled: boolean) => {
      setSettings((prev) => {
        const next: NotificationSettings = {
          ...prev,
          categories: { ...prev.categories, [category]: enabled },
        };
        saveSettings(next);
        return next;
      });
    },
    [],
  );

  return { settings, setEnabled, setCategoryEnabled };
}
