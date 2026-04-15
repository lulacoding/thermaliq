import { Bell, FileText, FolderOpen, Info } from "lucide-react";

const PROFILES_DIR = "~/.config/thermaliq/profiles/";

const EXAMPLE_ALERT = `[alerts]
[[alerts.rules]]
sensor_id = "cpu_package"
threshold_celsius = 90.0
action = "set_profile"
profile = "performance"
message = "CPU critical — switching to performance profile"`;

export default function AlertCenter() {
  return (
    <div style={styles.root}>
      <div style={styles.header}>
        <h1 style={styles.title}>Alert Center</h1>
      </div>

      <div style={styles.infoBanner}>
        <Info size={14} color="var(--accent)" style={{ flexShrink: 0, marginTop: 1 }} />
        <p style={styles.infoText}>
          Alerts and threshold rules are configured in TOML profile files — not through
          the UI. Edit the profile files directly to define alert conditions and actions.
        </p>
      </div>

      <div style={styles.grid}>
        <div className="card" style={styles.card}>
          <div style={styles.cardHeader}>
            <FolderOpen size={15} color="var(--accent)" />
            <span style={styles.cardTitle}>Profiles Directory</span>
          </div>
          <p style={styles.cardBody}>
            All thermal profiles and alert rules live in:
          </p>
          <div style={styles.pathBox} className="mono">
            {PROFILES_DIR}
          </div>
          <p style={styles.cardBody}>
            Each <span className="mono">.toml</span> file in this directory defines a
            named profile. Modify the file corresponding to your active profile to add
            or adjust alert rules.
          </p>
        </div>

        <div className="card" style={styles.card}>
          <div style={styles.cardHeader}>
            <FileText size={15} color="var(--accent)" />
            <span style={styles.cardTitle}>Alert Rule Format</span>
          </div>
          <p style={styles.cardBody}>
            Alert rules follow the TOML schema below. Each rule watches a sensor and
            triggers an action when the threshold is breached.
          </p>
          <pre style={styles.codeBlock} className="mono">
            {EXAMPLE_ALERT}
          </pre>
        </div>

        <div className="card" style={styles.card}>
          <div style={styles.cardHeader}>
            <Bell size={15} color="var(--accent)" />
            <span style={styles.cardTitle}>Available Actions</span>
          </div>
          <div style={styles.actionList}>
            {[
              { action: "set_profile", desc: "Switch to a named thermal profile" },
              { action: "set_fan_duty", desc: "Override fan duty % on a specific fan" },
              { action: "notify", desc: "Send a system notification (no hardware change)" },
              { action: "agent_intervention", desc: "Queue an agent intervention for review" },
            ].map(({ action, desc }) => (
              <div key={action} style={styles.actionRow}>
                <span style={styles.actionName} className="mono">{action}</span>
                <span style={styles.actionDesc}>{desc}</span>
              </div>
            ))}
          </div>
        </div>

        <div className="card" style={styles.card}>
          <div style={styles.cardHeader}>
            <Info size={15} color="var(--text-muted)" />
            <span style={styles.cardTitle}>Hot Reload</span>
          </div>
          <p style={styles.cardBody}>
            The daemon watches the profiles directory for changes. Editing a TOML file
            on disk takes effect immediately — no restart required. The active profile
            is re-evaluated on the next polling cycle (default: 1 second).
          </p>
        </div>
      </div>
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
    marginBottom: 20,
  },
  title: {
    fontSize: 20,
    fontWeight: 700,
    color: "var(--text-primary)",
  },
  infoBanner: {
    display: "flex",
    gap: 10,
    alignItems: "flex-start",
    background: "var(--accent-dim)",
    border: "1px solid rgba(59,130,246,0.3)",
    borderRadius: "var(--radius-sm)",
    padding: "10px 14px",
    marginBottom: 20,
  },
  infoText: {
    color: "var(--text-secondary)",
    fontSize: 13,
    lineHeight: 1.55,
  },
  grid: {
    display: "grid",
    gridTemplateColumns: "repeat(auto-fill, minmax(320px, 1fr))",
    gap: 14,
  },
  card: {
    display: "flex",
    flexDirection: "column",
    gap: 12,
  },
  cardHeader: {
    display: "flex",
    alignItems: "center",
    gap: 8,
  },
  cardTitle: {
    fontSize: 13,
    fontWeight: 600,
    color: "var(--text-primary)",
  },
  cardBody: {
    fontSize: 13,
    color: "var(--text-secondary)",
    lineHeight: 1.55,
  },
  pathBox: {
    background: "var(--bg-primary)",
    border: "1px solid var(--border)",
    borderRadius: "var(--radius-sm)",
    padding: "8px 12px",
    fontSize: 12,
    color: "var(--accent)",
  },
  codeBlock: {
    background: "var(--bg-primary)",
    border: "1px solid var(--border)",
    borderRadius: "var(--radius-sm)",
    padding: "10px 12px",
    fontSize: 11,
    color: "var(--text-secondary)",
    lineHeight: 1.6,
    overflowX: "auto",
    whiteSpace: "pre",
  },
  actionList: {
    display: "flex",
    flexDirection: "column",
    gap: 8,
  },
  actionRow: {
    display: "flex",
    gap: 10,
    alignItems: "flex-start",
  },
  actionName: {
    fontSize: 11,
    color: "var(--accent)",
    background: "var(--accent-dim)",
    padding: "2px 6px",
    borderRadius: "var(--radius-sm)",
    flexShrink: 0,
    marginTop: 1,
  },
  actionDesc: {
    fontSize: 12,
    color: "var(--text-secondary)",
    lineHeight: 1.4,
  },
};
