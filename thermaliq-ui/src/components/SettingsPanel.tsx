import { useState, useEffect } from "react";

interface Settings {
  provider: string;
  api_key: string;
  trust_level: string;
  guardrails: {
    min_fan_duty_percent: number;
    temp_kill_switch_celsius: number;
    change_cooldown_seconds: number;
  };
  telemetry_enabled: boolean;
}

const API_BASE = "http://127.0.0.1:7272";

export default function SettingsPanel() {
  const [settings, setSettings] = useState<Settings>({
    provider: "anthropic",
    api_key: "",
    trust_level: "approval",
    guardrails: {
      min_fan_duty_percent: 10,
      temp_kill_switch_celsius: 95,
      change_cooldown_seconds: 30,
    },
    telemetry_enabled: false,
  });

  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);
  const [apiToken, setApiToken] = useState<string | null>(null);

  // Load API token from storage
  useEffect(() => {
    const loadToken = async () => {
      try {
        // Try to get token from Tauri invoke or localStorage
        const token = localStorage.getItem("api_token");
        if (token) {
          setApiToken(token);
        }
      } catch (e) {
        console.error("Failed to load API token:", e);
      }
    };
    loadToken();
  }, []);

  // Load current settings on mount
  useEffect(() => {
    const loadSettings = async () => {
      try {
        setLoading(true);
        setError(null);

        const response = await fetch(`${API_BASE}/api/settings`, {
          headers: {
            "Content-Type": "application/json",
          },
        });

        if (!response.ok) {
          throw new Error(`Failed to load settings: ${response.statusText}`);
        }

        const data = await response.json();
        setSettings({
          provider: data.ai_provider || "anthropic",
          api_key: "", // Never load API key for security
          trust_level: data.trust_level || "approval",
          guardrails: {
            min_fan_duty_percent: data.guardrails?.min_fan_duty_percent || 10,
            temp_kill_switch_celsius: data.guardrails?.temp_kill_switch_celsius || 95,
            change_cooldown_seconds: data.guardrails?.change_cooldown_seconds || 30,
          },
          telemetry_enabled: data.telemetry_enabled || false,
        });
      } catch (err) {
        setError(err instanceof Error ? err.message : "Failed to load settings");
      } finally {
        setLoading(false);
      }
    };

    loadSettings();
  }, []);

  const handleProviderChange = (e: React.ChangeEvent<HTMLSelectElement>) => {
    setSettings({
      ...settings,
      provider: e.target.value,
    });
  };

  const handleApiKeyChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setSettings({
      ...settings,
      api_key: e.target.value,
    });
  };

  const handleTrustLevelChange = (e: React.ChangeEvent<HTMLSelectElement>) => {
    setSettings({
      ...settings,
      trust_level: e.target.value,
    });
  };

  const handleGuardrailChange = (field: keyof Settings["guardrails"], value: number) => {
    setSettings({
      ...settings,
      guardrails: {
        ...settings.guardrails,
        [field]: value,
      },
    });
  };

  const handleTelemetryChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setSettings({
      ...settings,
      telemetry_enabled: e.target.checked,
    });
  };

  const handleSave = async () => {
    try {
      setSaving(true);
      setError(null);
      setSuccess(null);

      // Validate API key requirement
      if (!settings.api_key && settings.provider !== "ollama") {
        setError("API key is required for non-local providers");
        setSaving(false);
        return;
      }

      const token = apiToken || localStorage.getItem("api_token");
      const headers: HeadersInit = {
        "Content-Type": "application/json",
      };
      if (token) {
        headers["Authorization"] = `Bearer ${token}`;
      }

      // Save AI provider if API key is provided
      if (settings.api_key && settings.provider !== "ollama") {
        const providerResponse = await fetch(`${API_BASE}/api/settings/ai-provider`, {
          method: "POST",
          headers,
          body: JSON.stringify({
            provider: settings.provider,
            api_key: settings.api_key,
          }),
        });

        if (!providerResponse.ok) {
          const errorData = await providerResponse.text();
          throw new Error(`Failed to save AI provider: ${errorData}`);
        }
      } else if (!settings.api_key && settings.provider !== "ollama") {
        throw new Error("API key is required for non-local providers");
      }

      // Save trust level
      const trustResponse = await fetch(`${API_BASE}/api/settings/trust-level`, {
        method: "POST",
        headers,
        body: JSON.stringify({
          trust_level: settings.trust_level,
        }),
      });

      if (!trustResponse.ok) {
        throw new Error("Failed to save trust level");
      }

      // Save guardrails
      const guardrailsResponse = await fetch(`${API_BASE}/api/settings/guardrails`, {
        method: "POST",
        headers,
        body: JSON.stringify({
          min_fan_duty_percent: settings.guardrails.min_fan_duty_percent,
          temp_kill_switch_celsius: settings.guardrails.temp_kill_switch_celsius,
          change_cooldown_seconds: settings.guardrails.change_cooldown_seconds,
        }),
      });

      if (!guardrailsResponse.ok) {
        throw new Error("Failed to save guardrails");
      }

      // Save telemetry
      const telemetryResponse = await fetch(`${API_BASE}/api/settings/telemetry`, {
        method: "POST",
        headers,
        body: JSON.stringify({
          enabled: settings.telemetry_enabled,
        }),
      });

      if (!telemetryResponse.ok) {
        throw new Error("Failed to save telemetry setting");
      }

      setSuccess("Settings saved successfully!");
      // Clear success message after 3 seconds
      setTimeout(() => setSuccess(null), 3000);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to save settings");
    } finally {
      setSaving(false);
    }
  };

  return (
    <div style={styles.container}>
      <div style={styles.header}>
        <h1 style={styles.title}>Settings</h1>
        <p style={styles.subtitle}>Configure API keys, trust levels, and safety guardrails</p>
      </div>

      {error && <div style={styles.errorBox}>{error}</div>}
      {success && <div style={styles.successBox}>{success}</div>}

      <div style={{...styles.section, ...styles.sectionCard}}>
        <div style={styles.cardAccentBar} />
        <h2 style={styles.sectionTitle}>AI Provider</h2>
        <div style={styles.settingGroup}>
          <label style={styles.label}>API Provider</label>
          <select
            style={styles.select}
            value={settings.provider}
            onChange={handleProviderChange}
            disabled={loading}
          >
            <option value="anthropic">Anthropic (Claude)</option>
            <option value="openai">OpenAI</option>
            <option value="gemini">Google Gemini</option>
            <option value="ollama">Ollama (Local)</option>
            <option value="openrouter">OpenRouter</option>
          </select>
        </div>
        {settings.provider !== "ollama" && (
          <div style={styles.settingGroup}>
            <label style={styles.label}>API Key</label>
            <input
              type="password"
              style={styles.input}
              placeholder="Enter your API key"
              value={settings.api_key}
              onChange={handleApiKeyChange}
              disabled={loading}
            />
            {!settings.api_key && settings.provider !== "ollama" && (
              <p style={styles.helperText}>API key required for this provider</p>
            )}
          </div>
        )}
      </div>

      <div style={{...styles.section, ...styles.sectionCard}}>
        <div style={{...styles.cardAccentBar, background: "var(--secondary)"}} />
        <h2 style={styles.sectionTitle}>Trust Level</h2>
        <div style={styles.settingGroup}>
          <label style={styles.label}>Agent Autonomy</label>
          <select
            style={styles.select}
            value={settings.trust_level}
            onChange={handleTrustLevelChange}
            disabled={loading}
          >
            <option value="passive">Passive (monitoring only)</option>
            <option value="approval">Approval Required (AI proposes, you approve)</option>
            <option value="autonomous">Full Auto (AI makes all decisions)</option>
          </select>
        </div>
      </div>

      <div style={{...styles.section, ...styles.sectionCard}}>
        <div style={styles.cardAccentBar} />
        <h2 style={styles.sectionTitle}>Safety Guardrails</h2>
        <div style={styles.settingGroup}>
          <label style={styles.label}>Minimum Fan Duty (%)</label>
          <input
            type="number"
            style={styles.input}
            min="0"
            max="100"
            value={settings.guardrails.min_fan_duty_percent}
            onChange={(e) =>
              handleGuardrailChange("min_fan_duty_percent", parseInt(e.target.value))
            }
            disabled={loading}
          />
          <p style={styles.helperText}>Fans will never go below this duty cycle</p>
        </div>
        <div style={styles.settingGroup}>
          <label style={styles.label}>Thermal Kill-Switch (°C)</label>
          <input
            type="number"
            style={styles.input}
            min="30"
            max="120"
            value={settings.guardrails.temp_kill_switch_celsius}
            onChange={(e) =>
              handleGuardrailChange("temp_kill_switch_celsius", parseInt(e.target.value))
            }
            disabled={loading}
          />
          <p style={styles.helperText}>Emergency: all fans to 100% if any sensor exceeds this</p>
        </div>
        <div style={styles.settingGroup}>
          <label style={styles.label}>Curve Change Cooldown (seconds)</label>
          <input
            type="number"
            style={styles.input}
            min="0"
            max="3600"
            value={settings.guardrails.change_cooldown_seconds}
            onChange={(e) =>
              handleGuardrailChange("change_cooldown_seconds", parseInt(e.target.value))
            }
            disabled={loading}
          />
          <p style={styles.helperText}>Minimum time between profile/curve changes</p>
        </div>
      </div>

      <div style={{...styles.section, ...styles.sectionCard}}>
        <div style={styles.cardAccentBar} />
        <h2 style={styles.sectionTitle}>General</h2>
        <div style={styles.checkboxGroup}>
          <input
            type="checkbox"
            id="telemetry"
            checked={settings.telemetry_enabled}
            onChange={handleTelemetryChange}
            disabled={loading}
          />
          <label htmlFor="telemetry" style={styles.checkboxLabel}>
            Send telemetry (helps improve AI recommendations)
          </label>
        </div>
      </div>

      <button
        style={{
          ...styles.button,
          ...(saving ? styles.buttonDisabled : {}),
        }}
        onClick={handleSave}
        disabled={saving || loading}
      >
        {saving ? "Saving..." : "Save Settings"}
      </button>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  container: {
    padding: "24px",
    overflow: "auto",
    height: "100%",
  },
  header: {
    marginBottom: 32,
  },
  title: {
    fontSize: 28,
    fontWeight: 700,
    color: "var(--text-primary)",
    margin: 0,
    marginBottom: 4,
  },
  subtitle: {
    fontSize: 13,
    color: "var(--text-secondary)",
    margin: 0,
  },
  section: {
    marginBottom: 32,
  },
  sectionCard: {
    background: "var(--bg-card)",
    border: "1px solid var(--border)",
    borderRadius: "var(--radius)",
    padding: 12,
    position: "relative",
    overflow: "hidden",
  },
  cardAccentBar: {
    position: "absolute",
    bottom: 0,
    left: 0,
    right: 0,
    height: 4,
    background: "var(--accent)",
  },
  sectionTitle: {
    fontSize: 11,
    fontWeight: 600,
    letterSpacing: "0.08em",
    textTransform: "uppercase",
    color: "var(--text-muted)",
    margin: "0 0 16px 0",
  },
  settingGroup: {
    marginBottom: 16,
  },
  label: {
    display: "block",
    fontSize: 13,
    color: "var(--text-primary)",
    marginBottom: 6,
    fontWeight: 500,
  },
  input: {
    width: "100%",
    padding: "8px 12px",
    background: "var(--bg-card)",
    border: "1px solid var(--border)",
    borderRadius: "var(--radius)",
    color: "var(--text-primary)",
    fontSize: 13,
    fontFamily: "inherit",
  },
  select: {
    width: "100%",
    padding: "8px 12px",
    background: "var(--bg-card)",
    border: "1px solid var(--border)",
    borderRadius: "var(--radius)",
    color: "var(--text-primary)",
    fontSize: 13,
    fontFamily: "inherit",
  },
  checkboxGroup: {
    display: "flex",
    alignItems: "center",
    gap: 10,
    marginBottom: 12,
  },
  checkboxLabel: {
    fontSize: 13,
    color: "var(--text-primary)",
    cursor: "pointer",
  },
  button: {
    padding: "8px 20px",
    background: "var(--accent)",
    color: "#fff",
    border: "none",
    borderRadius: "var(--radius-sm)",
    fontWeight: 600,
    fontSize: 13,
    cursor: "pointer",
    transition: "background 0.15s",
  },
  buttonDisabled: {
    opacity: 0.5,
    cursor: "not-allowed",
  },
  errorBox: {
    padding: "12px 16px",
    marginBottom: 24,
    background: "rgba(255, 87, 51, 0.1)",
    border: "1px solid rgba(255, 87, 51, 0.3)",
    borderRadius: "var(--radius)",
    color: "#FF5733",
    fontSize: 13,
    fontWeight: 500,
  },
  successBox: {
    padding: "12px 16px",
    marginBottom: 24,
    background: "rgba(0, 255, 65, 0.1)",
    border: "1px solid rgba(0, 255, 65, 0.3)",
    borderRadius: "var(--radius)",
    color: "#00FF41",
    fontSize: 13,
    fontWeight: 500,
  },
  helperText: {
    fontSize: 12,
    color: "var(--text-secondary)",
    marginTop: 4,
    margin: "4px 0 0 0",
  },
};
