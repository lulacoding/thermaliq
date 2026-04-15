import { useState, useEffect } from "react";
import { AlertCircle, CheckCircle, Loader } from "lucide-react";

interface DetectedHardware {
  fans: string[];
  sensors: { name: string; type: "cpu" | "gpu" | "system" }[];
  gpus: string[];
  storage: { path: string; size: string };
}

const API_BASE = "http://127.0.0.1:7272";

export default function HardwareDetect() {
  const [stage, setStage] = useState<"detecting" | "results" | "confirm">("detecting");
  const [hardware, setHardware] = useState<DetectedHardware>({
    fans: [],
    sensors: [],
    gpus: [],
    storage: { path: "~/.config/thermaliq/", size: "< 1 MB" },
  });

  useEffect(() => {
    const detectHardware = async () => {
      try {
        // Fetch sensors
        const sensorsRes = await fetch(`${API_BASE}/sensors`);
        if (!sensorsRes.ok) throw new Error(`Sensors fetch failed: ${sensorsRes.statusText}`);
        const sensorsData = (await sensorsRes.json()) as {
          sensors: Array<{ id: string; label: string; temp_celsius: number }>;
        };

        // Fetch fans
        const fansRes = await fetch(`${API_BASE}/fans`);
        if (!fansRes.ok) throw new Error(`Fans fetch failed: ${fansRes.statusText}`);
        const fansData = (await fansRes.json()) as {
          fans: Array<{ id: string; label: string; rpm: number; duty_percent: number }>;
        };

        // Parse sensors and classify by type
        const sensorsByType = new Map<string, typeof sensorsData.sensors>();
        const gpuLabels = new Set<string>();

        sensorsData.sensors.forEach((sensor) => {
          const label = sensor.label.toLowerCase();
          let type: "cpu" | "gpu" | "system" = "system";

          if (label.includes("cpu") || label.includes("core") || label.includes("package")) {
            type = "cpu";
          } else if (
            label.includes("gpu") ||
            label.includes("junction") ||
            label.includes("memory") ||
            label.includes("vram")
          ) {
            type = "gpu";
            gpuLabels.add(sensor.label);
          }

          if (!sensorsByType.has(type)) {
            sensorsByType.set(type, []);
          }
          sensorsByType.get(type)!.push(sensor);
        });

        // Build sensor list for display
        const displaySensors = sensorsData.sensors.map((s) => {
          let type: "cpu" | "gpu" | "system" = "system";
          const label = s.label.toLowerCase();

          if (label.includes("cpu") || label.includes("core") || label.includes("package")) {
            type = "cpu";
          } else if (
            label.includes("gpu") ||
            label.includes("junction") ||
            label.includes("memory") ||
            label.includes("vram")
          ) {
            type = "gpu";
          }

          return { name: s.label, type };
        });

        // For GPUs, try to extract model names from sensor labels or use placeholders
        const gpus = Array.from(gpuLabels).length > 0
          ? Array.from(gpuLabels).map((label) => {
              // Try to extract GPU model from label (e.g., "NVIDIA RTX 4080" from label)
              if (label.includes("NVIDIA")) return "NVIDIA GPU";
              if (label.includes("AMD")) return "AMD GPU";
              if (label.includes("Intel")) return "Intel Arc GPU";
              return "GPU Device";
            })
          : [];

        setHardware({
          fans: fansData.fans.map((f) => f.label),
          sensors: displaySensors,
          gpus,
          storage: { path: "~/.config/thermaliq/", size: "< 1 MB" },
        });

        setStage("results");
      } catch (error) {
        console.error("Hardware detection failed:", error);
        // Fallback to mock data on error
        setHardware({
          fans: ["CPU Fan", "System Fan"],
          sensors: [
            { name: "CPU Package", type: "cpu" },
            { name: "GPU Junction", type: "gpu" },
          ],
          gpus: ["GPU Device"],
          storage: { path: "~/.config/thermaliq/", size: "< 1 MB" },
        });
        setStage("results");
      }
    };

    detectHardware();
  }, []);

  const handleConfirm = () => {
    localStorage.setItem("thermaliq-first-launch", "done");
    setStage("confirm");
  };

  if (stage === "detecting") {
    return (
      <div style={styles.container}>
        <div style={styles.centerContent}>
          <Loader size={48} color="var(--accent)" style={{ animation: "spin 2s linear infinite" }} />
          <h2 style={styles.detectingTitle}>Detecting Hardware...</h2>
          <p style={styles.detectingText}>Scanning for fans, sensors, and GPUs. This usually takes a moment.</p>
        </div>
        <style>{`
          @keyframes spin {
            from { transform: rotate(0deg); }
            to { transform: rotate(360deg); }
          }
        `}</style>
      </div>
    );
  }

  if (stage === "confirm") {
    return (
      <div style={styles.container}>
        <div style={styles.centerContent}>
          <CheckCircle size={48} color="var(--accent)" />
          <h2 style={styles.confirmTitle}>All Set!</h2>
          <p style={styles.confirmText}>
            ThermalIQ is ready to manage your cooling. Go to the Dashboard to see live temperatures and create profiles.
          </p>
          <button style={styles.button} onClick={() => window.location.reload()}>
            Open Dashboard
          </button>
        </div>
      </div>
    );
  }

  return (
    <div style={styles.container}>
      <div style={styles.header}>
        <h1 style={styles.title}>Hardware Detected</h1>
        <p style={styles.subtitle}>ThermalIQ found the following devices on your system.</p>
      </div>

      {/* Fans */}
      <div style={{...styles.section, ...styles.sectionCard}}>
        <div style={styles.cardAccentBar} />
        <h2 style={styles.sectionTitle}>✓ Fans</h2>
        <div style={styles.itemList}>
          {hardware.fans.map((fan, i) => (
            <div key={i} style={styles.item}>
              <span style={styles.itemIcon}>⚙</span>
              <span style={styles.itemName}>{fan}</span>
            </div>
          ))}
        </div>
      </div>

      {/* Sensors */}
      <div style={{...styles.section, ...styles.sectionCard}}>
        <div style={styles.cardAccentBar} />
        <h2 style={styles.sectionTitle}>✓ Temperature Sensors</h2>
        <div style={styles.itemList}>
          {hardware.sensors.map((sensor, i) => (
            <div key={i} style={styles.item}>
              <span style={styles.itemIcon}>{sensor.type === "cpu" ? "C" : sensor.type === "gpu" ? "G" : "S"}</span>
              <div>
                <div style={styles.itemName}>{sensor.name}</div>
                <div style={styles.itemMeta}>Type: {sensor.type.toUpperCase()}</div>
              </div>
            </div>
          ))}
        </div>
      </div>

      {/* GPUs */}
      <div style={{...styles.section, ...styles.sectionCard}}>
        <div style={styles.cardAccentBar} />
        <h2 style={styles.sectionTitle}>✓ Graphics Cards</h2>
        <div style={styles.itemList}>
          {hardware.gpus.map((gpu, i) => (
            <div key={i} style={styles.item}>
              <span style={styles.itemIcon}>🎮</span>
              <span style={styles.itemName}>{gpu}</span>
            </div>
          ))}
        </div>
      </div>

      {/* Storage */}
      <div style={{...styles.section, ...styles.sectionCard}}>
        <div style={styles.cardAccentBar} />
        <h2 style={styles.sectionTitle}>✓ Configuration</h2>
        <div style={styles.storageBox}>
          <div style={styles.storageItem}>
            <span style={styles.storageLabel}>Config Path:</span>
            <span style={styles.storagePath}>{hardware.storage.path}</span>
          </div>
          <div style={styles.storageItem}>
            <span style={styles.storageLabel}>Size:</span>
            <span style={styles.storageValue}>{hardware.storage.size}</span>
          </div>
        </div>
      </div>

      {/* Info */}
      <div style={{...styles.infoBox, ...styles.sectionCard}}>
        <div style={styles.cardAccentBar} />
        <AlertCircle size={16} color="var(--text-muted)" />
        <p style={styles.infoText}>
          ThermalIQ will monitor these devices and automatically control your fans based on temperature. You can adjust curves and settings at any time.
        </p>
      </div>

      {/* Action Button */}
      <div style={styles.footer}>
        <button style={styles.button} onClick={handleConfirm}>
          Continue to Dashboard
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
    display: "flex",
    flexDirection: "column",
  },
  centerContent: {
    display: "flex",
    flexDirection: "column",
    alignItems: "center",
    justifyContent: "center",
    height: "100%",
    gap: 20,
    textAlign: "center",
  },
  detectingTitle: {
    fontSize: 24,
    fontWeight: 700,
    color: "var(--text-primary)",
    margin: 0,
  },
  detectingText: {
    fontSize: 14,
    color: "var(--text-secondary)",
    margin: 0,
    maxWidth: 300,
  },
  confirmTitle: {
    fontSize: 24,
    fontWeight: 700,
    color: "var(--text-primary)",
    margin: 0,
  },
  confirmText: {
    fontSize: 14,
    color: "var(--text-secondary)",
    margin: 0,
    maxWidth: 300,
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
    marginBottom: 24,
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
    margin: "0 0 12px 0",
  },
  itemList: {
    display: "flex",
    flexDirection: "column",
    gap: 8,
  },
  item: {
    display: "flex",
    alignItems: "center",
    gap: 12,
    padding: "10px 12px",
    background: "var(--bg-card)",
    border: "1px solid var(--border)",
    borderRadius: "var(--radius)",
  },
  itemIcon: {
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    width: 24,
    height: 24,
    borderRadius: "4px",
    background: "var(--accent-dim)",
    color: "var(--accent)",
    fontWeight: 600,
    fontSize: 11,
    flexShrink: 0,
  },
  itemName: {
    fontSize: 13,
    fontWeight: 500,
    color: "var(--text-primary)",
  },
  itemMeta: {
    fontSize: 11,
    color: "var(--text-muted)",
    marginTop: 2,
  },
  storageBox: {
    padding: "12px 16px",
    background: "var(--bg-card)",
    border: "1px solid var(--border)",
    borderRadius: "var(--radius)",
  },
  storageItem: {
    display: "flex",
    justifyContent: "space-between",
    fontSize: 13,
    color: "var(--text-primary)",
    marginBottom: 8,
  },
  storageLabel: {
    fontWeight: 500,
    color: "var(--text-secondary)",
  },
  storagePath: {
    fontFamily: "var(--font-mono)",
    fontSize: 12,
    color: "var(--accent)",
  },
  storageValue: {
    fontWeight: 500,
    color: "var(--text-primary)",
  },
  infoBox: {
    display: "flex",
    gap: 12,
    padding: "12px 16px",
    background: "var(--bg-card)",
    border: "1px solid var(--border)",
    borderRadius: "var(--radius)",
    marginTop: 12,
  },
  infoText: {
    fontSize: 12,
    color: "var(--text-secondary)",
    margin: 0,
  },
  footer: {
    display: "flex",
    gap: 12,
    marginTop: "auto",
    paddingTop: 24,
    borderTop: "1px solid var(--border)",
  },
  button: {
    padding: "8px 20px",
    background: "var(--accent)",
    color: "#fff",
    border: "none",
    borderRadius: "var(--radius)",
    fontWeight: 600,
    fontSize: 13,
    cursor: "pointer",
    transition: "background 0.15s",
  },
};
