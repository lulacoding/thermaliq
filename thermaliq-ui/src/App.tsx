import { useState } from "react";
import {
  LayoutDashboard,
  Bot,
  Layers,
  Bell,
  Thermometer,
} from "lucide-react";
import Dashboard from "./components/Dashboard";
import AgentPanel from "./components/AgentPanel";
import ProfileManager from "./components/ProfileManager";
import AlertCenter from "./components/AlertCenter";
import "./index.css";

type View = "dashboard" | "agent" | "profiles" | "alerts";

interface NavItem {
  id: View;
  label: string;
  icon: React.ReactNode;
}

const NAV_ITEMS: NavItem[] = [
  { id: "dashboard", label: "Dashboard", icon: <LayoutDashboard size={16} /> },
  { id: "agent", label: "Agent", icon: <Bot size={16} /> },
  { id: "profiles", label: "Profiles", icon: <Layers size={16} /> },
  { id: "alerts", label: "Alerts", icon: <Bell size={16} /> },
];

export default function App() {
  const [activeView, setActiveView] = useState<View>("dashboard");

  return (
    <div style={styles.root}>
      <aside style={styles.sidebar}>
        <div style={styles.logo}>
          <Thermometer size={20} color="var(--accent)" />
          <span style={styles.logoText}>ThermalIQ</span>
        </div>

        <nav style={styles.nav}>
          {NAV_ITEMS.map((item) => (
            <button
              key={item.id}
              onClick={() => setActiveView(item.id)}
              style={{
                ...styles.navItem,
                ...(activeView === item.id ? styles.navItemActive : {}),
              }}
            >
              <span style={styles.navIcon}>{item.icon}</span>
              {item.label}
            </button>
          ))}
        </nav>

        <div style={styles.sidebarFooter}>
          <span style={styles.footerText}>v0.1.0</span>
        </div>
      </aside>

      <main style={styles.main}>
        {activeView === "dashboard" && <Dashboard />}
        {activeView === "agent" && <AgentPanel />}
        {activeView === "profiles" && <ProfileManager />}
        {activeView === "alerts" && <AlertCenter />}
      </main>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  root: {
    display: "flex",
    height: "100vh",
    width: "100vw",
    background: "var(--bg-primary)",
    overflow: "hidden",
  },
  sidebar: {
    width: 200,
    minWidth: 200,
    background: "var(--bg-secondary)",
    borderRight: "1px solid var(--border)",
    display: "flex",
    flexDirection: "column",
    padding: "0 0 12px 0",
  },
  logo: {
    display: "flex",
    alignItems: "center",
    gap: 10,
    padding: "18px 16px 16px",
    borderBottom: "1px solid var(--border)",
    marginBottom: 8,
  },
  logoText: {
    fontSize: 15,
    fontWeight: 700,
    color: "var(--text-primary)",
    letterSpacing: "-0.02em",
  },
  nav: {
    display: "flex",
    flexDirection: "column",
    gap: 2,
    padding: "0 8px",
    flex: 1,
  },
  navItem: {
    display: "flex",
    alignItems: "center",
    gap: 9,
    padding: "8px 10px",
    borderRadius: "var(--radius-sm)",
    background: "transparent",
    color: "var(--text-secondary)",
    fontSize: 13,
    fontWeight: 500,
    cursor: "pointer",
    border: "none",
    textAlign: "left",
    transition: "background 0.12s, color 0.12s",
  },
  navItemActive: {
    background: "var(--accent-dim)",
    color: "var(--accent)",
  },
  navIcon: {
    display: "flex",
    alignItems: "center",
  },
  sidebarFooter: {
    padding: "8px 16px 0",
    borderTop: "1px solid var(--border)",
    marginTop: 8,
  },
  footerText: {
    fontSize: 11,
    color: "var(--text-muted)",
    fontFamily: "var(--font-mono)",
  },
};
