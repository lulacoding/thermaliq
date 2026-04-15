export default function SettingsPanel() {
  return (
    <div style={styles.container}>
      <div style={styles.header}>
        <h1 style={styles.title}>Settings</h1>
        <p style={styles.subtitle}>Configure API keys, trust levels, and safety guardrails</p>
      </div>

      <div style={{...styles.section, ...styles.sectionCard}}>
        <div style={styles.cardAccentBar} />
        <h2 style={styles.sectionTitle}>AI Provider</h2>
        <div style={styles.settingGroup}>
          <label style={styles.label}>API Provider</label>
          <select style={styles.select}>
            <option>Anthropic (Claude)</option>
            <option>OpenAI</option>
            <option>Google Gemini</option>
            <option>Ollama (Local)</option>
            <option>OpenRouter</option>
          </select>
        </div>
        <div style={styles.settingGroup}>
          <label style={styles.label}>API Key</label>
          <input type="password" style={styles.input} placeholder="Enter your API key" />
        </div>
      </div>

      <div style={{...styles.section, ...styles.sectionCard}}>
        <div style={{...styles.cardAccentBar, background: 'var(--secondary)'}} />
        <h2 style={styles.sectionTitle}>Trust Level</h2>
        <div style={styles.settingGroup}>
          <label style={styles.label}>Agent Autonomy</label>
          <select style={styles.select}>
            <option>Full Auto (AI makes all decisions)</option>
            <option>Bounded (within guardrails only)</option>
            <option>Suggest Only (AI recommends, you approve)</option>
          </select>
        </div>
      </div>

      <div style={{...styles.section, ...styles.sectionCard}}>
        <div style={styles.cardAccentBar} />
        <h2 style={styles.sectionTitle}>Safety Guardrails</h2>
        <div style={styles.settingGroup}>
          <label style={styles.label}>Minimum Fan Duty (%)</label>
          <input type="number" style={styles.input} min="0" max="100" placeholder="20" />
        </div>
        <div style={styles.settingGroup}>
          <label style={styles.label}>Thermal Kill-Switch (°C)</label>
          <input type="number" style={styles.input} min="30" max="120" placeholder="95" />
        </div>
        <div style={styles.settingGroup}>
          <label style={styles.label}>Curve Change Cooldown (seconds)</label>
          <input type="number" style={styles.input} min="0" max="3600" placeholder="30" />
        </div>
      </div>

      <div style={{...styles.section, ...styles.sectionCard}}>
        <div style={styles.cardAccentBar} />
        <h2 style={styles.sectionTitle}>General</h2>
        <div style={styles.checkboxGroup}>
          <input type="checkbox" id="autostart" defaultChecked />
          <label htmlFor="autostart" style={styles.checkboxLabel}>Start daemon on boot</label>
        </div>
        <div style={styles.checkboxGroup}>
          <input type="checkbox" defaultChecked />
          <label style={styles.checkboxLabel}>Send telemetry (helps improve AI)</label>
        </div>
      </div>

      <button style={styles.button}>Save Settings</button>
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
};
