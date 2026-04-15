import { useEffect, useState } from "react";
import { readTextFile, BaseDirectory } from "@tauri-apps/plugin-fs";

interface TokenState {
  token: string | null;
  loading: boolean;
  error: string | null;
}

const TOKEN_PATH = ".config/thermaliq/api.token";

export function useToken(): TokenState {
  const [token, setToken] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    readTextFile(TOKEN_PATH, { baseDir: BaseDirectory.Home })
      .then((contents) => {
        if (!cancelled) {
          setToken(contents.trim());
          setLoading(false);
        }
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
          setLoading(false);
        }
      });

    return () => {
      cancelled = true;
    };
  }, []);

  return { token, loading, error };
}
