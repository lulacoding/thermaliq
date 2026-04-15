import { Wifi, WifiOff, Wind, Zap } from "lucide-react";
import { LineChart, Line, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer } from "recharts";
import { useDaemon, type Sensor, type Fan } from "../hooks/useDaemon";
import { useState, useEffect } from "react";

function tempColor(c: number): string {
  if (c >= 85) return "var(--red)";
  if (c >= 70) return "var(--yellow)";
  return "var(--accent)";
}

function tempBadgeClass(c: number): string {
  if (c >= 85) return "badge badge-red";
  if (c >= 70) return "badge badge-yellow";
  return "badge badge-green";
}

function dutyBarColor(pct: number): string {
  if (pct >= 85) return "var(--red)";
  if (pct >= 60) return "var(--yellow)";
  return "var(--accent)";
}

// Generate mock time-series data (1 hour)
function generateChartData(sensors: Sensor[]) {
  const data = [];
  const now = Date.now();
  const cpuSensor = sensors.find((s) => s.label.includes("CPU"));
  const gpuSensor = sensors.find((s) => s.label.includes("GPU"));

  for (let i = 59; i >= 0; i--) {
    const time = new Date(now - i * 60000);
    data.push({
      time: `${time.getHours().toString().padStart(2, "0")}:${time.getMinutes().toString().padStart(2, "0")}`,
      cpu: cpuSensor ? cpuSensor.temp_celsius + (Math.random() - 0.5) * 3 : 45,
      gpu: gpuSensor ? gpuSensor.temp_celsius + (Math.random() - 0.5) * 3 : 50,
    });
  }
  return data;
}

interface LogEntry {
  time: string;
  type: "DETECTED" | "ACTION" | "LOGIC" | "SYSTEM";
  message: string;
}

// Mock log entries
const mockLogs: LogEntry[] = [
  { time: "10:15:22", type: "DETECTED", message: "GPU load spike to 87%" },
  { time: "10:15:45", type: "ACTION", message: "Activated aggressive profile" },
  { time: "10:16:10", type: "LOGIC", message: "Curve interpolation: duty 72%" },
  { time: "10:16:35", type: "SYSTEM", message: "Fan RPM: 4200 → 5100" },
  { time: "10:17:02", type: "DETECTED", message: "CPU temp stabilized at 68°C" },
  { time: "10:17:18", type: "ACTION", message: "Switched to balanced profile" },
];

function getLogColor(type: LogEntry["type"]): string {
  switch (type) {
    case "DETECTED": return "var(--accent)";
    case "ACTION": return "var(--secondary)";
    case "LOGIC": return "var(--tertiary)";
    case "SYSTEM": return "var(--text-muted)";
  }
}

function FanSpinner({ fan }: { fan: Fan }) {
  const durations = [1.5, 1.6, 1.8, 2.0, 2.2];
  const duration = durations[fan.id % durations.length];

  return (
    <div style={styles.fanCell}>
      <style>{`
        @keyframes spin-fan-${fan.id} {
          from { transform: rotate(0deg); }
          to { transform: rotate(360deg); }
        }
      `}</style>
      <div
        style={{
          animation: `spin-fan-${fan.id} ${duration}s linear infinite`,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
        }}
      >
        <Wind size={14} />
      </div>
      <div style={styles.fanInfo}>
        <div style={styles.fanLabel}>{fan.label}</div>
        <div style={styles.fanRpm} className="mono">{fan.rpm.toLocaleString()}</div>
      </div>
    </div>
  );
}

export default function Dashboard() {
  const { sensors, fans, connected } = useDaemon();
  const [chartData, setChartData] = useState(generateChartData(sensors));

  useEffect(() => {
    setChartData(generateChartData(sensors));
  }, [sensors]);

  const cpuSensor = sensors.find((s) => s.label.includes("CPU"));
  const gpuSensor = sensors.find((s) => s.label.includes("GPU"));

  return (
    <div style={styles.root}>
      <div style={styles.header}>
        <h1 style={styles.title}>Dashboard</h1>
        <div style={styles.connBadge}>
          {connected ? (
            <span className="badge badge-green">
              <Wifi size={10} />
              Live
            </span>
          ) : (
            <span className="badge badge-red">
              <WifiOff size={10} />
              Disconnected
            </span>
          )}
        </div>
      </div>

      {!connected && sensors.length === 0 && (
        <div style={styles.emptyState}>
          <WifiOff size={32} color="var(--text-muted)" />
          <p style={styles.emptyText}>Connecting to daemon at 127.0.0.1:7272…</p>
          <p style={styles.emptySubtext}>
            Make sure <span className="mono">thermaliqd</span> is running.
          </p>
        </div>
      )}

      {sensors.length > 0 && (
        <>
          {/* 12-Column Grid Dashboard */}
          <div className="grid-bg" style={styles.gridContainer}>
            {/* CPU Core Temp — 5 cols × 4 rows */}
            {cpuSensor && (
              <div style={{ ...styles.card, gridColumn: "span 5", gridRow: "span 4" }}>
                <div style={styles.cardAccentBar} />
                <div style={styles.cardLabel}>CPU Temperature</div>
                <div style={{ ...styles.primaryTempValue, color: tempColor(cpuSensor.temp_celsius) }} className="mono">
                  {cpuSensor.temp_celsius.toFixed(1)}
                  <span style={styles.tempUnit}>°C</span>
                </div>
                <span className={tempBadgeClass(cpuSensor.temp_celsius)}>
                  {cpuSensor.temp_celsius >= 85
                    ? "Critical"
                    : cpuSensor.temp_celsius >= 70
                    ? "Warm"
                    : "Normal"}
                </span>
              </div>
            )}

            {/* GPU Temp — 4 cols × 2 rows */}
            {gpuSensor && (
              <div style={{ ...styles.card, gridColumn: "span 4", gridRow: "span 2" }}>
                <div style={{ ...styles.cardAccentBar, background: "var(--secondary)" }} />
                <div style={styles.cardLabel}>{gpuSensor.label}</div>
                <div style={{ ...styles.tempValue, color: tempColor(gpuSensor.temp_celsius) }} className="mono">
                  {gpuSensor.temp_celsius.toFixed(1)}°C
                </div>
              </div>
            )}

            {/* Liquid/Airflow Status — 4 cols × 2 rows */}
            <div style={{ ...styles.card, gridColumn: "span 4", gridRow: "span 2" }}>
              <div style={{ ...styles.cardAccentBar, background: "var(--tertiary)" }} />
              <div style={styles.cardLabel}>Airflow</div>
              <div style={{ ...styles.tempValue, color: "var(--tertiary)" }} className="mono">
                85%
              </div>
              <span className="badge badge-blue">Optimal</span>
            </div>

            {/* Fan Group — 3 cols × 2 rows */}
            <div style={{ ...styles.card, gridColumn: "span 3", gridRow: "span 2" }}>
              <div style={styles.cardAccentBar} />
              <div style={styles.cardLabel}>Fans</div>
              <div style={styles.fanGroupCount} className="mono">{fans.length}</div>
            </div>

            {/* AI Status + Hardware — 7 cols × 2 rows */}
            <div style={{ ...styles.card, gridColumn: "span 7", gridRow: "span 2" }}>
              <div style={styles.cardAccentBar} />
              <div style={styles.cardLabel}>AI Agent</div>
              <div style={styles.agentRow}>
                <div style={styles.agentStatus}>
                  <Zap size={12} color="var(--accent)" style={{ marginRight: 4 }} />
                  <span className="mono">Active</span>
                </div>
                <div style={styles.hardwareInfo} className="mono">
                  {sensors.length} sensors • {fans.length} fans
                </div>
              </div>
            </div>

            {/* Temperature Chart — 12 cols × 3 rows (below CPU) */}
            <div style={{ ...styles.card, gridColumn: "span 12", gridRow: "span 3", marginTop: 8 }}>
              <div style={styles.cardAccentBar} />
              <div style={styles.cardLabel}>1-Hour Temperature Trend</div>
              <ResponsiveContainer width="100%" height={160}>
                <LineChart data={chartData} margin={{ top: 5, right: 10, left: -20, bottom: 5 }}>
                  <CartesianGrid strokeDasharray="3 3" stroke="var(--border)" vertical={false} />
                  <XAxis
                    dataKey="time"
                    stroke="var(--text-muted)"
                    style={{ fontSize: 10 }}
                    tick={{ dy: 4 }}
                  />
                  <YAxis stroke="var(--text-muted)" style={{ fontSize: 10 }} domain={[30, 100]} />
                  <Tooltip
                    contentStyle={{
                      background: "var(--bg-card)",
                      border: "1px solid var(--border)",
                    }}
                    labelStyle={{ color: "var(--text-primary)" }}
                  />
                  <Line
                    type="monotone"
                    dataKey="cpu"
                    stroke="var(--accent)"
                    dot={false}
                    strokeWidth={2}
                    isAnimationActive={false}
                  />
                  <Line
                    type="monotone"
                    dataKey="gpu"
                    stroke="var(--secondary)"
                    dot={false}
                    strokeWidth={2}
                    isAnimationActive={false}
                  />
                </LineChart>
              </ResponsiveContainer>
            </div>
          </div>

          {/* Terminal Panel — Bottom 40% */}
          <div style={styles.terminalPanel}>
            <div style={styles.terminalHeader}>
              <span style={styles.terminalTitle}>AGENT LOG</span>
              <div style={styles.statusSquare} />
            </div>
            <div style={styles.logContainer}>
              {mockLogs.map((log, i) => (
                <div key={i} style={styles.logEntry}>
                  <span style={styles.logTime}>[{log.time}]</span>
                  <span style={{ ...styles.logTag, color: getLogColor(log.type) }}>
                    [{log.type}]
                  </span>
                  <span style={styles.logMessage}>{log.message}</span>
                </div>
              ))}
            </div>
            <div style={styles.terminalFooter}>
              <span className="mono" style={styles.profileLabel}>
                Profile: Balanced • Fans: {fans.length}
              </span>
            </div>
          </div>

          {/* Fan Status Grid — overlay below logs */}
          {fans.length > 0 && (
            <div style={styles.fanStatusGrid}>
              {fans.map((f) => (
                <FanSpinner key={f.id} fan={f} />
              ))}
            </div>
          )}
        </>
      )}
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  root: {
    padding: 16,
    paddingTop: 0,
    height: "100%",
    display: "flex",
    flexDirection: "column",
    gap: 16,
    overflow: "auto",
  },
  header: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    paddingTop: 16,
  },
  title: {
    fontSize: 22,
    fontWeight: 700,
    color: "var(--text-primary)",
    margin: 0,
  },
  connBadge: {
    display: "flex",
    alignItems: "center",
  },
  gridContainer: {
    display: "grid",
    gridTemplateColumns: "repeat(12, 1fr)",
    gap: 12,
    padding: 16,
    background: "var(--bg-card)",
    border: "1px solid var(--border)",
    borderRadius: "var(--radius)",
    minHeight: 300,
    flex: "0 0 auto",
  },
  card: {
    background: "var(--bg-card)",
    border: "1px solid var(--border)",
    borderRadius: "var(--radius)",
    padding: 12,
    position: "relative",
    overflow: "hidden",
    display: "flex",
    flexDirection: "column",
    gap: 8,
  },
  cardAccentBar: {
    position: "absolute",
    bottom: 0,
    left: 0,
    right: 0,
    height: 4,
    background: "var(--accent)",
  },
  cardLabel: {
    fontSize: 8,
    fontWeight: 600,
    letterSpacing: "0.1em",
    textTransform: "uppercase",
    color: "var(--text-muted)",
  },
  primaryTempValue: {
    fontSize: 56,
    fontWeight: 900,
    lineHeight: 1,
    letterSpacing: "-0.03em",
  },
  tempValue: {
    fontSize: 32,
    fontWeight: 700,
    lineHeight: 1,
    letterSpacing: "-0.03em",
  },
  tempUnit: {
    fontSize: 14,
    fontWeight: 400,
    marginLeft: 2,
    opacity: 0.7,
  },
  fanGroupCount: {
    fontSize: 40,
    fontWeight: 700,
    color: "var(--accent)",
  },
  agentRow: {
    display: "flex",
    alignItems: "center",
    gap: 12,
    fontSize: 11,
  },
  agentStatus: {
    display: "flex",
    alignItems: "center",
    color: "var(--accent)",
  },
  hardwareInfo: {
    color: "var(--text-muted)",
    fontSize: 10,
  },
  terminalPanel: {
    background: "var(--bg-elevated)",
    border: "1px solid var(--border)",
    borderRadius: "var(--radius)",
    display: "flex",
    flexDirection: "column",
    height: 200,
    flex: "0 0 auto",
    overflow: "hidden",
  },
  terminalHeader: {
    height: 24,
    background: "#111111",
    borderBottom: "1px solid var(--border)",
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    paddingLeft: 12,
    paddingRight: 12,
  },
  terminalTitle: {
    fontSize: 10,
    fontWeight: 700,
    color: "var(--text-muted)",
    letterSpacing: "0.1em",
  },
  statusSquare: {
    width: 8,
    height: 8,
    background: "var(--accent)",
    borderRadius: 1,
  },
  logContainer: {
    flex: 1,
    overflow: "auto",
    padding: "8px 12px",
    display: "flex",
    flexDirection: "column",
    gap: 2,
  },
  logEntry: {
    fontSize: "0.65rem",
    color: "var(--text-secondary)",
    fontFamily: "var(--font-mono)",
    display: "flex",
    gap: 4,
  },
  logTime: {
    color: "var(--text-muted)",
    minWidth: 60,
  },
  logTag: {
    minWidth: 80,
    fontWeight: 600,
  },
  logMessage: {
    flex: 1,
  },
  terminalFooter: {
    height: 24,
    background: "var(--accent)",
    color: "#003907",
    borderTop: "1px solid var(--border)",
    display: "flex",
    alignItems: "center",
    paddingLeft: 12,
    fontSize: 10,
    fontWeight: 600,
  },
  profileLabel: {
    fontSize: 9,
  },
  fanStatusGrid: {
    display: "grid",
    gridTemplateColumns: "repeat(auto-fill, minmax(120px, 1fr))",
    gap: 8,
    flex: "0 0 auto",
  },
  fanCell: {
    background: "var(--bg-card)",
    border: "1px solid var(--border)",
    borderRadius: "var(--radius)",
    padding: 8,
    display: "flex",
    flexDirection: "column",
    alignItems: "center",
    gap: 6,
  },
  fanInfo: {
    display: "flex",
    flexDirection: "column",
    alignItems: "center",
    gap: 2,
  },
  fanLabel: {
    fontSize: 9,
    color: "var(--text-muted)",
    textTransform: "uppercase",
    letterSpacing: "0.06em",
  },
  fanRpm: {
    fontSize: 12,
    fontWeight: 700,
    color: "var(--text-primary)",
  },
  emptyState: {
    display: "flex",
    flexDirection: "column",
    alignItems: "center",
    justifyContent: "center",
    gap: 12,
    paddingTop: 80,
    textAlign: "center",
    flex: 1,
  },
  emptyText: {
    color: "var(--text-secondary)",
    fontSize: 14,
  },
  emptySubtext: {
    color: "var(--text-muted)",
    fontSize: 12,
  },
};
