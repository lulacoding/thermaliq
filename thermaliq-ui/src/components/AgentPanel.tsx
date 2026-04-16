import { useState, useEffect } from "react";
import { Zap } from "lucide-react";
import { useTranslation } from "react-i18next";

interface Intervention {
  id: string;
  description: string;
  proposed_at: number;
  proposed_action: string;
}

interface AgentStatusData {
  mode: "Passive" | "Approval" | "Autonomous";
  pending_count: number;
}

const API_BASE = "http://127.0.0.1:7272";

export default function AgentPanel() {
  const { t } = useTranslation();
  const [agentStatus, setAgentStatus] = useState<"idle" | "active" | "error">("active");
  const [agentMode, setAgentMode] = useState<"Passive" | "Approval" | "Autonomous">("Approval");
  const [decisionCount, setDecisionCount] = useState(0);
  const [lastAction, setLastAction] = useState(t("agent.waitingForIntervention"));
  const [error, setError] = useState<string | null>(null);

  // Fetch agent status and recent interventions on mount
  useEffect(() => {
    const fetchAgentData = async () => {
      try {
        setError(null);

        // Fetch agent status (mode and pending count)
        const statusRes = await fetch(`${API_BASE}/api/agent/status`);
        if (!statusRes.ok) throw new Error("Failed to fetch agent status");
        const statusData = (await statusRes.json()) as AgentStatusData;
        setAgentMode(statusData.mode);

        // Fetch recent interventions
        const interventionsRes = await fetch(`${API_BASE}/api/agent/interventions`);
        if (!interventionsRes.ok) throw new Error("Failed to fetch interventions");
        const interventionsData = (await interventionsRes.json()) as {
          interventions: Intervention[];
        };

        // Count total interventions made (approximation via pending count + interface history)
        setDecisionCount(interventionsData.interventions.length);

        // Extract last action from most recent intervention
        if (interventionsData.interventions.length > 0) {
          const latest = interventionsData.interventions[0];
          const timeDiff = Math.floor((Date.now() - latest.proposed_at) / 1000);
          const timeStr =
            timeDiff < 60
              ? `${timeDiff}s ago`
              : timeDiff < 3600
              ? `${Math.floor(timeDiff / 60)}m ago`
              : `${Math.floor(timeDiff / 3600)}h ago`;
          setLastAction(`${latest.description} • ${timeStr}`);
        }
      } catch (err) {
        console.error("Failed to fetch agent data:", err);
        setError(err instanceof Error ? err.message : "Failed to load agent data");
        setAgentStatus("error");
      }
    };

    fetchAgentData();
  }, []);

  return (
    <div style={styles.container}>
      <div style={styles.header}>
        <h1 style={styles.title}>{t("agent.title")}</h1>
        <p style={styles.subtitle}>{t("agent.subtitle")}</p>
      </div>

      {/* Agent Status */}
      <div style={{...styles.section, ...styles.sectionCard}}>
        <div style={styles.cardAccentBar} />
        <h2 style={styles.sectionTitle}>{t("agent.status")}</h2>
        {error ? (
          <p style={{color: "var(--red)", fontSize: 13}}>{error}</p>
        ) : (
          <div style={styles.statusContent}>
            <div style={styles.statusIndicator}>
              <div style={{...styles.statusDot, ...styles[`status${agentStatus}`]}} />
              <span style={styles.statusText}>
                {agentStatus === "active" && t("agent.statusActive")}
                {agentStatus === "idle" && t("agent.statusIdle")}
                {agentStatus === "error" && t("agent.statusError")}
              </span>
            </div>
            <p style={styles.statusDetail}>
              {agentStatus === "active" && t("agent.statusDetailActive")}
              {agentStatus === "idle" && t("agent.statusDetailIdle")}
              {agentStatus === "error" && t("agent.statusDetailError")}
            </p>
          </div>
        )}
      </div>

      {/* Recent Actions */}
      <div style={{...styles.section, ...styles.sectionCard}}>
        <div style={styles.cardAccentBar} />
        <h2 style={styles.sectionTitle}>{t("agent.recentActions")}</h2>
        <div style={styles.actionList}>
          <div style={styles.actionItem}>
            <Zap size={14} color="var(--accent)" style={{ marginRight: 8 }} />
            <span style={styles.actionText}>{lastAction}</span>
          </div>
          <div style={styles.actionMeta}>
            <span className="mono">{decisionCount} {t("agent.decisionsThisSession")}</span>
          </div>
        </div>
      </div>

      {/* Controls */}
      <div style={{...styles.section, ...styles.sectionCard}}>
        <div style={styles.cardAccentBar} />
        <h2 style={styles.sectionTitle}>{t("agent.controls")}</h2>
        <div style={styles.controlGroup}>
          <button style={styles.button}>{t("agent.pauseAgent")}</button>
          <button style={{...styles.button, background: "var(--secondary)"}}>{t("agent.viewLogs")}</button>
        </div>
      </div>

      {/* Trust Level Info */}
      <div style={{...styles.section, ...styles.sectionCard}}>
        <div style={{...styles.cardAccentBar, background: 'var(--secondary)'}} />
        <h2 style={styles.sectionTitle}>{t("agent.trustLevel")}</h2>
        <p style={styles.trustText}>
          {t("agent.mode")}: <span style={{color: 'var(--accent)', fontWeight: 600}}>{agentMode}</span>
        </p>
        <p style={styles.trustDescription}>
          {t("settings.guardrails")}
        </p>
      </div>
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
  statusContent: {
    display: "flex",
    flexDirection: "column",
    gap: 12,
  },
  statusIndicator: {
    display: "flex",
    alignItems: "center",
    gap: 8,
  },
  statusDot: {
    width: 8,
    height: 8,
    borderRadius: "50%",
    animation: "pulse 1.5s ease-in-out infinite",
  },
  statusactive: {
    background: "var(--accent)",
  },
  statusidle: {
    background: "var(--text-muted)",
  },
  statuserror: {
    background: "var(--red)",
  },
  statusText: {
    fontSize: 13,
    fontWeight: 600,
    color: "var(--text-primary)",
  },
  statusDetail: {
    fontSize: 12,
    color: "var(--text-secondary)",
    margin: 0,
  },
  actionList: {
    display: "flex",
    flexDirection: "column",
    gap: 12,
  },
  actionItem: {
    display: "flex",
    alignItems: "center",
    fontSize: 13,
    color: "var(--text-primary)",
  },
  actionText: {
    flex: 1,
  },
  actionMeta: {
    fontSize: 11,
    color: "var(--text-muted)",
    fontFamily: "var(--font-mono)",
  },
  controlGroup: {
    display: "flex",
    gap: 8,
  },
  button: {
    flex: 1,
    padding: "8px 16px",
    background: "var(--accent)",
    color: "#fff",
    border: "none",
    borderRadius: "var(--radius-sm)",
    fontWeight: 600,
    fontSize: 13,
    cursor: "pointer",
    transition: "background 0.15s",
  },
  trustText: {
    fontSize: 13,
    color: "var(--text-primary)",
    margin: "0 0 8px 0",
  },
  trustDescription: {
    fontSize: 12,
    color: "var(--text-secondary)",
    margin: 0,
    lineHeight: 1.5,
  },
};
