import { useState, useEffect } from "react";
import { CheckCircle, Zap, AlertCircle } from "lucide-react";
import { useToken } from "../hooks/useToken";

type Step = "detect" | "spinning" | "results" | "confirm";

interface Fan {
  name: string;
  minRpm: number;
  maxRpm: number;
}

const API_BASE = "http://127.0.0.1:7272";

export default function CalibrationWizard() {
  const { token } = useToken();
  const [step, setStep] = useState<Step>("detect");
  const [fans, setFans] = useState<Fan[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [availableFans, setAvailableFans] = useState<
    Array<{ id: string; label: string }>
  >([]);

  useEffect(() => {
    const fetchFans = async () => {
      try {
        const res = await fetch(`${API_BASE}/fans`);
        if (!res.ok) throw new Error("Failed to fetch fans");
        const data = (await res.json()) as {
          fans: Array<{ id: string; label: string }>;
        };
        setAvailableFans(data.fans);
      } catch (err) {
        console.error("Failed to fetch fans:", err);
        setError(err instanceof Error ? err.message : "Failed to load fans");
      }
    };
    fetchFans();
  }, []);

  const handleStartCalibration = async () => {
    if (!token) {
      setError("No API token — cannot run calibration");
      return;
    }

    setError(null);
    setStep("spinning");
    setFans([]);

    try {
      // Calibrate each fan sequentially
      const results: Fan[] = [];

      for (const fan of availableFans) {
        try {
          // Run calibration (takes ~42 seconds, set 60s timeout)
          const controller = new AbortController();
          const timeoutId = setTimeout(() => controller.abort(), 60000);

          const calRes = await fetch(`${API_BASE}/api/calibration/${fan.id}/run`, {
            method: "POST",
            headers: {
              "Content-Type": "application/json",
              Authorization: `Bearer ${token}`,
            },
            signal: controller.signal,
          });
          clearTimeout(timeoutId);

          if (!calRes.ok) {
            throw new Error(`Calibration failed for ${fan.label}`);
          }

          const calData = (await calRes.json()) as {
            fan_id: string;
            start_percent: number;
            stop_percent: number;
            min_rpm: number;
            max_rpm: number;
          };

          results.push({
            name: fan.label,
            minRpm: calData.min_rpm,
            maxRpm: calData.max_rpm,
          });
        } catch (err) {
          console.error(`Calibration error for ${fan.label}:`, err);
          setError(
            err instanceof Error ? err.message : "Calibration failed"
          );
          setStep("detect");
          return;
        }
      }

      setFans(results);
      setStep("results");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Calibration failed");
      setStep("detect");
    }
  };

  const handleConfirm = () => {
    setStep("confirm");
  };

  return (
    <div style={styles.container}>
      <div style={styles.header}>
        <div style={styles.headerContent}>
          <h1 style={styles.title}>Fan Calibration</h1>
          <p style={styles.subtitle}>
            Let ThermalIQ learn your fan capabilities for optimal thermal control
          </p>
        </div>
      </div>

      <div style={styles.wizard}>
        {error && (
          <div style={styles.errorBanner}>
            <AlertCircle size={16} />
            <span>{error}</span>
          </div>
        )}

        {step === "detect" && (
          <div style={styles.stepContent}>
            <div style={styles.stepIcon}>
              <Zap size={32} color="var(--accent)" />
            </div>
            <h2 style={styles.stepTitle}>Detect Connected Fans</h2>
            <p style={styles.stepDescription}>
              This wizard will scan your system and measure the RPM range of each connected fan.
              It takes about 2-3 minutes.
            </p>
            <button
              style={styles.primaryButton}
              onClick={handleStartCalibration}
              disabled={availableFans.length === 0}
            >
              {availableFans.length === 0 ? "Loading fans..." : "Start Calibration"}
            </button>
          </div>
        )}

        {step === "spinning" && (
          <div style={styles.stepContent}>
            <div style={styles.spinnerContainer}>
              <div style={styles.spinner}></div>
            </div>
            <h2 style={styles.stepTitle}>Calibrating Fans...</h2>
            <p style={styles.stepDescription}>
              Testing fan speeds. This may sound loud—it's normal.
            </p>
          </div>
        )}

        {step === "results" && (
          <div style={styles.stepContent}>
            <div style={styles.stepIcon}>
              <CheckCircle size={32} color="var(--accent)" />
            </div>
            <h2 style={styles.stepTitle}>Calibration Results</h2>

            <div style={styles.fansList}>
              {fans.map((fan) => (
                <div key={fan.name} style={styles.fanCard}>
                  <div style={styles.fanCardAccentBar} />
                  <div style={styles.fanName}>{fan.name}</div>
                  <div style={styles.fanRange}>
                    <span style={styles.fanRangeLabel}>Range:</span>
                    <span style={styles.fanRangeValue}>
                      {fan.minRpm} – {fan.maxRpm} RPM
                    </span>
                  </div>
                </div>
              ))}
            </div>

            <button style={styles.primaryButton} onClick={handleConfirm}>
              Apply & Continue
            </button>
            <button style={styles.secondaryButton} onClick={() => setStep("detect")}>
              Re-calibrate
            </button>
          </div>
        )}

        {step === "confirm" && (
          <div style={styles.stepContent}>
            <div style={styles.stepIcon}>
              <CheckCircle size={32} color="var(--green)" />
            </div>
            <h2 style={styles.stepTitle}>Calibration Complete!</h2>
            <p style={styles.stepDescription}>
              Your fans are now calibrated. ThermalIQ will use this data to generate optimal fan curves.
            </p>
            <div style={styles.nextSteps}>
              <h3 style={styles.nextStepsTitle}>Next Steps:</h3>
              <ul style={styles.nextStepsList}>
                <li>Check the Dashboard to see your sensor readings</li>
                <li>Create or customize fan curves in Profiles</li>
                <li>Enable AI agent for autonomous thermal management</li>
              </ul>
            </div>
            <button style={styles.primaryButton} onClick={() => setStep("detect")}>
              Done
            </button>
          </div>
        )}
      </div>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  container: {
    padding: "24px",
    height: "100%",
    display: "flex",
    flexDirection: "column",
  },
  header: {
    marginBottom: 32,
  },
  headerContent: {
    maxWidth: 800,
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
  wizard: {
    flex: 1,
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
  },
  stepContent: {
    textAlign: "center",
    maxWidth: 500,
  },
  stepIcon: {
    marginBottom: 24,
  },
  stepTitle: {
    fontSize: 24,
    fontWeight: 700,
    color: "var(--text-primary)",
    margin: 0,
    marginBottom: 8,
  },
  stepDescription: {
    fontSize: 13,
    color: "var(--text-secondary)",
    margin: 0,
    marginBottom: 24,
    lineHeight: 1.6,
  },
  spinnerContainer: {
    marginBottom: 24,
    display: "flex",
    justifyContent: "center",
  },
  spinner: {
    width: 48,
    height: 48,
    border: "3px solid var(--accent-dim)",
    borderTop: "3px solid var(--accent)",
    borderRadius: "50%",
    animation: "spin 1s linear infinite",
  },
  fansList: {
    textAlign: "left",
    marginBottom: 24,
  },
  fanCard: {
    padding: 16,
    background: "var(--bg-card)",
    border: "1px solid var(--border)",
    marginBottom: 12,
    borderRadius: "var(--radius)",
    position: "relative",
    overflow: "hidden",
  },
  fanCardAccentBar: {
    position: "absolute",
    bottom: 0,
    left: 0,
    right: 0,
    height: 4,
    background: "var(--accent)",
  },
  fanName: {
    fontSize: 14,
    fontWeight: 600,
    color: "var(--text-primary)",
    marginBottom: 8,
  },
  fanRange: {
    fontSize: 12,
    color: "var(--text-secondary)",
    display: "flex",
    gap: 8,
  },
  fanRangeLabel: {
    color: "var(--text-muted)",
  },
  fanRangeValue: {
    color: "var(--accent)",
    fontWeight: 600,
    fontFamily: "var(--font-mono)",
  },
  primaryButton: {
    padding: "10px 24px",
    background: "var(--accent)",
    color: "#fff",
    border: "none",
    borderRadius: "var(--radius-sm)",
    fontWeight: 600,
    fontSize: 13,
    cursor: "pointer",
    transition: "background 0.15s",
    marginRight: 12,
  },
  secondaryButton: {
    padding: "10px 24px",
    background: "transparent",
    color: "var(--text-secondary)",
    border: "1px solid var(--border)",
    borderRadius: "var(--radius-sm)",
    fontWeight: 600,
    fontSize: 13,
    cursor: "pointer",
    transition: "background 0.15s",
  },
  errorBanner: {
    display: "flex",
    alignItems: "center",
    gap: 12,
    padding: "12px 16px",
    background: "rgba(239, 68, 68, 0.1)",
    border: "1px solid rgba(239, 68, 68, 0.3)",
    borderRadius: "var(--radius-sm)",
    color: "#ef4444",
    fontSize: 13,
    marginBottom: 24,
  },
  nextSteps: {
    textAlign: "left",
    padding: 16,
    background: "var(--accent-dim)",
    borderRadius: "var(--radius)",
    marginBottom: 24,
  },
  nextStepsTitle: {
    fontSize: 12,
    fontWeight: 600,
    color: "var(--accent)",
    margin: "0 0 8px 0",
    textTransform: "uppercase",
    letterSpacing: "0.05em",
  },
  nextStepsList: {
    fontSize: 12,
    color: "var(--text-primary)",
    margin: 0,
    paddingLeft: 16,
    lineHeight: 1.8,
  },
};
