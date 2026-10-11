/** Global app state: reducer + context, the RPC polling loop, toasts and sheets.
 *  Components never call `fetch` — they dispatch through here. */

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  type ReactNode,
} from 'react';
import { ApiError, MOCK, rpc, setErrorSink } from '../api';
import type {
  Caps,
  Document,
  EngineConfig,
  Health,
  Issue,
  Mut,
  Preset,
  PresetWire,
  RecentProject,
  Summary,
  V,
} from '../types';
import { uid } from '../lib/format';
import { wireDoc, type WireDoc } from '../lib/doc';

/** `system.caps` sends size presets as `[key, w, h, label]` tuples. Convert once
 *  here so every component can keep reading `p.k / p.wh / p.d`. */
export function presetsOf(raw: PresetWire[] | undefined): Preset[] {
  return (raw ?? []).map((x) => {
    if (Array.isArray(x)) return { k: String(x[0]), wh: [Number(x[1]), Number(x[2])] as [number, number], d: String(x[3] ?? x[0]) };
    return { k: x.k, wh: [x.wh?.[0] ?? 0, x.wh?.[1] ?? 0] as [number, number], d: x.d ?? x.k };
  });
}

export type Route = 'home' | 'studio' | 'lab' | 'access';
export type LeftTab = 'boards' | 'layers' | 'assets' | 'templates';
export type RightTab = 'style' | 'layout' | 'inspect';

export interface Toast {
  id: string;
  kind: 'ok' | 'err' | 'info';
  text: string;
  code?: string;
}

export interface Sheet {
  name: 'new' | 'export' | 'settings' | 'confirm' | 'template' | 'help' | 'explain' | 'history';
  props?: V;
}

export interface AgentCall {
  m: string;
  p: V;
  ok: boolean;
  summary: string;
  ms: number;
}

export interface AgentMsg {
  id: string;
  from: 'me' | 'ac';
  text?: string;
  plan?: string[];
  calls?: AgentCall[];
  before?: string;
  after?: string;
  error?: string;
}

export interface LabState {
  files: { path: string; name: string; w: number; h: number; bytes: number; fmt: string; url?: string }[];
  active: string | null;
  chain: { op: string; args: V }[];
  result: { out: string; url: string; bytes: number; sha: string; w: number; h: number } | null;
  rows: { i: number; name: string; ok: boolean; out?: string; url?: string; bytes?: number; e?: string }[];
  outDir: string;
  fmt: string;
  q: number;
  split: number;
}

export interface View {
  zoom: number;
  pan: { x: number; y: number };
}

/** Window-only preferences. These are *not* engine settings — `system.config`
 *  has no key for them — so they live in localStorage and are labelled as such
 *  in the settings sheet. Both are read by the canvas overlay. */
export interface UiPrefs {
  snapping: boolean;
  safeArea: boolean;
}
const PREF_KEY = 'agentcanvas.ui.prefs';
const defaultPrefs: UiPrefs = { snapping: true, safeArea: true };

export function loadPrefs(): UiPrefs {
  try {
    const raw = localStorage.getItem(PREF_KEY);
    if (raw) return { ...defaultPrefs, ...(JSON.parse(raw) as UiPrefs) };
  } catch {
    /* private mode / disabled storage: defaults are fine */
  }
  return defaultPrefs;
}

export function savePrefs(p: UiPrefs): void {
  try {
    localStorage.setItem(PREF_KEY, JSON.stringify(p));
  } catch {
    /* ignore */
  }
}

export interface AppState {
  route: Route;
  doc: Document | null;
  pid: string | null;
  board: string | null;
  sel: string[];
  hover: string | null;
  view: View;
  save: 'idle' | 'dirty' | 'saving' | 'saved';
  toasts: Toast[];
  sheets: Sheet[];
  leftTab: LeftTab;
  rightTab: RightTab;
  agent: { open: boolean; size: { w: number; h: number }; thread: AgentMsg[]; running: boolean };
  issues: Issue[];
  issuesBoard: string | null;
  recent: RecentProject[] | null;
  caps: Caps | null;
  health: Health | null;
  /** last `system.config` read; drives the autosave interval and the settings sheet */
  config: EngineConfig | null;
  prefs: UiPrefs;
  fonts: string[];
  lab: LabState;
  /** revision the last successful mutation returned */
  rev: number;
}

const initialLab: LabState = {
  files: [],
  active: null,
  chain: [
    { op: 'crop', args: { ratio: '3:4' } },
    { op: 'scale', args: { by: 0.8 } },
    { op: 'sharpen', args: { amount: 0.7 } },
    { op: 'watermark', args: { text: 'AGENTCANVAS', pos: 'bottom-right', size: 42, opacity: 0.5 } },
  ],
  result: null,
  rows: [],
  outDir: 'out',
  fmt: 'png',
  q: 92,
  split: 50,
};

const initialState: AppState = {
  route: 'home',
  doc: null,
  pid: null,
  board: null,
  sel: [],
  hover: null,
  view: { zoom: 0.45, pan: { x: 0, y: 0 } },
  save: 'idle',
  toasts: [],
  sheets: [],
  leftTab: 'layers',
  rightTab: 'style',
  agent: { open: true, size: { w: 372, h: 424 }, thread: [], running: false },
  issues: [],
  issuesBoard: null,
  recent: null,
  caps: null,
  health: null,
  config: null,
  prefs: loadPrefs(),
  fonts: [],
  lab: initialLab,
  rev: 0,
};

export type Action =
  | { t: 'route'; route: Route }
  | { t: 'doc'; doc: Document; board?: string | null }
  | { t: 'clear-doc' }
  | { t: 'board'; board: string }
  | { t: 'sel'; sel: string[] }
  | { t: 'hover'; hover: string | null }
  | { t: 'view'; view: Partial<View> }
  | { t: 'save'; save: AppState['save'] }
  | { t: 'rev'; rev: number }
  | { t: 'toast'; toast: Toast }
  | { t: 'untoast'; id: string }
  | { t: 'sheet'; sheet: Sheet | null; replace?: boolean }
  | { t: 'left-tab'; tab: LeftTab }
  | { t: 'right-tab'; tab: RightTab }
  | { t: 'agent'; patch: Partial<AppState['agent']> }
  | { t: 'agent-msg'; msg: AgentMsg }
  | { t: 'issues'; issues: Issue[]; board: string | null }
  | { t: 'caps'; caps: Caps | null }
  | { t: 'health'; health: Health | null }
  | { t: 'config'; config: EngineConfig | null }
  | { t: 'prefs'; prefs: Partial<UiPrefs> }
  | { t: 'recent'; recent: RecentProject[] }
  | { t: 'fonts'; fonts: string[] }
  | { t: 'lab'; patch: Partial<LabState> };

function reducer(s: AppState, a: Action): AppState {
  switch (a.t) {
    case 'route':
      return { ...s, route: a.route };
    case 'doc': {
      const first = a.board !== undefined ? a.board : (a.doc.artboards.find((x) => x.visible)?.id ?? null);
      const stillThere = a.doc.nodes.some((n) => n.id === s.sel[0]);
      return {
        ...s,
        doc: a.doc,
        pid: a.doc.project_id || s.pid,
        board: first ?? s.board,
        rev: a.doc.revision,
        sel: stillThere ? s.sel.filter((id) => a.doc.nodes.some((n) => n.id === id)) : [],
        save: a.doc.meta?.dirty ? 'dirty' : s.save === 'dirty' ? s.save : 'saved',
      };
    }
    case 'clear-doc':
      return { ...s, doc: null, pid: null, board: null, sel: [], issues: [], rev: 0, save: 'idle' };
    case 'board':
      return { ...s, board: a.board, sel: [] };
    case 'sel':
      return { ...s, sel: a.sel };
    case 'hover':
      return s.hover === a.hover ? s : { ...s, hover: a.hover };
    case 'view':
      return { ...s, view: { ...s.view, ...a.view } };
    case 'save':
      return { ...s, save: a.save };
    case 'rev':
      return { ...s, rev: a.rev };
    case 'toast':
      return { ...s, toasts: [...s.toasts.slice(-3), a.toast] };
    case 'untoast':
      return { ...s, toasts: s.toasts.filter((x) => x.id !== a.id) };
    case 'sheet': {
      if (a.sheet === null) return { ...s, sheets: [] };
      if (a.replace) return { ...s, sheets: [a.sheet] };
      return { ...s, sheets: [...s.sheets.filter((x) => x.name !== a.sheet!.name), a.sheet] };
    }
    case 'left-tab':
      return { ...s, leftTab: a.tab };
    case 'right-tab':
      return { ...s, rightTab: a.tab };
    case 'agent':
      return { ...s, agent: { ...s.agent, ...a.patch } };
    case 'agent-msg':
      return { ...s, agent: { ...s.agent, thread: [...s.agent.thread, a.msg] } };
    case 'issues':
      return { ...s, issues: a.issues, issuesBoard: a.board };
    case 'caps':
      return { ...s, caps: a.caps ? { ...a.caps, presets: presetsOf(a.caps.presets as unknown as PresetWire[]) } : null };
    case 'health':
      return { ...s, health: a.health };
    case 'config':
      return { ...s, config: a.config };
    case 'prefs': {
      const prefs = { ...s.prefs, ...a.prefs };
      savePrefs(prefs);
      return { ...s, prefs };
    }
    case 'recent':
      return { ...s, recent: a.recent };
    case 'fonts':
      return { ...s, fonts: a.fonts };
    case 'lab':
      return { ...s, lab: { ...s.lab, ...a.patch } };
    default:
      return s;
  }
}

/* ---------------------------------------------------------------- context */

export interface AppApi {
  state: AppState;
  dispatch: (a: Action) => void;
  toast: (kind: Toast['kind'], text: string, code?: string) => void;
  openSheet: (name: Sheet['name'], props?: V) => void;
  closeSheets: () => void;
  go: (route: Route) => void;
  /** Any mutating call: stamps `rev`, then re-reads the document so the window
   *  and the agent always look at the same revision. */
  mutate: <T = Mut>(m: string, p?: V) => Promise<T | null>;
  read: <T = V>(m: string, p?: V, opts?: { silent?: boolean }) => Promise<T | null>;
  loadProject: (p: string, opts?: { route?: Route }) => Promise<Document | null>;
  refresh: () => Promise<Document | null>;
  save: () => Promise<void>;
  select: (ids: string[]) => void;
  setView: (v: Partial<View>) => void;
}

const Ctx = createContext<AppApi | null>(null);

export function useApp(): AppApi {
  const v = useContext(Ctx);
  if (!v) throw new Error('useApp outside provider');
  return v;
}

const FALLBACK_POLL_MS = 60_000;
/** The engine clamps `autosaveSecs` itself, but a hand-edited config file can put
 *  anything there; a 1 ms poll would hang the window. */
const clampMs = (ms: number): number => Math.min(600_000, Math.max(5_000, Math.round(ms) || FALLBACK_POLL_MS));

export function AppProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(reducer, initialState);
  const ref = useRef(state);
  ref.current = state;

  /* the engine owns the autosave cadence — `system.config.autosaveSecs` */
  const pollMs = clampMs((state.config?.autosaveSecs ?? 0) * 1000 || FALLBACK_POLL_MS);

  const toast = useCallback((kind: Toast['kind'], text: string, code?: string) => {
    const id = uid('t');
    dispatch({ t: 'toast', toast: { id, kind, text, code } });
    window.setTimeout(() => dispatch({ t: 'untoast', id }), kind === 'err' ? 6200 : 3200);
  }, []);

  /* every thrown RPC error becomes one glass toast, never inline fine print */
  useEffect(() => {
    setErrorSink((err: ApiError, m: string) => {
      if (err.code === 'REVISION_CONFLICT') {
        toast('info', '画布已被更新，已重新读取', err.code);
        void reload();
        return;
      }
      toast('err', err.hint ? `${err.msg} · ${err.hint}` : `${m} · ${err.msg}`, err.code);
    });
    return () => setErrorSink(null);
  }, []);

  const read = useCallback(
    async <T,>(m: string, p: V = {}, opts: { silent?: boolean } = {}): Promise<T | null> => {
      try {
        return await rpc<T>(m, p, { dedup: !opts.silent, ...opts });
      } catch {
        return null;
      }
    },
    [],
  );

  const refresh = useCallback(async (): Promise<Document | null> => {
    const pid = ref.current.pid;
    if (!pid) return null;
    const raw = await read<WireDoc>('project.get', { p: pid, depth: 1 }, { silent: true });
    const doc = raw && raw.nodes ? wireDoc(raw) : null;
    if (doc) {
      dispatch({ t: 'doc', doc });
    }
    return doc;
  }, [read]);

  const mutate = useCallback(
    async <T = Mut,>(m: string, p: V = {}): Promise<T | null> => {
      const cur = ref.current;
      if (!cur.pid) {
        toast('err', '还没有打开项目', 'NO_PROJECT');
        return null;
      }
      dispatch({ t: 'save', save: 'saving' });
      try {
        const r = (await rpc<T>(m, { p: cur.pid, rev: cur.rev, ...p })) as unknown as Mut;
        if (typeof r.rev === 'number') dispatch({ t: 'rev', rev: r.rev });
        const warnings = r.warnings ?? [];
        if (warnings.length) toast('info', warnings[0], warnings.length > 1 ? `+${warnings.length - 1}` : undefined);
        dispatch({ t: 'save', save: 'dirty' });
        await refresh();
        return r as unknown as T;
      } catch (e) {
        dispatch({ t: 'save', save: cur.doc?.meta?.dirty ? 'dirty' : 'idle' });
        throw e;
      }
    },
    [refresh, toast],
  );

  const loadProject = useCallback(
    async (pid: string, opts: { route?: Route } = {}): Promise<Document | null> => {
      const raw = await read<WireDoc>('project.get', { p: pid, depth: 1 });
      if (!raw || !raw.nodes) return null;
      const doc = wireDoc(raw);
      const board = doc.artboards.find((x) => x.visible)?.id ?? null;
      dispatch({ t: 'doc', doc, board });
      if (opts.route) dispatch({ t: 'route', route: opts.route });
      return doc;
    },
    [read],
  );

  const save = useCallback(async () => {
    const pid = ref.current.pid;
    if (!pid) return;
    dispatch({ t: 'save', save: 'saving' });
    const r = await read<{ path?: string; sha?: string; rev?: number }>('project.save', { p: pid }, { silent: true });
    dispatch({ t: 'save', save: r ? 'saved' : 'dirty' });
    if (r?.sha) toast('ok', `已保存 · ${r.sha.slice(0, 8)}`);
  }, [read, toast]);

  const select = useCallback((ids: string[]) => dispatch({ t: 'sel', sel: ids }), []);
  const setView = useCallback((v: Partial<View>) => dispatch({ t: 'view', view: v }), []);
  const go = useCallback((route: Route) => dispatch({ t: 'route', route }), []);
  const openSheet = useCallback(
    (name: Sheet['name'], props: V = {}) => dispatch({ t: 'sheet', sheet: { name, props } }),
    [],
  );
  const closeSheets = useCallback(() => dispatch({ t: 'sheet', sheet: null }), []);

  /* ------------------------------- boot + engine-driven autosave/poll loop */
  const reload = refresh;

  useEffect(() => {
    void (async () => {
      const [caps, health, config] = await Promise.all([
        read<Caps>('system.caps', {}, { silent: true }),
        read<Health>('system.health', {}, { silent: true }),
        read<EngineConfig>('system.config', {}, { silent: true }),
      ]);
      if (caps) dispatch({ t: 'caps', caps });
      if (health) dispatch({ t: 'health', health });
      if (config) dispatch({ t: 'config', config });
      const list = await read<RecentProject[] | Summary[]>('project.list', {}, { silent: true });
      if (Array.isArray(list)) dispatch({ t: 'recent', recent: list as RecentProject[] });
      const f = await read<{ families: string[] }>('text.fonts', {}, { silent: true });
      if (f?.families) dispatch({ t: 'fonts', fonts: f.families });
    })();
  }, [read]);

  useEffect(() => {
    const timer = window.setInterval(async () => {
      const cur = ref.current;
      if (!cur.pid) return;
      if (cur.save === 'dirty') await save();
      const head = await read<{ rev: number }>('project.get', { p: cur.pid, depth: 0 }, { silent: true });
      if (head && typeof head.rev === 'number' && head.rev !== cur.rev) {
        await reload();
        toast('info', '本地引擎更新了画布', `rev ${head.rev}`);
      }
    }, pollMs);
    return () => window.clearInterval(timer);
  }, [pollMs, read, reload, save, toast]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') dispatch({ t: 'sheet', sheet: null });
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const api = useMemo<AppApi>(
    () => ({
      state,
      dispatch,
      toast,
      openSheet,
      closeSheets,
      go,
      mutate,
      read,
      loadProject,
      refresh,
      save,
      select,
      setView,
    }),
    [state, toast, openSheet, closeSheets, go, mutate, read, loadProject, refresh, save, select, setView],
  );

  return <Ctx.Provider value={api}>{children}</Ctx.Provider>;
}

export const isMock = MOCK;
