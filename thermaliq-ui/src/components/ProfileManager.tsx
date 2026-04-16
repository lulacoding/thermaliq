import { useEffect, useState, useCallback } from "react";
import { Layers, CheckCircle } from "lucide-react";
import { useToken } from "../hooks/useToken";
import { useTranslation } from "react-i18next";

const BASE = "http://127.0.0.1:7272";

const PROFILE_DESCRIPTIONS: Record<string, string> = {
  balanced: "Balanced performance and acoustics for everyday use.",
  performance: "Maximum cooling for sustained high workloads.",
  silent: "Minimal fan noise — prioritises acoustics over thermals.",
  gaming: "Aggressive cooling tuned for sustained GPU and CPU loads.",
  efficiency: "Power-optimised — reduces fan speed at idle.",
};

export default function ProfileManager() {
  const { t } = useTranslation();
  const { token } = useToken();

  const [profiles, setProfiles] = useState<string[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [applying, setApplying] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const fetchProfiles = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [profRes, activeRes] = await Promise.all([
        fetch(`${BASE}/api/profiles`),
        fetch(`${BASE}/api/profiles/active`),
      ]);
      if (!profRes.ok) throw new Error(`Profiles: HTTP ${profRes.status}`);
      if (!activeRes.ok) throw new Error(`Active profile: HTTP ${activeRes.status}`);
      const profData = (await profRes.json()) as { profiles: string[] };
      const activeData = (await activeRes.json()) as { name: string };
      setProfiles(profData.profiles ?? []);
      setActive(activeData.name ?? null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void fetchProfiles();
  }, [fetchProfiles]);

  const applyProfile = useCallback(async (name: string) => {
    if (!token) { setError("No API token loaded — cannot apply profile"); return; }
    if (applying) return;
    setApplying(name);
    setError(null);
    try {
      const res = await fetch(`${BASE}/api/profiles/active`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${token}`,
        },
        body: JSON.stringify({ name }),
      });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      setActive(name);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setApplying(null);
    }
  }, [token, applying]);

  return (
    <div style={styles.root}>
      <div style={styles.header}>
        <h1 style={styles.title}>{t("profiles.title")}</h1>
        <button
          className="btn-ghost"
          onClick={() => void fetchProfiles()}
          disabled={loading}
          style={{ fontSize: 12 }}
        >
          {t("profiles.refresh")}
        </button>
      </div>

      {error && (
        <div style={styles.errorBanner} className="error-text">
          {error}
        </div>
      )}

      {loading && (
        <p style={styles.loadingText} className="loading-pulse">
          {t("profiles.loading")}
        </p>
      )}

      {!loading && (
        <div style={styles.grid}>
          {profiles.map((name) => {
            const isActive = name === active;
            const isApplying = applying === name;
            return (
              <button
                key={name}
                onClick={() => void applyProfile(name)}
                disabled={isActive || isApplying || !token}
                style={{
                  ...styles.profileCard,
                  ...(isActive ? styles.profileCardActive : {}),
                }}
              >
                <div style={styles.profileCardAccentBar} />
                <div style={styles.profileTop}>
                  <div style={styles.profileIconWrap}>
                    <Layers
                      size={18}
                      color={isActive ? "var(--accent)" : "var(--text-muted)"}
                    />
                  </div>
                  {isActive && (
                    <span className="badge badge-blue">
                      <CheckCircle size={9} />
                      {t("profiles.active")}
                    </span>
                  )}
                  {isApplying && (
                    <span className="badge badge-gray loading-pulse">
                      {t("profiles.applying")}
                    </span>
                  )}
                </div>
                <div style={styles.profileName}>
                  {name.charAt(0).toUpperCase() + name.slice(1)}
                </div>
                <div style={styles.profileDesc}>
                  {PROFILE_DESCRIPTIONS[name] ?? "Custom thermal profile."}
                </div>
              </button>
            );
          })}
        </div>
      )}

      {!token && !loading && (
        <div style={styles.tokenWarning}>
          <p style={styles.tokenWarningText}>
            {t("profiles.tokenMissing")}{" "}
            <span className="mono">~/.config/thermaliq/api.token</span>.
            {t("profiles.switchingDisabled")}
          </p>
        </div>
      )}
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  root: {
    padding: 24,
    height: "100%",
    overflowY: "auto",
  },
  header: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    marginBottom: 24,
  },
  title: {
    fontSize: 20,
    fontWeight: 700,
    color: "var(--text-primary)",
  },
  errorBanner: {
    background: "var(--red-dim)",
    border: "1px solid rgba(239,68,68,0.3)",
    borderRadius: "var(--radius-sm)",
    padding: "8px 12px",
    marginBottom: 16,
    fontSize: 12,
  },
  loadingText: {
    color: "var(--text-muted)",
    fontSize: 13,
  },
  grid: {
    display: "grid",
    gridTemplateColumns: "repeat(auto-fill, minmax(220px, 1fr))",
    gap: 12,
  },
  profileCard: {
    background: "var(--bg-card)",
    border: "1px solid var(--border)",
    borderRadius: "var(--radius)",
    padding: "16px",
    display: "flex",
    flexDirection: "column",
    gap: 10,
    textAlign: "left",
    cursor: "pointer",
    transition: "border-color 0.15s, background 0.15s",
    position: "relative",
    overflow: "hidden",
  },
  profileCardAccentBar: {
    position: "absolute",
    bottom: 0,
    left: 0,
    right: 0,
    height: 4,
    background: "var(--accent)",
  },
  profileCardActive: {
    borderColor: "var(--accent)",
    background: "var(--accent-dim)",
  },
  profileTop: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
  },
  profileIconWrap: {
    display: "flex",
    alignItems: "center",
  },
  profileName: {
    fontSize: 15,
    fontWeight: 700,
    color: "var(--text-primary)",
  },
  profileDesc: {
    fontSize: 12,
    color: "var(--text-secondary)",
    lineHeight: 1.5,
  },
  tokenWarning: {
    marginTop: 20,
    background: "var(--yellow-dim)",
    border: "1px solid rgba(234,179,8,0.3)",
    borderRadius: "var(--radius-sm)",
    padding: "10px 14px",
  },
  tokenWarningText: {
    color: "var(--yellow)",
    fontSize: 12,
  },
};
