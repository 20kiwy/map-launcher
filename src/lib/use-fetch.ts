"use client";

import { useCallback, useEffect, useRef, useState } from "react";

export interface FetchState<T> {
  data: T | null;
  error: string | null;
  loading: boolean;
}

export interface FetchResult<T> extends FetchState<T> {
  retry: () => void;
}

/**
 * Minimal GET-with-retry hook.
 *
 * All state transitions happen in async continuations or event handlers
 * (never synchronously inside the effect body). In-flight requests are
 * aborted on unmount / url change; `retry` re-runs the request.
 */
export function useFetch<T>(url: string): FetchResult<T> {
  const [state, setState] = useState<FetchState<T>>({
    data: null,
    error: null,
    loading: true,
  });
  const [nonce, setNonce] = useState(0);
  const abortRef = useRef<AbortController | null>(null);

  useEffect(() => {
    const ac = new AbortController();
    abortRef.current = ac;
    let cancelled = false;

    const run = async (): Promise<void> => {
      try {
        const res = await fetch(url, { signal: ac.signal });
        if (!res.ok) throw new Error(`HTTP ${res.status} from ${url}`);
        const data = (await res.json()) as T;
        if (!cancelled && !ac.signal.aborted) {
          setState({ data, error: null, loading: false });
        }
      } catch (err: unknown) {
        if (cancelled || ac.signal.aborted) return;
        if (err instanceof DOMException && err.name === "AbortError") return;
        setState({
          data: null,
          error: err instanceof Error ? err.message : "request failed",
          loading: false,
        });
      }
    };

    void run();

    return () => {
      cancelled = true;
      ac.abort();
    };
  }, [url, nonce]);

  const retry = useCallback(() => {
    setState((s) => ({ data: s.data, error: null, loading: true }));
    setNonce((n) => n + 1);
  }, []);

  return { ...state, retry };
}
