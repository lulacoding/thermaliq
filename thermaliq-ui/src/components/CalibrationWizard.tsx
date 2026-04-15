import { useState } from "react";
import { CheckCircle, Zap } from "lucide-react";

type Step = "detect" | "spinning" | "results" | "confirm";

export default function CalibrationWizard() {
  const [step, setStep] = useState<Step>("detect");
  const [fans, setFans] = useState<{ name: string; minRpm: number; maxRpm: number }[]>([]);

  const handleStartCalibration = () => {
    setFans([
      { name: "CPU Fan", minRpm: 800, maxRpm: 3600 },
      { name: "Chassis Fan 1", minRpm: 500, maxRpm: 2400 },
    ]);
    setStep("spinning");
    setTimeout(() => setStep("results"), 3000);
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
            <button style={styles.primaryButton} onClick={handleStartCalibration}>
              Start Calibration
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
