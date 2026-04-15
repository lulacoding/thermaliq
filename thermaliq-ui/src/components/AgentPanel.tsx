import { useEffect, useState, useCallback } from "react";
import { Bot, CheckCircle, XCircle, RefreshCw } from "lucide-react";
import { useAgent, type AgentMode } from "../hooks/useAgent";
import { useToken } from "../hooks/useToken";

const BASE = "http://127.0.0.1:7272";

interface Intervention {
  id: string;
  description: string;
  proposed_action: string;
  proposed_at: string;
}

function modeBadgeClass(mode: AgentMode | null): string {
  if (mode === "autonomous") return "badge badge-red";
  if (mode === "approval") return "badge badge-yellow";
  return "badge badge-gray";
}

function modeLabel(mode: AgentMode | null): string {
  if (!mode) return "Unknown";
  return mode.charAt(0).toUpperCase() + mode.slice(1);
}

export default function AgentPanel() {
  const { mode, pendingCount, events } = useAgent();
  const { token } = useToken();

  const [interventions, setInterventions] = useState<Intervention[]>([]);
  const [loadingInterventions, setLoadingInterventions] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [settingMode, setSettingMode] = useState(false);

  const authHeaders = useCallback((): HeadersInit => ({
    "Content-Type": "application/json",
    ...(token ? { Authorization: `Bearer ${token}` } : {}),
  }), [token]);

  const fetchInterventions = useCallback(async () => {
    setLoadingInterventions(true);
    try {
      const res = await fetch(`${BASE}/api/agent/interventions`);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = (await res.json()) as { interventions: Intervention[] };
      setInterventions(data.interventions ?? []);
    } catch (err) {
      setActionError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoadingInterventions(false);
    }
  }, []);

  useEffect(() => {
    void fetchInterventions();
  }, [fetchInterventions, pendingCount]);

  const setMode = useCallback(async (m: AgentMode) => {
    if (!token) { setActionError("No API token loaded"); return; }
    setSettingMode(true);
    setActionError(null);
    try {
      const res = await fetch(`${BASE}/api/agent/mode`, {
        method: "POST",
        headers: authHeaders(),
        body: JSON.stringify({ mode: m }),
      });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
    } catch (err) {
      setActionError(err instanceof Error ? err.message : String(err));
    } finally {
      setSettingMode(false);
    }
  }, [token, authHeaders]);

  const handleIntervention = useCallback(async (id: string, action: "approve" | "reject") => {
    if (!token) { setActionError("No API token loaded"); return; }
    setActionError(null);
    try {
      const res = await fetch(`${BASE}/api/agent/interventions/${id}/${action}`, {
        method: "POST",
        headers: authHeaders(),
      });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      await fetchInterventions();
    } catch (err) {
      setActionError(err instanceof Error ? err.message : String(err));
    }
  }, [token, authHeaders, fetchInterventions]);

  const MODES: AgentMode[] = ["passive", "approval", "autonomous"];

  return (
    <div style={styles.root}>
      <div style={styles.header}>
        <h1 style={styles.title}>Agent</h1>
        <span className={modeBadgeClass(mode)}>
          <Bot size={10} />
          {modeLabel(mode)}
        </span>
      </div>

      {actionError && (
        <div style={styles.errorBanner} className="error-text">
          {actionError}
        </div>
      )}

      {/* Mode switcher */}
      <section style={styles.section}>
        <div className="section-title">Agent Mode</div>
        <div className="card" style={styles.modeCard}>
          <p style={styles.modeDescription}>
            Control how autonomously the agent applies thermal interventions.
          </p>
          <div style={styles.modeButtons}>
            {MODES.map((m) => (
              <button
                key={m}
                onClick={() => void setMode(m)}
                disabled={settingMode || mode === m}
                className={mode === m ? "btn-primary" : "btn-ghost"}
                style={styles.modeBtn}
              >
                {m.charAt(0).toUpperCase() + m.slice(1)}
              </button>
            ))}
          </div>
          <div style={styles.modeHints}>
            <span style={styles.hint}><strong>Passive</strong> — monitor only</span>
            <span style={styles.hint}><strong>Approval</strong> — propose changes, require sign-off</span>
            <span style={styles.hint}><strong>Autonomous</strong> — act immediately</span>
          </div>
        </div>
      </section>

      {/* Pending interventions */}
      {mode === "approval" && (
        <section style={styles.section}>
          <div style={styles.sectionRow}>
            <div className="section-title" style={{ marginBottom: 0 }}>
              Pending Interventions
              {pendingCount > 0 && (
                <span className="badge badge-yellow" style={{ marginLeft: 8 }}>
                  {pendingCount}
                </span>
              )}
            </div>
            <button
              className="btn-ghost"
              onClick={() => void fetchInterventions()}
              disabled={loadingInterventions}
              style={{ padding: "3px 8px" }}
            >
              <RefreshCw size={12} />
            </button>
          </div>

          <div style={styles.interventionList}>
            {loadingInterventions && (
              <p style={styles.loadingText} className="loading-pulse">Loading…</p>
            )}
            {!loadingInterventions && interventions.length === 0 && (
              <div className="card" style={styles.emptyCard}>
                <p style={styles.emptyText}>No pending interventions.</p>
              </div>
            )}
            {interventions.map((iv) => (
              <div key={iv.id} className="card" style={styles.interventionCard}>
                <p style={styles.ivDescription}>{iv.description}</p>
                <p style={styles.ivAction} className="mono">{iv.proposed_action}</p>
                <div style={styles.ivMeta}>
                  <span style={styles.ivTime}>
                    {new Date(Number(iv.proposed_at) * 1000).toLocaleTimeString()}
                  </span>
                  <div style={styles.ivActions}>
                    <button
                      className="btn-success"
                      onClick={() => void handleIntervention(iv.id, "approve")}
                    >
                      <CheckCircle size={12} style={{ marginRight: 4 }} />
                      Approve
                    </button>
                    <button
                      className="btn-danger"
                      onClick={() => void handleIntervention(iv.id, "reject")}
                    >
                      <XCircle size={12} style={{ marginRight: 4 }} />
                      Reject
                    </button>
                  </div>
                </div>
              </div>
            ))}
          </div>
        </section>
      )}

      {/* Event log */}
      <section style={styles.section}>
        <div className="section-title">Event Log</div>
        <div className="card" style={styles.logCard}>
          {events.length === 0 && (
            <p style={styles.emptyText}>Waiting for agent events…</p>
          )}
          {events.map((ev) => (
            <div key={ev.ts} style={styles.logEntry}>
              <span style={styles.logTime} className="mono">
                {new Date(ev.ts).toLocaleTimeString()}
              </span>
              <span style={styles.logRaw} className="mono">{ev.raw}</span>
            </div>
          ))}
        </div>
      </section>
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
    gap: 12,
    marginBottom: 24,
  },
  title: {
    fontSize: 20,
    fontWeight: 700,
    color: "var(--text-primary)",
    flex: 1,
  },
  errorBanner: {
    background: "var(--red-dim)",
    border: "1px solid rgba(239,68,68,0.3)",
    borderRadius: "var(--radius-sm)",
    padding: "8px 12px",
    marginBottom: 16,
    fontSize: 12,
  },
  section: {
    marginBottom: 28,
  },
  sectionRow: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    marginBottom: 12,
  },
  modeCard: {
    display: "flex",
    flexDirection: "column",
    gap: 14,
  },
  modeDescription: {
    color: "var(--text-secondary)",
    fontSize: 13,
  },
  modeButtons: {
    display: "flex",
    gap: 8,
  },
  modeBtn: {
    minWidth: 100,
  },
  modeHints: {
    display: "flex",
    flexDirection: "column",
    gap: 4,
  },
  hint: {
    fontSize: 12,
    color: "var(--text-muted)",
  },
  interventionList: {
    display: "flex",
    flexDirection: "column",
    gap: 10,
    marginTop: 12,
  },
  interventionCard: {
    display: "flex",
    flexDirection: "column",
    gap: 8,
  },
  ivDescription: {
    color: "var(--text-primary)",
    fontSize: 13,
  },
  ivAction: {
    color: "var(--accent)",
    fontSize: 12,
    background: "var(--accent-dim)",
    padding: "4px 8px",
    borderRadius: "var(--radius-sm)",
  },
  ivMeta: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
  },
  ivTime: {
    fontSize: 11,
    color: "var(--text-muted)",
    fontFamily: "var(--font-mono)",
  },
  ivActions: {
    display: "flex",
    gap: 8,
  },
  logCard: {
    maxHeight: 240,
    overflowY: "auto",
    display: "flex",
    flexDirection: "column",
    gap: 4,
  },
  logEntry: {
    display: "flex",
    gap: 10,
    alignItems: "flex-start",
    borderBottom: "1px solid var(--border)",
    paddingBottom: 4,
  },
  logTime: {
    fontSize: 11,
    color: "var(--text-muted)",
    flexShrink: 0,
  },
  logRaw: {
    fontSize: 11,
    color: "var(--text-secondary)",
    wordBreak: "break-all",
  },
  loadingText: {
    color: "var(--text-muted)",
    fontSize: 12,
  },
  emptyCard: {
    padding: 12,
  },
  emptyText: {
    color: "var(--text-muted)",
    fontSize: 12,
  },
};
