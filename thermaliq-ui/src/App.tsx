import { useState } from "react";
import {
  LayoutDashboard,
  Settings,
  Layers,
  Zap,
  Cpu,
  LineChart,
  Minus,
  Square,
  X,
  Bot,
} from "lucide-react";
import Dashboard from "./components/Dashboard";
import ProfileManager from "./components/ProfileManager";
import CurveEditor from "./components/CurveEditor";
import SettingsPanel from "./components/SettingsPanel";
import CalibrationWizard from "./components/CalibrationWizard";
import HardwareDetect from "./components/HardwareDetect";
import AgentPanel from "./components/AgentPanel";
import { useNotifications } from "./hooks/useNotifications";
import { useNotificationSettings } from "./hooks/useNotificationSettings";
import "./index.css";

type View = "dashboard" | "profiles" | "curves" | "settings" | "calibrate" | "hardware" | "agent";

interface NavItem {
  id: View;
  icon: React.ReactNode;
}

const NAV_ITEMS: NavItem[] = [
  { id: "dashboard", icon: <LayoutDashboard size={16} /> },
  { id: "profiles", icon: <Layers size={16} /> },
  { id: "curves", icon: <LineChart size={16} /> },
  { id: "hardware", icon: <Cpu size={16} /> },
  { id: "agent", icon: <Bot size={16} /> },
  { id: "settings", icon: <Settings size={16} /> },
  { id: "calibrate", icon: <Zap size={16} /> },
];

export default function App() {
  const [activeView, setActiveView] = useState<View>("dashboard");
  const { settings: notifSettings } = useNotificationSettings();
  useNotifications(notifSettings);

  return (
    <div style={styles.root}>
      {/* Fixed Top Bar */}
      <header style={styles.topBar}>
        <div style={styles.topBarLeft}>
          <span style={styles.topBarTitle}>THERMALIQ</span>
          <div className="pulse-dot" />
        </div>
        <div style={styles.windowControls}>
          <button style={styles.windowButton} onClick={() => {}}>
            <Minus size={14} />
          </button>
          <button style={styles.windowButton} onClick={() => {}}>
            <Square size={14} />
          </button>
          <button style={{...styles.windowButton, color: 'var(--red)'}} onClick={() => {}}>
            <X size={14} />
          </button>
        </div>
      </header>

      {/* Icon-Only Sidebar */}
      <aside style={styles.sidebar}>
        <div style={styles.sidebarLogo}>
          <span style={styles.logoText}>TIQ</span>
        </div>

        <nav style={styles.nav}>
          {NAV_ITEMS.map((item) => (
            <button
              key={item.id}
              onClick={() => setActiveView(item.id)}
              title={item.id}
              style={{
                ...styles.navItem,
                ...(activeView === item.id ? styles.navItemActive : {}),
              }}
            >
              <span style={styles.navIcon}>{item.icon}</span>
            </button>
          ))}
        </nav>

        <div style={styles.sidebarFooter}>
          <span style={styles.footerText}>v1.0.0</span>
        </div>
      </aside>

      {/* Main Content */}
      <main style={styles.main}>
        {activeView === "dashboard" && <Dashboard />}
        {activeView === "profiles" && <ProfileManager />}
        {activeView === "curves" && <CurveEditor />}
        {activeView === "hardware" && <HardwareDetect />}
        {activeView === "agent" && <AgentPanel />}
        {activeView === "settings" && <SettingsPanel />}
        {activeView === "calibrate" && <CalibrationWizard />}
      </main>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  root: {
    display: "flex",
    flexDirection: "column",
    height: "100vh",
    width: "100vw",
    background: "var(--bg-void)",
    overflow: "hidden",
  },
  topBar: {
    position: "fixed",
    top: 0,
    left: 0,
    right: 0,
    height: "var(--top-bar-h)",
    zIndex: 50,
    background: "var(--bg-void)",
    borderBottom: "1px solid var(--border)",
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    paddingLeft: 16,
    paddingRight: 12,
  },
  topBarLeft: {
    display: "flex",
    alignItems: "center",
    gap: 12,
  },
  topBarTitle: {
    fontSize: 13,
    fontWeight: 900,
    color: "var(--text-primary)",
    letterSpacing: "0.05em",
  },
  windowControls: {
    display: "flex",
    gap: 8,
  },
  windowButton: {
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    width: 28,
    height: 28,
    background: "transparent",
    color: "var(--text-secondary)",
    border: "1px solid transparent",
    cursor: "pointer",
    transition: "color 0.15s",
  },
  sidebar: {
    position: "fixed",
    left: 0,
    top: "var(--top-bar-h)",
    bottom: 0,
    width: "var(--sidebar-w)",
    background: "var(--bg-void)",
    borderRight: "1px solid var(--border)",
    display: "flex",
    flexDirection: "column",
    padding: "8px 0",
    zIndex: 40,
  },
  sidebarLogo: {
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    height: 48,
    borderBottom: "1px solid var(--border)",
  },
  logoText: {
    fontSize: 11,
    fontWeight: 900,
    color: "var(--accent)",
    letterSpacing: "0.05em",
  },
  nav: {
    display: "flex",
    flexDirection: "column",
    gap: 2,
    padding: "4px 6px",
    flex: 1,
  },
  navItem: {
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    width: 36,
    height: 36,
    borderRadius: "var(--radius-sm)",
    background: "transparent",
    color: "var(--text-muted)",
    cursor: "pointer",
    border: "none",
    transition: "color 0.12s, background 0.12s, border-left 0.12s",
    borderLeft: "2px solid transparent",
  },
  navItemActive: {
    borderLeft: "2px solid var(--accent)",
    background: "rgba(0, 255, 65, 0.10)",
    color: "var(--accent)",
  },
  navIcon: {
    display: "flex",
    alignItems: "center",
  },
  sidebarFooter: {
    padding: "4px 6px 0",
    borderTop: "1px solid var(--border)",
    display: "flex",
    justifyContent: "center",
  },
  footerText: {
    fontSize: 8,
    color: "var(--text-muted)",
    fontFamily: "var(--font-mono)",
  },
  main: {
    marginLeft: "var(--sidebar-w)",
    marginTop: "var(--top-bar-h)",
    height: "calc(100vh - var(--top-bar-h))",
    overflow: "auto",
  },
};
