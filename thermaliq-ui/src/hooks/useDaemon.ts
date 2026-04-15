import { useEffect, useRef, useState, useCallback } from "react";

export interface Sensor {
  id: string;
  temp_celsius: number;
  label: string;
}

export interface Fan {
  id: string;
  rpm: number;
  duty_percent: number;
  label: string;
}

interface SensorPayload {
  sensors: Sensor[];
  fans: Fan[];
  ts: string;
}

interface DaemonState {
  sensors: Sensor[];
  fans: Fan[];
  connected: boolean;
}

const WS_URL = "ws://127.0.0.1:7272/ws/sensors";
const MAX_BACKOFF_MS = 5000;

export function useDaemon(): DaemonState {
  const [sensors, setSensors] = useState<Sensor[]>([]);
  const [fans, setFans] = useState<Fan[]>([]);
  const [connected, setConnected] = useState(false);

  const wsRef = useRef<WebSocket | null>(null);
  const backoffRef = useRef(500);
  const retryTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const unmountedRef = useRef(false);

  const connect = useCallback(() => {
    if (unmountedRef.current) return;
    if (wsRef.current?.readyState === WebSocket.OPEN) return; // Already connected

    const ws = new WebSocket(WS_URL);
    wsRef.current = ws;

    ws.onopen = () => {
      if (unmountedRef.current) { ws.close(); return; }
      setConnected(true);
      backoffRef.current = 500;
    };

    ws.onmessage = (event: MessageEvent<string>) => {
      try {
        const payload = JSON.parse(event.data) as SensorPayload;
        setSensors(payload.sensors ?? []);
        setFans(payload.fans ?? []);
      } catch {
        // malformed frame — ignore
      }
    };

    ws.onclose = () => {
      if (unmountedRef.current) return;
      setConnected(false);
      const delay = backoffRef.current;
      backoffRef.current = Math.min(backoffRef.current * 2, MAX_BACKOFF_MS);
      retryTimerRef.current = setTimeout(connect, delay);
    };

    ws.onerror = () => {
      // Don't close immediately; let onclose handler manage reconnection
      // This prevents error spam during development
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

  return { sensors, fans, connected };
}
