import { useTranslation } from "react-i18next";
import type { UpdateInfo, UpdaterError } from "../hooks/useUpdater";

interface UpdateBannerProps {
  updateAvailable: UpdateInfo | null;
  installing: boolean;
  error: UpdaterError | null;
  onInstall: () => void;
  onDismiss: () => void;
}

const CHANGELOG_BASE = "https://github.com/lulacoding/thermaliq/releases/tag/v";

export default function UpdateBanner({
  updateAvailable,
  installing,
  error,
  onInstall,
  onDismiss,
}: UpdateBannerProps) {
  const { t } = useTranslation();

  if (error) {
    if (error.kind === "network") return null; // silent

    const msg =
      error.kind === "signature"
        ? t("updater.errorSignature")
        : error.kind === "permissions"
        ? t("updater.errorPermissions")
        : t("updater.errorUnknown", { message: (error as { kind: "unknown"; message: string }).message });

    return (
      <div style={{ ...styles.banner, ...styles.bannerError }}>
        <span style={styles.text}>{msg}</span>
        <button style={styles.btnClose} onClick={onDismiss} aria-label={t("common.close")}>
          ×
        </button>
      </div>
    );
  }

  if (!updateAvailable) return null;

  return (
    <div style={styles.banner}>
      <span style={styles.text}>
        {t("updater.available", { version: updateAvailable.version })}
      </span>
      <div style={styles.actions}>
        <a
          href={`${CHANGELOG_BASE}${updateAvailable.version}`}
          target="_blank"
          rel="noreferrer"
          style={styles.linkBtn}
        >
          {t("updater.viewChangelog")}
        </a>
        <button
          style={{ ...styles.btn, ...(installing ? styles.btnDisabled : {}) }}
          onClick={onInstall}
          disabled={installing}
        >
          {installing ? t("updater.installing") : t("updater.installNow")}
        </button>
        <button style={styles.btnSecondary} onClick={onDismiss} disabled={installing}>
          {t("updater.later")}
        </button>
      </div>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  banner: {
    position: "fixed",
    bottom: 16,
    right: 16,
    zIndex: 1000,
    display: "flex",
    alignItems: "center",
    gap: 12,
    padding: "10px 14px",
    background: "var(--bg-card)",
    border: "1px solid var(--accent)",
    borderRadius: "var(--radius)",
    boxShadow: "0 4px 16px rgba(0,0,0,0.4)",
    maxWidth: 480,
  },
  bannerError: {
    borderColor: "rgba(255, 87, 51, 0.6)",
  },
  text: {
    fontSize: 13,
    color: "var(--text-primary)",
    flex: 1,
  },
  actions: {
    display: "flex",
    gap: 8,
    alignItems: "center",
    flexShrink: 0,
  },
  btn: {
    padding: "5px 12px",
    background: "var(--accent)",
    color: "#000",
    border: "none",
    borderRadius: "var(--radius-sm)",
    fontWeight: 600,
    fontSize: 12,
    cursor: "pointer",
  },
  btnDisabled: {
    opacity: 0.5,
    cursor: "not-allowed",
  },
  btnSecondary: {
    padding: "5px 12px",
    background: "transparent",
    color: "var(--text-secondary)",
    border: "1px solid var(--border)",
    borderRadius: "var(--radius-sm)",
    fontSize: 12,
    cursor: "pointer",
  },
  btnClose: {
    background: "transparent",
    border: "none",
    color: "var(--text-muted)",
    fontSize: 18,
    cursor: "pointer",
    lineHeight: 1,
    padding: "0 4px",
  },
  linkBtn: {
    fontSize: 12,
    color: "var(--accent)",
    textDecoration: "underline",
    cursor: "pointer",
  },
};
