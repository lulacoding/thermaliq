import { useState, useEffect } from "react";
import { LineChart, Line, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer } from "recharts";

interface Breakpoint {
  id: string;
  temp: number;
  duty: number;
}

const API_BASE = "http://127.0.0.1:7272";

export default function CurveEditor() {
  const [aiInput, setAiInput] = useState("");
  const [breakpoints, setBreakpoints] = useState<Breakpoint[]>([
    { id: "1", temp: 30, duty: 20 },
    { id: "2", temp: 50, duty: 40 },
    { id: "3", temp: 70, duty: 70 },
    { id: "4", temp: 85, duty: 100 },
  ]);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editTemp, setEditTemp] = useState("");
  const [editDuty, setEditDuty] = useState("");
  const [profileName, setProfileName] = useState("custom");
  const [fanId, setFanId] = useState("cpu_fan");
  const [aiLoading, setAiLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);
  const [apiToken, setApiToken] = useState<string | null>(null);

  const handleChartClick = (data: any) => {
    // Allow adding new points by clicking on the chart
    if (data && data.activeTooltipIndex !== undefined) {
      const temp = Math.round(data.xAxisPayload?.value);
      if (temp) {
        const newDuty = Math.round((temp / 85) * 100);
        const newId = Math.max(...breakpoints.map((b) => parseInt(b.id)), 0) + 1;
        setBreakpoints([...breakpoints, { id: newId.toString(), temp, duty: Math.min(newDuty, 100) }].sort((a, b) => a.temp - b.temp));
      }
    }
  };

  const handleEditStart = (bp: Breakpoint) => {
    setEditingId(bp.id);
    setEditTemp(bp.temp.toString());
    setEditDuty(bp.duty.toString());
  };

  const handleEditSave = () => {
    if (editingId) {
      setBreakpoints(
        breakpoints.map((bp) =>
          bp.id === editingId
            ? { ...bp, temp: parseInt(editTemp) || bp.temp, duty: parseInt(editDuty) || bp.duty }
            : bp
        )
      );
      setEditingId(null);
    }
  };

  const handleDelete = (id: string) => {
    setBreakpoints(breakpoints.filter((bp) => bp.id !== id));
  };

  // Load API token from localStorage on mount
  useEffect(() => {
    const token = localStorage.getItem("api_token");
    if (token) {
      setApiToken(token);
    }
  }, []);

  const handleAiDescribe = async () => {
    if (!aiInput.trim()) {
      return;
    }

    try {
      setAiLoading(true);
      setError(null);
      setSuccess(null);

      const token = apiToken || localStorage.getItem("api_token");
      const headers: HeadersInit = {
        "Content-Type": "application/json",
      };
      if (token) {
        headers["Authorization"] = `Bearer ${token}`;
      }

      const response = await fetch(`${API_BASE}/api/curves/ai-generate`, {
        method: "POST",
        headers,
        body: JSON.stringify({
          prompt: aiInput,
          fan_id: fanId,
        }),
      });

      if (!response.ok) {
        const errorData = await response.text();
        throw new Error(`Failed to generate curve: ${errorData}`);
      }

      const data = await response.json();

      // Extract curve points from AI response
      if (data.curve && data.curve.points) {
        const aiPoints: Breakpoint[] = data.curve.points.map(
          (point: any, idx: number) => ({
            id: `ai_${idx}`,
            temp: Math.round(point.temp_celsius),
            duty: Math.round(point.duty_percent),
          })
        );
        setBreakpoints(aiPoints.sort((a, b) => a.temp - b.temp));
        setSuccess(`AI generated curve: ${data.generated_profile}`);
        setTimeout(() => setSuccess(null), 3000);
      }

      setAiInput("");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to generate curve");
    } finally {
      setAiLoading(false);
    }
  };

  const handleSave = async () => {
    try {
      setSaving(true);
      setError(null);
      setSuccess(null);

      const token = apiToken || localStorage.getItem("api_token");
      const headers: HeadersInit = {
        "Content-Type": "application/json",
      };
      if (token) {
        headers["Authorization"] = `Bearer ${token}`;
      }

      const points = breakpoints.map((bp) => ({
        temp_celsius: bp.temp,
        duty_percent: bp.duty,
      }));

      const response = await fetch(`${API_BASE}/api/curves/save`, {
        method: "POST",
        headers,
        body: JSON.stringify({
          profile: profileName,
          fan_id: fanId,
          points,
        }),
      });

      if (!response.ok) {
        const errorData = await response.text();
        throw new Error(`Failed to save curve: ${errorData}`);
      }

      setSuccess("Curve saved successfully!");
      setTimeout(() => setSuccess(null), 3000);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to save curve");
    } finally {
      setSaving(false);
    }
  };

  const chartData = breakpoints.map((bp) => ({
    temp: bp.temp,
    duty: bp.duty,
    id: bp.id,
  }));

  return (
    <div style={styles.container}>
      <div style={styles.header}>
        <h1 style={styles.title}>Curve Editor</h1>
        <p style={styles.subtitle}>Drag points on the chart or edit the table below. Use AI to generate curves automatically.</p>
      </div>

      {error && <div style={styles.errorBox}>{error}</div>}
      {success && <div style={styles.successBox}>{success}</div>}

      {/* Profile & Fan Selection */}
      <div style={{...styles.section, ...styles.sectionCard}}>
        <div style={styles.cardAccentBar} />
        <h2 style={styles.sectionTitle}>Curve Settings</h2>
        <div style={styles.settingRow}>
          <div style={styles.settingGroup}>
            <label style={styles.label}>Profile Name</label>
            <input
              type="text"
              style={styles.input}
              value={profileName}
              onChange={(e) => setProfileName(e.target.value)}
              placeholder="e.g., balanced, performance"
            />
          </div>
          <div style={styles.settingGroup}>
            <label style={styles.label}>Fan ID</label>
            <input
              type="text"
              style={styles.input}
              value={fanId}
              onChange={(e) => setFanId(e.target.value)}
              placeholder="e.g., cpu_fan, gpu_fan"
            />
          </div>
        </div>
      </div>

      {/* AI Describe Input */}
      <div style={{...styles.section, ...styles.sectionCard}}>
        <div style={styles.cardAccentBar} />
        <h2 style={styles.sectionTitle}>AI Curve Generation</h2>
        <div style={styles.aiInputGroup}>
          <input
            type="text"
            style={styles.aiInput}
            placeholder="Describe what you want: e.g., 'aggressive cooling' or 'silent mode'"
            value={aiInput}
            onChange={(e) => setAiInput(e.target.value)}
            onKeyPress={(e) => e.key === "Enter" && handleAiDescribe()}
            disabled={aiLoading}
          />
          <button
            style={{
              ...styles.aiButton,
              ...(aiLoading ? styles.buttonDisabled : {}),
            }}
            onClick={handleAiDescribe}
            disabled={aiLoading}
          >
            {aiLoading ? "Generating..." : "Generate"}
          </button>
        </div>
      </div>

      {/* Chart */}
      <div style={{...styles.section, ...styles.sectionCard}}>
        <div style={styles.cardAccentBar} />
        <h2 style={styles.sectionTitle}>Fan Curve</h2>
        <div style={styles.chartContainer}>
          <ResponsiveContainer width="100%" height={300}>
            <LineChart data={chartData}>
              <CartesianGrid strokeDasharray="3 3" stroke="var(--border)" />
              <XAxis
                dataKey="temp"
                label={{ value: "Temperature (°C)", position: "insideBottomRight", offset: -5 }}
                stroke="var(--text-muted)"
              />
              <YAxis
                label={{ value: "Fan Duty (%)", angle: -90, position: "insideLeft" }}
                domain={[0, 100]}
                stroke="var(--text-muted)"
              />
              <Tooltip
                contentStyle={{
                  background: "var(--bg-card)",
                  border: "1px solid var(--border)",
                  borderRadius: "4px",
                  color: "var(--text-primary)",
                }}
              />
              <Line
                type="monotone"
                dataKey="duty"
                stroke="var(--accent)"
                strokeWidth={2}
                dot={{ fill: "var(--accent)", r: 5 }}
                onClick={handleChartClick}
              />
            </LineChart>
          </ResponsiveContainer>
          <p style={styles.chartHint}>Click on the chart to add new points</p>
        </div>
      </div>

      {/* Breakpoint Table */}
      <div style={{...styles.section, ...styles.sectionCard}}>
        <div style={styles.cardAccentBar} />
        <h2 style={styles.sectionTitle}>Breakpoints</h2>
        <div style={styles.tableContainer}>
          <table style={styles.table}>
            <thead>
              <tr>
                <th style={styles.th}>Temperature (°C)</th>
                <th style={styles.th}>Fan Duty (%)</th>
                <th style={styles.th}>Action</th>
              </tr>
            </thead>
            <tbody>
              {breakpoints
                .sort((a, b) => a.temp - b.temp)
                .map((bp) => (
                  <tr key={bp.id}>
                    <td style={styles.td}>
                      {editingId === bp.id ? (
                        <input
                          type="number"
                          style={styles.tableInput}
                          value={editTemp}
                          onChange={(e) => setEditTemp(e.target.value)}
                          min="0"
                          max="120"
                        />
                      ) : (
                        bp.temp
                      )}
                    </td>
                    <td style={styles.td}>
                      {editingId === bp.id ? (
                        <input
                          type="number"
                          style={styles.tableInput}
                          value={editDuty}
                          onChange={(e) => setEditDuty(e.target.value)}
                          min="0"
                          max="100"
                        />
                      ) : (
                        bp.duty
                      )}
                    </td>
                    <td style={styles.td}>
                      {editingId === bp.id ? (
                        <>
                          <button style={styles.actionButton} onClick={handleEditSave}>
                            ✓
                          </button>
                          <button style={{ ...styles.actionButton, color: "var(--red)" }} onClick={() => setEditingId(null)}>
                            ✕
                          </button>
                        </>
                      ) : (
                        <>
                          <button style={styles.actionButton} onClick={() => handleEditStart(bp)}>
                            Edit
                          </button>
                          <button style={{ ...styles.actionButton, color: "var(--red)" }} onClick={() => handleDelete(bp.id)}>
                            Delete
                          </button>
                        </>
                      )}
                    </td>
                  </tr>
                ))}
            </tbody>
          </table>
        </div>
      </div>

      {/* Save Button */}
      <div style={styles.footer}>
        <button
          style={{
            ...styles.saveButton,
            ...(saving ? styles.buttonDisabled : {}),
          }}
          onClick={handleSave}
          disabled={saving}
        >
          {saving ? "Saving..." : "Save Curve"}
        </button>
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
  aiInputGroup: {
    display: "flex",
    gap: 8,
  },
  aiInput: {
    flex: 1,
    padding: "8px 12px",
    background: "var(--bg-card)",
    border: "1px solid var(--border)",
    borderRadius: "var(--radius)",
    color: "var(--text-primary)",
    fontSize: 13,
    fontFamily: "inherit",
  },
  aiButton: {
    padding: "8px 16px",
    background: "var(--accent)",
    color: "#fff",
    border: "none",
    borderRadius: "var(--radius)",
    fontWeight: 600,
    fontSize: 13,
    cursor: "pointer",
    whiteSpace: "nowrap",
  },
  chartContainer: {
    background: "var(--bg-card)",
    border: "1px solid var(--border)",
    borderRadius: "var(--radius)",
    padding: "16px",
  },
  chartHint: {
    fontSize: 12,
    color: "var(--text-muted)",
    margin: "8px 0 0 0",
  },
  tableContainer: {
    overflowX: "auto",
  },
  table: {
    width: "100%",
    borderCollapse: "collapse",
    fontSize: 13,
  },
  th: {
    padding: "8px 12px",
    textAlign: "left",
    fontWeight: 600,
    color: "var(--text-primary)",
    borderBottom: "2px solid var(--border)",
    background: "var(--bg-card)",
  },
  td: {
    padding: "8px 12px",
    borderBottom: "1px solid var(--border)",
    color: "var(--text-primary)",
  },
  tableInput: {
    width: "100%",
    padding: "4px 6px",
    background: "var(--bg-card)",
    border: "1px solid var(--accent)",
    borderRadius: "4px",
    color: "var(--text-primary)",
    fontSize: 13,
    fontFamily: "inherit",
    boxSizing: "border-box",
  },
  actionButton: {
    padding: "4px 8px",
    background: "transparent",
    color: "var(--accent)",
    border: "1px solid var(--accent)",
    borderRadius: "4px",
    fontSize: 12,
    cursor: "pointer",
    marginRight: "4px",
  },
  footer: {
    display: "flex",
    gap: 12,
    paddingTop: 24,
    borderTop: "1px solid var(--border)",
  },
  saveButton: {
    padding: "8px 20px",
    background: "var(--accent)",
    color: "#fff",
    border: "none",
    borderRadius: "var(--radius)",
    fontWeight: 600,
    fontSize: 13,
    cursor: "pointer",
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
  settingRow: {
    display: "flex",
    gap: 16,
  },
  settingGroup: {
    flex: 1,
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
};
