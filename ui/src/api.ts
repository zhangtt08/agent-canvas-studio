/** The single RPC bridge. Everything in the app — human clicks and agent calls
 *  alike — goes through `rpc()` and therefore through the same engine entry
 *  point (`rpc::call`), producing the same history entries.
 *
 *  - inside Tauri:      `invoke('rpc', { method, params })`
 *  - plain browser dev: `POST http://127.0.0.1:<port>/rpc` with `{m,p}`
 *  - no port + no Tauri: the dev fixture answers, so the UI is reviewable alone
 */

import type { V } from './types';

declare global {
  interface Window {
    __TAURI__?: unknown;
    __TAURI_INTERNALS__?: unknown;
    /** Written by the Rust initialisation script before any page script runs. */
    __AC_PORT__?: number;
  }
}

export const isTauri =
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

const url = typeof window !== 'undefined' ? new URLSearchParams(window.location.search) : null;

export const PORT: number =
  (typeof window !== 'undefined' && window.__AC_PORT__) || 8899;

export const HTTP_BASE = `http://127.0.0.1:${PORT}`;

/** Fixture answers only outside Tauri, and never when a real port was injected
 *  (`?live=1` forces HTTP, `?mock=1` forces the fixture). */
export const MOCK: boolean =
  !isTauri && !window.__AC_PORT__ ? url?.get('live') !== '1' : url?.get('mock') === '1';

/* ------------------------------------------------------------------ errors */

export class ApiError extends Error {
  code: string;
  msg: string;
  hint?: string;
  details?: unknown;

  constructor(code: string, msg: string, hint?: string, details?: unknown) {
    super(hint ? `${msg} — ${hint}` : msg);
    this.name = 'ApiError';
    this.code = code;
    this.msg = msg;
    this.hint = hint;
    this.details = details;
  }
}

const str = (v: unknown): string | undefined =>
  typeof v === 'string' && v.length > 0 ? v : undefined;

export function toApiError(raw: unknown): ApiError {
  if (raw instanceof ApiError) return raw;
  if (typeof raw === 'string') return new ApiError('rpc', raw);
  if (raw instanceof Error) return new ApiError('rpc', raw.message || String(raw));
  const o = (raw ?? {}) as V;
  return new ApiError(
    str(o.code) ?? 'rpc',
    str(o.e) ?? str(o.msg) ?? str(o.message) ?? '请求失败',
    str(o.hint),
    o.details,
  );
}

type Sink = (err: ApiError, method: string) => void;
let sink: Sink | null = null;
export function setErrorSink(f: Sink | null): void {
  sink = f;
}

/* ------------------------------------------------------------------- calls */

type InvokeFn = <T = unknown>(cmd: string, args?: V) => Promise<T>;
let invokeCache: InvokeFn | null = null;

async function tauriCall<T>(m: string, p: V): Promise<T> {
  if (!invokeCache) {
    const mod = (await import('@tauri-apps/api/core')) as { invoke?: InvokeFn };
    if (typeof mod.invoke !== 'function') throw new ApiError('tauri', 'invoke 不可用');
    invokeCache = mod.invoke;
  }
  return invokeCache<T>('rpc', { method: m, params: p });
}

let fixture: ((m: string, p: V) => Promise<unknown>) | null = null;

/** Answers `{m,p}` with a Promise; rejects with ApiError. */
async function mockCall(m: string, p: V): Promise<unknown> {
  if (!fixture) {
    fixture = (await import('./dev/fixture')).mockCall;
  }
  return fixture(m, p);
}

export interface RpcOpts {
  /** Do not raise a toast for this call (background polls). */
  silent?: boolean;
  /** Reuse an identical in-flight request instead of firing a second one. */
  dedup?: boolean;
}

const inflight = new Map<string, Promise<unknown>>();

export async function rpc<T = V>(m: string, p: V = {}, opts: RpcOpts = {}): Promise<T> {
  const key = `${m}|${JSON.stringify(p)}`;
  if (opts.dedup) {
    const live = inflight.get(key);
    if (live) return live as Promise<T>;
  }

  const run = (async (): Promise<T> => {
    try {
      if (MOCK) return (await mockCall(m, p)) as T;
      if (isTauri) return await tauriCall<T>(m, p);
      let res: Response;
      try {
        res = await fetch(`${HTTP_BASE}/rpc`, {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ m, p }),
        });
      } catch (e) {
        throw new ApiError('offline', '无法连接本地引擎', `${HTTP_BASE}/rpc 未响应 (${String(e)})`);
      }
      const text = await res.text();
      let json: unknown = null;
      if (text) {
        try {
          json = JSON.parse(text);
        } catch {
          if (!res.ok) throw new ApiError('http', `HTTP ${res.status}`, text.slice(0, 200));
          throw new ApiError('json', '响应不是合法 JSON', text.slice(0, 200));
        }
      }
      if (!res.ok) {
        const o = (json ?? {}) as V;
        throw new ApiError(
          str(o.code) ?? 'http',
          str(o.e) ?? `HTTP ${res.status}`,
          str(o.hint),
          o.details,
        );
      }
      return json as T;
    } catch (e) {
      const err = toApiError(e);
      if (!opts.silent && sink) sink(err, m);
      throw err;
    } finally {
      inflight.delete(key);
    }
  })();

  if (opts.dedup) inflight.set(key, run);
  return run;
}

/* ------------------------------------------------------------------- hooks */

import { useCallback, useEffect, useRef, useState } from 'react';

export interface RpcResult<T> {
  data: T | null;
  error: ApiError | null;
  loading: boolean;
  reload: () => void;
}

/** Declarative read with in-flight de-duplication. `p` is compared by value. */
export function useRpc<T = V>(m: string, p: V = {}, enabled = true): RpcResult<T> {
  const json = JSON.stringify(p);
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<ApiError | null>(null);
  const [loading, setLoading] = useState(enabled);
  const [tick, setTick] = useState(0);
  const alive = useRef(true);
  alive.current = true;

  useEffect(() => {
    if (!enabled) {
      setLoading(false);
      return;
    }
    setLoading(true);
    let mine = true;
    rpc<T>(m, JSON.parse(json) as V, { dedup: true, silent: true })
      .then((v) => {
        if (mine) {
          setData(v);
          setError(null);
        }
      })
      .catch((e: unknown) => {
        if (mine) setError(toApiError(e));
      })
      .finally(() => {
        if (mine) setLoading(false);
      });
    return () => {
      mine = false;
    };
  }, [m, json, enabled, tick]);

  const reload = useCallback(() => setTick((t) => t + 1), []);
  return { data, error, loading, reload };
}

/** Imperative call bound to a component; ignores itself after unmount. */
export function useCaller(): {
  run: <T = V>(m: string, p?: V, opts?: RpcOpts) => Promise<T | null>;
  busy: boolean;
} {
  const [busy, setBusy] = useState(false);
  const alive = useRef(true);
  useEffect(
    () => () => {
      alive.current = false;
    },
    [],
  );
  const run = useCallback(async <T,>(m: string, p: V = {}, opts: RpcOpts = {}): Promise<T | null> => {
    setBusy(true);
    try {
      const v = await rpc<T>(m, p, opts);
      return alive.current ? v : null;
    } catch {
      return null;
    } finally {
      if (alive.current) setBusy(false);
    }
  }, []);
  return { run, busy };
}

/* -------------------------------------------------------------------- urls */

/** The engine serves its own assets/fonts/files over loopback HTTP; the preview
 *  SVG references them, so browser and rasteriser load identical bytes. */
export function assetUrl(id: string): string {
  return `${HTTP_BASE}/asset?id=${encodeURIComponent(id)}`;
}

export function fileUrl(path: string): string {
  if (!path) return '';
  if (/^(data:|https?:|blob:)/.test(path)) return path;
  return `${HTTP_BASE}/file?path=${encodeURIComponent(path)}`;
}

/** Turns a `/file?path=…` style url from `render.preview` into an absolute one. */
export function absUrl(u: string): string {
  if (!u) return '';
  if (/^(data:|https?:|blob:)/.test(u)) return u;
  return `${HTTP_BASE}${u.startsWith('/') ? '' : '/'}${u}`;
}

/* ------------------------------------------------------------------- misc */

/** A copy button never uses `navigator.clipboard` blindly (webview policies). */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    /* fall through to the textarea trick */
  }
  try {
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.setAttribute('readonly', '');
    ta.style.position = 'fixed';
    ta.style.opacity = '0';
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand('copy');
    document.body.removeChild(ta);
    return ok;
  } catch {
    return false;
  }
}
