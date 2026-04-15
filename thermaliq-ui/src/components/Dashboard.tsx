import { Wifi, WifiOff, Thermometer, Wind } from "lucide-react";
import { useDaemon, type Sensor, type Fan } from "../hooks/useDaemon";

function tempColor(c: number): string {
  if (c >= 85) return "var(--red)";
  if (c >= 70) return "var(--yellow)";
  return "var(--green)";
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

function SensorCard({ sensor }: { sensor: Sensor }) {
  const color = tempColor(sensor.temp_celsius);
  return (
    <div className="card" style={styles.sensorCard}>
      <div style={styles.cardHeader}>
        <Thermometer size={13} color="var(--text-muted)" />
        <span style={styles.cardLabel}>{sensor.label}</span>
        <span style={styles.cardId}>#{sensor.id}</span>
      </div>
      <div style={{ ...styles.tempValue, color }} className="mono">
        {sensor.temp_celsius.toFixed(1)}
        <span style={styles.tempUnit}>°C</span>
      </div>
      <div>
        <span className={tempBadgeClass(sensor.temp_celsius)}>
          {sensor.temp_celsius >= 85
            ? "Critical"
            : sensor.temp_celsius >= 70
            ? "Warm"
            : "Normal"}
        </span>
      </div>
    </div>
  );
}

function FanCard({ fan }: { fan: Fan }) {
  const barColor = dutyBarColor(fan.duty_percent);
  return (
    <div className="card" style={styles.fanCard}>
      <div style={styles.cardHeader}>
        <Wind size={13} color="var(--text-muted)" />
        <span style={styles.cardLabel}>{fan.label}</span>
        <span style={styles.cardId}>#{fan.id}</span>
      </div>
      <div style={styles.fanStats}>
        <div>
          <div style={styles.statLabel}>RPM</div>
          <div style={styles.statValue} className="mono">
            {fan.rpm.toLocaleString()}
          </div>
        </div>
        <div>
          <div style={styles.statLabel}>Duty</div>
          <div style={{ ...styles.statValue, color: barColor }} className="mono">
            {fan.duty_percent}%
          </div>
        </div>
      </div>
      <div style={styles.barTrack}>
        <div
          style={{
            ...styles.barFill,
            width: `${Math.min(fan.duty_percent, 100)}%`,
            background: barColor,
          }}
        />
      </div>
    </div>
  );
}

export default function Dashboard() {
  const { sensors, fans, connected } = useDaemon();

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
        <section style={styles.section}>
          <div className="section-title">Thermal Sensors</div>
          <div style={styles.grid}>
            {sensors.map((s) => (
              <SensorCard key={s.id} sensor={s} />
            ))}
          </div>
        </section>
      )}

      {fans.length > 0 && (
        <section style={styles.section}>
          <div className="section-title">Fans</div>
          <div style={styles.grid}>
            {fans.map((f) => (
              <FanCard key={f.id} fan={f} />
            ))}
          </div>
        </section>
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
  connBadge: {
    display: "flex",
    alignItems: "center",
  },
  section: {
    marginBottom: 28,
  },
  grid: {
    display: "grid",
    gridTemplateColumns: "repeat(auto-fill, minmax(180px, 1fr))",
    gap: 12,
  },
  sensorCard: {
    display: "flex",
    flexDirection: "column",
    gap: 10,
  },
  fanCard: {
    display: "flex",
    flexDirection: "column",
    gap: 10,
  },
  cardHeader: {
    display: "flex",
    alignItems: "center",
    gap: 6,
    overflow: "hidden",
  },
  cardLabel: {
    fontSize: 12,
    fontWeight: 600,
    color: "var(--text-secondary)",
    flex: 1,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  cardId: {
    fontSize: 10,
    color: "var(--text-muted)",
    fontFamily: "var(--font-mono)",
  },
  tempValue: {
    fontSize: 36,
    fontWeight: 700,
    lineHeight: 1,
    letterSpacing: "-0.03em",
  },
  tempUnit: {
    fontSize: 16,
    fontWeight: 400,
    marginLeft: 2,
    opacity: 0.7,
  },
  fanStats: {
    display: "flex",
    gap: 20,
  },
  statLabel: {
    fontSize: 10,
    color: "var(--text-muted)",
    textTransform: "uppercase",
    letterSpacing: "0.06em",
    marginBottom: 2,
  },
  statValue: {
    fontSize: 22,
    fontWeight: 700,
    lineHeight: 1,
  },
  barTrack: {
    height: 4,
    background: "var(--border)",
    borderRadius: 2,
    overflow: "hidden",
  },
  barFill: {
    height: "100%",
    borderRadius: 2,
    transition: "width 0.4s ease",
  },
  emptyState: {
    display: "flex",
    flexDirection: "column",
    alignItems: "center",
    justifyContent: "center",
    gap: 12,
    paddingTop: 80,
    textAlign: "center",
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
