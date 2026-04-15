import { useEffect, useRef, useState, useCallback } from "react";

export type AgentMode = "passive" | "approval" | "autonomous";

interface AgentEvent {
  raw: string;
  ts: number;
}

interface AgentState {
  mode: AgentMode | null;
  pendingCount: number;
  events: AgentEvent[];
}

const WS_URL = "ws://127.0.0.1:7272/ws/agent";
const MAX_BACKOFF_MS = 5000;
const MAX_EVENTS = 100;

export function useAgent(): AgentState {
  const [mode, setMode] = useState<AgentMode | null>(null);
  const [pendingCount, setPendingCount] = useState(0);
  const [events, setEvents] = useState<AgentEvent[]>([]);

  const wsRef = useRef<WebSocket | null>(null);
  const backoffRef = useRef(500);
  const retryTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const unmountedRef = useRef(false);

  const connect = useCallback(() => {
    if (unmountedRef.current) return;

    const ws = new WebSocket(WS_URL);
    wsRef.current = ws;

    ws.onopen = () => {
      if (unmountedRef.current) { ws.close(); return; }
      backoffRef.current = 500;
    };

    ws.onmessage = (event: MessageEvent<string>) => {
      try {
        const payload = JSON.parse(event.data) as {
          mode?: AgentMode;
          pending_count?: number;
        };
        if (payload.mode !== undefined) setMode(payload.mode);
        if (payload.pending_count !== undefined) setPendingCount(payload.pending_count);
        setEvents((prev) => {
          const next: AgentEvent[] = [{ raw: event.data, ts: Date.now() }, ...prev];
          return next.slice(0, MAX_EVENTS);
        });
      } catch {
        // malformed frame — ignore
      }
    };

    ws.onclose = () => {
      if (unmountedRef.current) return;
      const delay = backoffRef.current;
      backoffRef.current = Math.min(backoffRef.current * 2, MAX_BACKOFF_MS);
      retryTimerRef.current = setTimeout(connect, delay);
    };

    ws.onerror = () => {
      ws.close();
    };
  }, []);

  useEffect(() => {
    unmountedRef.current = false;
    connect();

    return () => {
      unmountedRef.current = true;
      if (retryTimerRef.current !== null) {
        clearTimeout(retryTimerRef.current);
      }
      wsRef.current?.close();
    };
  }, [connect]);

  return { mode, pendingCount, events };
}
