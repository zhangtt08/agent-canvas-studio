/** Dev fixture: a small, honest in-memory engine so the UI is reviewable without
 *  the Rust side. Enabled only outside Tauri with no injected port (see api.ts).
 *
 *  It keeps real `Document` shapes, real revisions, real undo/redo stacks, and it
 *  compiles the same kind of SVG paint path (`render.svg` / `render.preview`) so
 *  the canvas, inspector and export sheets are populated with genuine data.
 *  It is never loaded inside Tauri — `api.ts` imports it lazily and only when the
 *  fixture conditions hold. */

import { ApiError } from '../api';
import type {
  Artboard,
  Asset,
  Document,
  Intent,
  Issue,
  Node,
  NodeType,
  Paint,
  Rect,
  V,
} from '../types';

/* ------------------------------------------------------------------ utils */

const clone = <T,>(v: T): T => JSON.parse(JSON.stringify(v)) as T;
const wait = (ms = 24) => new Promise((r) => setTimeout(r, ms));
const rid = (p: string) => `${p}_${Math.random().toString(36).slice(2, 7)}`;
const num = (v: unknown, d = 0): number => (typeof v === 'number' && Number.isFinite(v) ? v : d);
const str = (v: unknown, d = ''): string => (typeof v === 'string' ? v : d);
const bool = (v: unknown, d = false): boolean => (typeof v === 'boolean' ? v : d);
const arr = (v: unknown): V[] => (Array.isArray(v) ? (v as V[]) : []);
const idsOf = (v: unknown): string[] => (Array.isArray(v) ? v.filter((x) => typeof x === 'string') : []);
/** The fixture's workspace root, named in errors the way the engine names its `--root`. */
const ROOT = 'workspace';
/** What the engine accepts as an image: everything else is NOT_FOUND, not a silent raster. */
const ASSET_LIKE = /\.(png|jpe?g|webp|bmp|gif|tiff?|svg)$/i;
/** `kind` as the engine reports it — `vector` for SVG, `raster` for anything pixel. */
const wireKind = (name: string): string => (/\.svg$/i.test(name) ? 'vector' : 'raster');

function fail(code: string, msg: string, hint?: string, details?: unknown): never {
  throw new ApiError(code, msg, hint, details);
}

function merge(target: V, patch: V): V {
  for (const [k, v] of Object.entries(patch)) {
    if (v === null) delete target[k];
    else if (
      v &&
      typeof v === 'object' &&
      !Array.isArray(v) &&
      target[k] &&
      typeof target[k] === 'object' &&
      !Array.isArray(target[k])
    )
      merge(target[k] as V, v as V);
    else target[k] = clone(v);
  }
  return target;
}

const svgData = (s: string): string => `data:image/svg+xml;charset=utf-8,${encodeURIComponent(s)}`;

function gcd(a: number, b: number): number {
  let x = Math.round(Math.abs(a));
  let y = Math.round(Math.abs(b));
  while (y) {
    const t = y;
    y = x % y;
    x = t;
  }
  return x || 1;
}

/* -------------------------------------------------------------- node maker */

function mkNode(name: string, type: NodeType, box: Rect, extra: Partial<Node> = {}): Node {
  return {
    id: extra.id ?? `n_${name}`,
    type,
    name,
    artboard: extra.artboard,
    parent: extra.parent,
    children: extra.children ?? [],
    box,
    rotation: extra.rotation ?? 0,
    origin: extra.origin ?? 'center',
    flip_x: false,
    flip_y: false,
    visible: extra.visible ?? true,
    locked: extra.locked ?? false,
    opacity: extra.opacity ?? 1,
    blend: extra.blend,
    role: extra.role,
    z_role: extra.z_role,
    layout: extra.layout ?? { mode: 'manual' },
    style: extra.style,
    text: extra.text,
    image: extra.image,
    vector: extra.vector,
    clip: extra.clip,
    fx: extra.fx,
    data: {},
  };
}

const solid = (color: string): Paint => ({ t: 'solid', color });
const linear = (...stops: [number, string][]): Paint => ({
  t: 'linear',
  angle: 135,
  stops: stops.map(([at, color]) => ({ at, color })),
});
const lay = (i: Intent): Node['layout'] => ({
  mode: 'constraint',
  intent: i,
  provenance: ['size', 'anchor', 'below'],
});

/* --------------------------------------------------------- synthetic photos */

function photoSvg(kind: 'bottle' | 'cup' | 'shoe', w: number, h: number): string {
  const bg =
    kind === 'bottle'
      ? ['#f4efe6', '#e3d9c9', '#cbbfa9']
      : kind === 'cup'
        ? ['#eef1f7', '#d9e2f2', '#b9c7e6']
        : ['#101427', '#1d2544', '#39456f'];
  const dark = kind === 'shoe';
  const subject =
    kind === 'bottle'
      ? `<g>
  <ellipse cx="${w * 0.5}" cy="${h * 0.885}" rx="${w * 0.26}" ry="${h * 0.026}" fill="rgba(60,45,25,.24)"/>
  <path d="M${w * 0.42} ${h * 0.16} h${w * 0.16} v${h * 0.07} l${w * 0.075} ${h * 0.09} v${h * 0.5} a${w * 0.06} ${w * 0.06} 0 0 1 ${-w * 0.06} ${w * 0.06} h${-w * 0.23} a${w * 0.06} ${w * 0.06} 0 0 1 ${-w * 0.06} ${-w * 0.06} v${-h * 0.5} l${w * 0.075} ${-h * 0.09} z" fill="url(#glass)"/>
  <rect x="${w * 0.415}" y="${h * 0.11}" width="${w * 0.17}" height="${h * 0.055}" rx="${w * 0.02}" fill="#2b2b2e"/>
  <rect x="${w * 0.36}" y="${h * 0.4}" width="${w * 0.28}" height="${h * 0.2}" rx="${w * 0.012}" fill="rgba(255,255,255,.92)"/>
  <rect x="${w * 0.395}" y="${h * 0.45}" width="${w * 0.21}" height="${h * 0.012}" rx="${h * 0.006}" fill="#1d2140"/>
  <rect x="${w * 0.395}" y="${h * 0.478}" width="${w * 0.15}" height="${h * 0.008}" rx="${h * 0.004}" fill="#8b8f9e"/>
  <path d="M${w * 0.45} ${h * 0.21} v${h * 0.42}" stroke="rgba(255,255,255,.7)" stroke-width="${w * 0.016}" stroke-linecap="round"/>
</g>`
      : kind === 'cup'
        ? `<g>
  <ellipse cx="${w * 0.5}" cy="${h * 0.86}" rx="${w * 0.29}" ry="${h * 0.03}" fill="rgba(30,40,70,.2)"/>
  <path d="M${w * 0.28} ${h * 0.32} h${w * 0.44} l${-w * 0.05} ${h * 0.46} a${w * 0.05} ${w * 0.05} 0 0 1 ${-w * 0.05} ${h * 0.04} h${-w * 0.23} a${w * 0.05} ${w * 0.05} 0 0 1 ${-w * 0.05} ${-h * 0.04} z" fill="url(#glass)"/>
  <ellipse cx="${w * 0.5}" cy="${h * 0.325}" rx="${w * 0.22}" ry="${h * 0.045}" fill="#fbf7f2"/>
  <ellipse cx="${w * 0.5}" cy="${h * 0.332}" rx="${w * 0.17}" ry="${h * 0.033}" fill="#c79c6d"/>
  <path d="M${w * 0.72} ${h * 0.42} q${w * 0.11} ${h * 0.03} ${w * 0.02} ${h * 0.16}" stroke="rgba(255,255,255,.6)" stroke-width="${w * 0.02}" fill="none"/>
</g>`
        : `<g>
  <ellipse cx="${w * 0.5}" cy="${h * 0.8}" rx="${w * 0.34}" ry="${h * 0.045}" fill="rgba(0,0,0,.45)"/>
  <path d="M${w * 0.16} ${h * 0.62} q${w * 0.06} ${-h * 0.18} ${w * 0.22} ${-h * 0.2} q${w * 0.12} ${-h * 0.02} ${w * 0.2} ${h * 0.06} l${w * 0.1} ${h * 0.08} q${w * 0.06} ${h * 0.05} ${-w * 0.02} ${h * 0.08} l${-w * 0.6} ${h * 0.02} q${-w * 0.05} 0 ${-w * 0.04} ${-h * 0.04} z" fill="#eef1fb"/>
  <path d="M${w * 0.16} ${h * 0.62} h${w * 0.72} q${w * 0.03} ${h * 0.08} ${-w * 0.05} ${h * 0.09} l${-w * 0.6} 0 q${-w * 0.08} ${-h * 0.01} ${-w * 0.07} ${-h * 0.09} z" fill="#4b6bfb"/>
  <path d="M${w * 0.34} ${h * 0.46} q${w * 0.1} ${h * 0.06} ${w * 0.2} ${h * 0.02}" stroke="#8b5cf6" stroke-width="${w * 0.018}" fill="none" stroke-linecap="round"/>
  <circle cx="${w * 0.72}" cy="${h * 0.56}" r="${w * 0.03}" fill="#22d3ee"/>
</g>`;
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">
<defs>
  <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${bg[0]}"/><stop offset=".55" stop-color="${bg[1]}"/><stop offset="1" stop-color="${bg[2]}"/></linearGradient>
  <linearGradient id="glass" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="${dark ? '#c9d3ee' : '#ffffff'}" stop-opacity=".95"/><stop offset=".38" stop-color="${dark ? '#8f9dc4' : '#efe8dc'}"/><stop offset="1" stop-color="${dark ? '#3a4568' : '#c9bda9'}"/></linearGradient>
  <radialGradient id="key" cx=".32" cy=".22" r=".8"><stop offset="0" stop-color="#ffffff" stop-opacity="${dark ? 0.22 : 0.8}"/><stop offset="1" stop-color="#ffffff" stop-opacity="0"/></radialGradient>
  <radialGradient id="vig" cx=".5" cy=".5" r=".75"><stop offset=".6" stop-color="#000000" stop-opacity="0"/><stop offset="1" stop-color="#000000" stop-opacity="${dark ? 0.5 : 0.18}"/></radialGradient>
</defs>
<rect width="${w}" height="${h}" fill="url(#bg)"/>
<circle cx="${w * 0.78}" cy="${h * 0.24}" r="${w * 0.2}" fill="#ffffff" opacity="${dark ? 0.08 : 0.34}"/>
<circle cx="${w * 0.2}" cy="${h * 0.7}" r="${w * 0.13}" fill="${dark ? '#4b6bfb' : '#ffffff'}" opacity="${dark ? 0.16 : 0.3}"/>
${subject}
<rect width="${w}" height="${h}" fill="url(#key)"/>
<rect width="${w}" height="${h}" fill="url(#vig)"/>
</svg>`;
}

/* ------------------------------------------------------- the document store */

interface Proj {
  dir: string;
  doc: Document;
  past: string[];
  future: string[];
  snaps: Record<string, string>;
}

const store = new Map<string, Proj>();
const log: V[] = [];
const jobs: V[] = [{ id: 'j_7c2f', kind: 'export', label: '导出 3 个文件 · PNG @2x', done: 0.62, stage: 'raster', ok: null }];

function touch(p: Proj, origin: string): void {
  p.doc.revision += 1;
  p.doc.meta = { ...(p.doc.meta ?? {}), updated_at: new Date().toISOString(), origin, dirty: true };
}

function pushHistory(p: Proj): void {
  p.past.push(JSON.stringify(p.doc));
  if (p.past.length > 60) p.past.shift();
  p.future = [];
}

function mkAsset(pid: string, name: string, kind: 'bottle' | 'cup' | 'shoe', w: number, h: number): Asset {
  const vec = /\.svg$/i.test(name);
  return {
    id: `a_${name.replace(/[^a-z0-9]/gi, '').slice(0, 12)}`,
    name,
    uri: `assets/${name}`,
    mime: vec ? 'image/svg+xml' : 'image/png',
    width: w,
    height: h,
    bytes: Math.round(w * h * 2.6),
    sha256: `${pid.slice(2)}${name.slice(0, 4)}`.padEnd(64, '0'),
    alpha: vec,
    kind: wireKind(name),
    source: 'local',
    tags: ['product'],
    thumb: svgData(photoSvg(kind, 420, 420)),
    palette: kind === 'shoe' ? ['#101427', '#1d2544', '#4b6bfb', '#eef1fb'] : ['#f4efe6', '#e3d9c9', '#c79c6d', '#2b2b2e'],
    orientation: h > w ? 'portrait' : w > h ? 'landscape' : 'square',
  };
}

function mkBoard(pid: string, name: string, w: number, h: number, bg: Paint, safe: number): Artboard {
  return {
    id: `b_${pid.slice(2)}${name.length}`,
    name,
    width: w,
    height: h,
    background: bg,
    visible: true,
    layers: [],
    safe_margin: safe,
    guides: [{ axis: 'v', pos: Math.round(w / 2), label: '中轴' }],
    ratio: `${Math.round((w / gcd(w, h)) * 100) / 100}:${Math.round((h / gcd(w, h)) * 100) / 100}`,
    vars: {},
  };
}

/** Adds a node to the doc + the board's layer list, in one move. */
function put(pr: Proj, ab: Artboard, n: Node): Node {
  n.artboard = ab.id;
  pr.doc.nodes.push(n);
  ab.layers.push(n.id);
  return n;
}

/* ------------------------------------------------------------------- seeds */

function seedSummer(): void {
  const pid = 'p_summer01';
  const milk = mkAsset(pid, 'milk-bottle.png', 'bottle', 1600, 2000);
  const flat = mkAsset(pid, 'milk-flatlay.png', 'cup', 1400, 1400);
  const kv = mkBoard(pid, '主海报 3:4', 1080, 1440, linear([0, '#fbf8f3'], [0.52, '#f3ece0'], [1, '#e6dccb']), 72);
  const sq = mkBoard(pid, '方图 1:1', 1080, 1080, linear([0, '#ffffff'], [1, '#e2eaf7']), 64);
  const doc: Document = {
    schema_version: '1.1.0',
    project_id: pid,
    revision: 12,
    name: '夏日轻乳 · 新品上市',
    tokens: {
      colors: [
        { name: 'ink', value: '#171d3f' },
        { name: 'accent', value: '#4b6bfb' },
        { name: 'violet', value: '#8b5cf6' },
      ],
      fonts: [{ name: 'display', family: 'PingFang SC', weight: 800, size: 126 }],
      spacing: [{ name: 'gap', value: 90 }],
      radii: [{ name: 'card', value: 28 }],
    },
    assets: [milk, flat],
    artboards: [kv, sq],
    nodes: [],
    meta: {
      created_at: '2026-06-02T09:12:00Z',
      updated_at: new Date(Date.now() - 1000 * 60 * 42).toISOString(),
      author: 'EDY',
      origin: 'desktop',
      description: '6 月新品企划主视觉',
      dirty: false,
    },
  };
  const pr: Proj = { dir: `projects/${pid}`, doc, past: [], future: [], snaps: {} };

  put(pr, kv, mkNode('背景', 'rect', { x: -20, y: -20, w: 1120, h: 1480 }, {
    id: 'n_bg',
    locked: true,
    z_role: 'background',
    style: { fill: linear([0, '#fbf8f3'], [0.52, '#f3ece0'], [1, '#e6dccb']) },
    layout: lay({ size: { mode: 'fill' } }),
  }));
  put(pr, kv, mkNode('柔光', 'ellipse', { x: 150, y: 60, w: 780, h: 620 }, {
    id: 'n_glow',
    opacity: 0.72,
    z_role: 'decor',
    style: { fill: { t: 'radial', cx: 0.5, cy: 0.5, r: 0.62, stops: [{ at: 0, color: '#ffffffcc' }, { at: 1, color: '#ffffff00' }] } },
    layout: lay({ anchor: 'top', center_in: kv.id, size: { w_ref: 0.72, ratio: 1.26 } }),
  }));
  put(pr, kv, mkNode('标题', 'text', { x: 90, y: 148, w: 900, h: 152 }, {
    id: 'n_headline',
    role: 'headline',
    z_role: 'content',
    text: {
      content: '夏日轻乳',
      family: 'PingFang SC',
      size: 126,
      weight: 800,
      color: '#171d3f',
      align: 'left',
      valign: 'top',
      line_ratio: 1.12,
      letter_spacing: -4,
      box_mode: 'auto-height',
      overflow: 'shrink',
    },
    layout: lay({ anchor: 'top-left', margin_top: 148, margin_left: 90, size: { w_ref: 0.84 } }),
  }));
  put(pr, kv, mkNode('副标题', 'text', { x: 92, y: 336, w: 900, h: 44 }, {
    id: 'n_sub',
    role: 'subhead',
    z_role: 'content',
    text: {
      content: 'LESS, BUT BETTER · 生牛乳发酵',
      family: 'Inter',
      size: 27,
      weight: 500,
      color: '#6f6558',
      align: 'left',
      valign: 'top',
      letter_spacing: 3.4,
    },
    layout: lay({ below: { of: 'n_headline', gap: 36, cross: 'start' }, size: { w_ref: 0.66 } }),
  }));
  put(pr, kv, mkNode('产品主图', 'image', { x: 254, y: 404, w: 572, h: 640 }, {
    id: 'n_photo',
    role: 'hero',
    z_role: 'content',
    image: { asset: milk.id, fit: 'cover' },
    style: { fill: { t: 'image', asset: milk.id, fit: 'cover', opacity: 1 }, radius: { all: 28 } },
    fx: { shadows: [{ x: 0, y: 34, blur: 60, color: '#3a2f1e33' }] },
    layout: lay({ center_in: kv.id, size: { w_ref: 0.53, ratio: 0.89 }, safe_margin: 96 }),
  }));
  put(pr, kv, mkNode('NEW 角标', 'rect', { x: 828, y: 96, w: 164, h: 58 }, {
    id: 'n_tagbg',
    z_role: 'label',
    style: { fill: solid('#171d3f'), radius: { all: 29 } },
    layout: lay({ anchor: 'top-right', margin_top: 96, margin_right: 88 }),
  }));
  put(pr, kv, mkNode('角标文字', 'text', { x: 828, y: 111, w: 164, h: 30 }, {
    id: 'n_tagtx',
    z_role: 'label',
    text: { content: 'NEW', size: 22, weight: 700, color: '#ffffff', align: 'center', letter_spacing: 4 },
    layout: lay({ align_to: { of: 'n_tagbg', edge: 'center', offset: 0 } }),
  }));
  put(pr, kv, mkNode('促销胶囊', 'rect', { x: 90, y: 1096, w: 268, h: 76 }, {
    id: 'n_badge',
    z_role: 'label',
    style: { fill: linear([0, '#4b6bfb'], [1, '#8b5cf6']), radius: { all: 38 } },
    fx: { shadows: [{ x: 0, y: 16, blur: 30, color: '#4b6bfb59' }] },
    layout: lay({ anchor: 'bottom-left', margin_left: 90, margin_bottom: 268 }),
  }));
  put(pr, kv, mkNode('促销文字', 'text', { x: 90, y: 1116, w: 268, h: 36 }, {
    id: 'n_badgetx',
    z_role: 'label',
    text: { content: '限时尝鲜', size: 27, weight: 700, color: '#ffffff', align: 'center' },
    layout: lay({ center_in: 'n_badge' }),
  }));
  put(pr, kv, mkNode('价格', 'text', { x: 690, y: 1082, w: 300, h: 92 }, {
    id: 'n_price',
    role: 'cta',
    z_role: 'content',
    text: { content: '¥39', size: 74, weight: 800, color: '#171d3f', align: 'right', letter_spacing: -2 },
    layout: lay({ anchor: 'bottom-right', margin_right: 90, margin_bottom: 266 }),
  }));
  put(pr, kv, mkNode('分隔线', 'line', { x: 90, y: 1226, w: 900, h: 0 }, {
    id: 'n_rule',
    z_role: 'decor',
    style: { stroke: { color: '#171d3f26', width: 1.5 } },
    vector: { line_end: [990, 1226] },
    layout: lay({ size: { w_ref: 0.84 }, anchor: 'bottom' }),
  }));
  put(pr, kv, mkNode('正文', 'text', { x: 90, y: 1258, w: 900, h: 66 }, {
    id: 'n_body',
    z_role: 'content',
    text: {
      content: '轻盈不腻，一口回到夏天的傍晚。冷链直送，48 小时内上桌。',
      size: 25,
      weight: 400,
      color: '#4b443c',
      line_ratio: 1.62,
      align: 'left',
    },
    layout: lay({ below: { of: 'n_rule', gap: 32, cross: 'start' }, size: { w_ref: 0.84, max_h: 140 } }),
  }));
  put(pr, kv, mkNode('页脚', 'text', { x: 90, y: 1374, w: 620, h: 32 }, {
    id: 'n_foot',
    z_role: 'label',
    text: { content: 'AGENTCANVAS · 6 月新品企划', size: 19, weight: 600, color: '#a29889', letter_spacing: 2.6 },
    layout: lay({ anchor: 'bottom-left', margin_left: 90, margin_bottom: 34 }),
  }));

  put(pr, sq, mkNode('方图背景', 'rect', { x: 0, y: 0, w: 1080, h: 1080 }, {
    id: 'n_bg2',
    style: { fill: linear([0, '#ffffff'], [1, '#dde7f7']) },
    layout: lay({ size: { mode: 'fill' } }),
  }));
  put(pr, sq, mkNode('俯拍图', 'image', { x: 150, y: 176, w: 780, h: 520 }, {
    id: 'n_photo2',
    image: { asset: flat.id, fit: 'cover' },
    style: { radius: { all: 36 }, fill: { t: 'image', asset: flat.id, fit: 'cover', opacity: 1 } },
    fx: { shadows: [{ x: 0, y: 26, blur: 48, color: '#1d254430' }] },
    layout: lay({ center_in: sq.id, size: { w_ref: 0.72 } }),
  }));
  put(pr, sq, mkNode('方图标题', 'text', { x: 150, y: 748, w: 780, h: 88 }, {
    id: 'n_title2',
    text: { content: '轻乳 · 冷藏系列', size: 62, weight: 800, color: '#171d3f', align: 'center', letter_spacing: -1 },
    layout: lay({ below: { of: 'n_photo2', gap: 42, cross: 'center' } }),
  }));
  put(pr, sq, mkNode('方图说明', 'text', { x: 150, y: 858, w: 780, h: 40 }, {
    id: 'n_note2',
    text: { content: '生牛乳 85% · 每瓶 280ml', size: 24, weight: 500, color: '#6b7796', align: 'center', letter_spacing: 2 },
    layout: lay({ below: { of: 'n_title2', gap: 22, cross: 'center' } }),
  }));
  store.set(pid, pr);
}

function seedNight(): void {
  const pid = 'p_night22';
  const shoe = mkAsset(pid, 'sneaker-night.png', 'shoe', 1600, 1200);
  const ab = mkBoard(pid, '横幅 16:9', 1920, 1080, linear([0, '#0d1226'], [1, '#232c4e']), 80);
  const doc: Document = {
    schema_version: '1.1.0',
    project_id: pid,
    revision: 5,
    name: '城市夜跑 · 上新横幅',
    tokens: { colors: [{ name: 'ink', value: '#0d1226' }], fonts: [], spacing: [], radii: [] },
    assets: [shoe],
    artboards: [ab],
    nodes: [],
    meta: {
      created_at: '2026-06-18T02:20:00Z',
      updated_at: new Date(Date.now() - 1000 * 60 * 60 * 9).toISOString(),
      author: 'EDY',
      origin: 'mcp',
      description: '由 agent 通过 MCP 生成',
      dirty: false,
    },
  };
  const pr: Proj = { dir: `projects/${pid}`, doc, past: [], future: [], snaps: {} };
  put(pr, ab, mkNode('背景', 'rect', { x: 0, y: 0, w: 1920, h: 1080 }, {
    id: 'n_bg',
    style: { fill: linear([0, '#0d1226'], [0.55, '#1b2445'], [1, '#3a2f6e']) },
    layout: lay({ size: { mode: 'fill' } }),
  }));
  put(pr, ab, mkNode('光晕', 'ellipse', { x: 900, y: -140, w: 1100, h: 760 }, {
    id: 'n_beam',
    opacity: 0.5,
    z_role: 'decor',
    style: { fill: { t: 'radial', cx: 0.5, cy: 0.5, r: 0.6, stops: [{ at: 0, color: '#4b6bfb66' }, { at: 1, color: '#4b6bfb00' }] } },
    layout: lay({ anchor: 'top-right' }),
  }));
  put(pr, ab, mkNode('跑鞋', 'image', { x: 1000, y: 200, w: 760, h: 640 }, {
    id: 'n_photo',
    image: { asset: shoe.id, fit: 'cover' },
    style: { radius: { all: 32 }, fill: { t: 'image', asset: shoe.id, fit: 'cover', opacity: 1 } },
    fx: { shadows: [{ x: 0, y: 30, blur: 70, color: '#00000066' }] },
    layout: lay({ anchor: 'right', margin_right: 160, size: { w_ref: 0.4 } }),
  }));
  put(pr, ab, mkNode('引导词', 'text', { x: 150, y: 210, w: 700, h: 40 }, {
    id: 'n_kicker',
    text: { content: 'CITY NIGHT RUN', size: 24, weight: 700, color: '#22d3ee', letter_spacing: 7 },
    layout: lay({ anchor: 'top-left', margin_top: 210, margin_left: 150 }),
  }));
  put(pr, ab, mkNode('标题', 'text', { x: 150, y: 272, w: 780, h: 250 }, {
    id: 'n_headline',
    role: 'headline',
    text: { content: '把城市跑成背景音', size: 104, weight: 800, color: '#f4f6ff', line_ratio: 1.14, letter_spacing: -3 },
    layout: lay({ below: { of: 'n_kicker', gap: 22, cross: 'start' } }),
  }));
  put(pr, ab, mkNode('正文', 'text', { x: 150, y: 700, w: 620, h: 60 }, {
    id: 'n_body',
    text: { content: '反光中底 · 夜跑可见度提升 3.2 倍', size: 27, weight: 400, color: '#b9c2e6', line_ratio: 1.6 },
    layout: lay({ below: { of: 'n_headline', gap: 34, cross: 'start' } }),
  }));
  put(pr, ab, mkNode('按钮', 'rect', { x: 150, y: 826, w: 340, h: 84 }, {
    id: 'n_cta',
    role: 'cta',
    style: { fill: solid('#f4f6ff'), radius: { all: 42 } },
    layout: lay({ below: { of: 'n_body', gap: 66, cross: 'start' } }),
  }));
  put(pr, ab, mkNode('按钮文字', 'text', { x: 150, y: 850, w: 340, h: 38 }, {
    id: 'n_ctatx',
    text: { content: '立即预约', size: 27, weight: 700, color: '#171d3f', align: 'center' },
    layout: lay({ center_in: 'n_cta' }),
  }));
  store.set(pid, pr);
}

function seedCoffee(): void {
  const pid = 'p_coffee08';
  const cup = mkAsset(pid, 'pour-over.png', 'cup', 1200, 1500);
  const ab = mkBoard(pid, '小红书 3:4', 1242, 1660, linear([0, '#f8f4ee'], [1, '#e6dbc9']), 84);
  const doc: Document = {
    schema_version: '1.1.0',
    project_id: pid,
    revision: 3,
    name: '咖啡节 2026 · 小红书',
    tokens: { colors: [{ name: 'ink', value: '#241c14' }], fonts: [], spacing: [], radii: [] },
    assets: [cup],
    artboards: [ab],
    nodes: [],
    meta: {
      created_at: '2026-07-01T11:00:00Z',
      updated_at: new Date(Date.now() - 1000 * 60 * 60 * 30).toISOString(),
      author: 'EDY',
      origin: 'http',
      dirty: false,
    },
  };
  const pr: Proj = { dir: `projects/${pid}`, doc, past: [], future: [], snaps: {} };
  put(pr, ab, mkNode('背景', 'rect', { x: 0, y: 0, w: 1242, h: 1660 }, {
    id: 'n_bg',
    locked: true,
    style: { fill: linear([0, '#f8f4ee'], [1, '#e6dbc9']) },
    layout: lay({ size: { mode: 'fill' } }),
  }));
  put(pr, ab, mkNode('手冲', 'image', { x: 216, y: 372, w: 810, h: 700 }, {
    id: 'n_photo',
    image: { asset: cup.id, fit: 'contain' },
    style: { radius: { all: 405 } },
    fx: { shadows: [{ x: 0, y: 24, blur: 44, color: '#4b3a2433' }] },
    layout: lay({ center_in: ab.id, size: { w_ref: 0.65, ratio: 1 } }),
  }));
  put(pr, ab, mkNode('标题', 'text', { x: 124, y: 150, w: 994, h: 120 }, {
    id: 'n_headline',
    text: { content: '咖啡节 2026', size: 104, weight: 800, color: '#241c14', align: 'center', letter_spacing: -2 },
    layout: lay({ anchor: 'top', margin_top: 150 }),
  }));
  put(pr, ab, mkNode('副标题', 'text', { x: 124, y: 1132, w: 994, h: 56 }, {
    id: 'n_sub',
    text: { content: '10.24 — 10.26 · 西湖畔', size: 34, weight: 600, color: '#6b5c48', align: 'center', letter_spacing: 3 },
    layout: lay({ below: { of: 'n_photo', gap: 60, cross: 'center' } }),
  }));
  put(pr, ab, mkNode('正文', 'text', { x: 216, y: 1226, w: 810, h: 76 }, {
    id: 'n_body',
    text: {
      content: '32 家独立烘焙 · 4 场杯测 · 1 条完整的风味动线',
      size: 26,
      weight: 400,
      color: '#5a4c3c',
      align: 'center',
      line_ratio: 1.6,
    },
    layout: lay({ below: { of: 'n_sub', gap: 26, cross: 'center' } }),
  }));
  store.set(pid, pr);
}

seedSummer();
seedNight();
seedCoffee();

for (let i = 0; i < 10; i += 1) {
  const m = ['system.caps', 'project.list', 'poster', 'render.preview', 'canvas.batch', 'layout.resolve', 'inspect.validate', 'asset.import', 'render.export', 'text.fonts'][i];
  log.unshift({
    t: new Date(Date.now() - (10 - i) * 1000 * 47).toISOString(),
    m,
    ms: 4 + ((i * 23) % 96),
    ok: i !== 4,
    code: i === 4 ? 'REVISION_CONFLICT' : undefined,
    via: ['mcp', 'http', 'cli'][i % 3],
    p: i % 2 ? { p: 'p_summer01', b: 'b_summer012' } : {},
  });
}

/* --------------------------------------------------------------- measuring */

const esc = (s: string): string =>
  s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

function charW(ch: string, size: number, ls: number): number {
  const wide = /[ -〿㐀-鿿＀-￯]/.test(ch);
  const narrow = /[iIl1.,:;'|!\s]/.test(ch);
  return size * (wide ? 1.02 : narrow ? 0.36 : 0.62) + ls;
}

const measure = (s: string, size: number, ls: number): number =>
  [...s].reduce((a, ch) => a + charW(ch, size, ls), 0);

function wrapText(content: string, size: number, ls: number, maxW: number): string[] {
  const out: string[] = [];
  for (const para of content.split('\n')) {
    let line = '';
    for (const ch of para) {
      if (line && measure(line + ch, size, ls) > maxW) {
        out.push(line);
        line = ch;
      } else line += ch;
    }
    out.push(line);
  }
  return out.length ? out : [''];
}

/* ------------------------------------------------------------- svg compiler */

function gradDef(p: Paint, key: string, defs: string[]): string {
  if (p.t === 'linear') {
    const a = ((p.angle ?? 90) * Math.PI) / 180;
    defs.push(
      `<linearGradient id="${key}" x1="${(0.5 - Math.sin(a) / 2).toFixed(3)}" y1="${(0.5 + Math.cos(a) / 2).toFixed(3)}" x2="${(
        0.5 +
        Math.sin(a) / 2
      ).toFixed(3)}" y2="${(0.5 - Math.cos(a) / 2).toFixed(3)}">` +
        [...p.stops]
          .sort((x, y) => x.at - y.at)
          .map((s) => `<stop offset="${s.at}" stop-color="${s.color}"/>`)
          .join('') +
        '</linearGradient>',
    );
    return `url(#${key})`;
  }
  if (p.t === 'radial') {
    defs.push(
      `<radialGradient id="${key}" cx="${p.cx ?? 0.5}" cy="${p.cy ?? 0.5}" r="${p.r ?? 0.6}">` +
        p.stops.map((s) => `<stop offset="${s.at}" stop-color="${s.color}"/>`).join('') +
        '</radialGradient>',
    );
    return `url(#${key})`;
  }
  return 'none';
}

function nodeSvg(n: Node, doc: Document, defs: string[], seq: number): string {
  if (!n.visible) return '';
  const b = n.box;
  const st = n.style ?? {};
  const cx = b.x + b.w / 2;
  const cy = b.y + b.h / 2;
  const rot = Math.abs(n.rotation) > 0.001 ? ` transform="rotate(${n.rotation.toFixed(2)} ${cx.toFixed(2)} ${cy.toFixed(2)})"` : '';
  const op = n.opacity < 1 ? ` opacity="${n.opacity.toFixed(3)}"` : '';
  const fillOf = (): string => {
    const p = st.fill;
    if (!p || p.t === 'none') return 'none';
    if (p.t === 'solid') return p.color;
    if (p.t === 'image') return 'none';
    return gradDef(p, `g${seq}`, defs);
  };
  const stroke = st.stroke
    ? ` stroke="${st.stroke.color}" stroke-width="${st.stroke.width ?? 1}"${
        st.stroke.dash?.length ? ` stroke-dasharray="${st.stroke.dash.join(' ')}"` : ''
      }`
    : '';
  const rad = st.radius?.all ?? 0;
  const fx = n.fx?.shadows?.length
    ? (() => {
        const s = n.fx!.shadows![0];
        defs.push(
          `<filter id="f${seq}" x="-45%" y="-45%" width="190%" height="190%"><feDropShadow dx="${s.x}" dy="${s.y}" stdDeviation="${(
            s.blur / 2
          ).toFixed(1)}" flood-color="${s.color.slice(0, 7)}" flood-opacity="${
            s.color.length >= 9 ? parseInt(s.color.slice(7, 9), 16) / 255 : 0.35
          }"/></filter>`,
        );
        return ` filter="url(#f${seq})"`;
      })()
    : '';
  const box = (extra = '') =>
    `<rect${rot}${op}${fx} x="${b.x.toFixed(1)}" y="${b.y.toFixed(1)}" width="${Math.max(0, b.w).toFixed(
      1,
    )}" height="${Math.max(0, b.h).toFixed(1)}" rx="${rad}" fill="${fillOf()}"${stroke}${extra}/>`;

  if (n.type === 'text' && n.text) {
    const t = n.text;
    const size = t.size ?? 16;
    const ls = t.letter_spacing ?? 0;
    const lh = (t.line_ratio ?? 1.35) * size;
    const lines = t.lines?.length ? t.lines : wrapText(t.content, size, ls, Math.max(20, b.w));
    let fs = size;
    let myLines = lines;
    if (t.overflow === 'shrink' && lines.length * lh > b.h + 1) {
      const k = Math.min(1, Math.sqrt((b.h + 1) / (lines.length * lh)));
      fs = Math.max(10, Math.round(size * k));
      myLines = wrapText(t.content, fs, ls * k, Math.max(20, b.w));
    }
    const total = myLines.length * (t.line_ratio ?? 1.35) * fs;
    const startY =
      t.valign === 'middle' ? cy - total / 2 + (t.line_ratio ?? 1.35) * fs * 0.78 : t.valign === 'bottom' ? b.y + b.h - total + (t.line_ratio ?? 1.35) * fs * 0.78 : b.y + (t.line_ratio ?? 1.35) * fs * 0.78;
    const anchor = t.align === 'center' ? 'middle' : t.align === 'right' ? 'end' : 'start';
    const tx = t.align === 'center' ? cx : t.align === 'right' ? b.x + b.w : b.x;
    const sp = t.padding ? ` paint-order="stroke fill"` : '';
    return (
      `<text${rot}${op}${fx}${sp} font-family="${esc(t.family ?? 'system-ui')}" font-size="${fs}" font-weight="${
        t.weight ?? 400
      }" fill="${t.color ?? '#171d3f'}" letter-spacing="${(ls * (fs / size)).toFixed(2)}" text-anchor="${anchor}">` +
      myLines.map((l, i) => `<tspan x="${tx.toFixed(1)}" y="${(startY + i * lh * (fs / size)).toFixed(1)}">${esc(l)}</tspan>`).join('') +
      '</text>'
    );
  }

  if (n.type === 'image' && n.image) {
    const a = doc.assets.find((x) => x.id === n.image!.asset);
    if (!a) return box(' fill="#e5e7f0"');
    defs.push(
      `<clipPath id="c${seq}"><rect x="${b.x.toFixed(1)}" y="${b.y.toFixed(1)}" width="${Math.max(0, b.w).toFixed(
        1,
      )}" height="${Math.max(0, b.h).toFixed(1)}" rx="${rad}"/></clipPath>`,
    );
    const par = n.image.fit === 'contain' ? 'xMidYMid meet' : n.image.fit === 'fill' ? 'none' : 'xMidYMid slice';
    const filters: string[] = [];
    if (n.image.blur) filters.push(`feGaussianBlur stdDeviation="${n.image.blur}"`);
    if (n.image.grayscale) filters.push('feColorMatrix type="saturate" values="0"');
    if (n.image.brightness || n.image.contrast)
      filters.push(
        `feComponentTransfer><feFuncR type="linear" slope="${n.image.contrast ?? 1}" intercept="${(
          n.image.brightness ?? 0
        ).toFixed(2)}"/><feFuncG type="linear" slope="${n.image.contrast ?? 1}" intercept="${(n.image.brightness ?? 0).toFixed(
          2,
        )}"/><feFuncB type="linear" slope="${n.image.contrast ?? 1}" intercept="${(n.image.brightness ?? 0).toFixed(2)}/></feComponentTransfer`,
      );
    if (filters.length) defs.push(`<filter id="x${seq}">${filters.join('')}</filter>`);
    const tint = n.image.tint
      ? `<rect x="${b.x}" y="${b.y}" width="${b.w}" height="${b.h}" rx="${rad}" fill="${n.image.tint}" opacity=".35" clip-path="url(#c${seq})"/>`
      : '';
    return (
      `<image${rot}${op}${fx}${filters.length ? ` filter="url(#x${seq})"` : ''} href="${a.thumb}" x="${b.x.toFixed(1)}" y="${b.y.toFixed(
        1,
      )}" width="${b.w.toFixed(1)}" height="${b.h.toFixed(1)}" preserveAspectRatio="${par}" clip-path="url(#c${seq})"/>` + tint
    );
  }

  if (n.type === 'ellipse') {
    return `<ellipse${rot}${op}${fx} cx="${cx.toFixed(1)}" cy="${cy.toFixed(1)}" rx="${(b.w / 2).toFixed(1)}" ry="${(
      b.h / 2
    ).toFixed(1)}" fill="${fillOf()}"${stroke}/>`;
  }
  if (n.type === 'line') {
    const x2 = n.vector?.line_end?.[0] ?? b.x + b.w;
    const y2 = n.vector?.line_end?.[1] ?? b.y + b.h;
    return `<line${rot}${op}${fx} x1="${b.x.toFixed(1)}" y1="${b.y.toFixed(1)}" x2="${x2.toFixed(1)}" y2="${y2.toFixed(
      1,
    )}" stroke="${st.stroke?.color ?? '#171d3f'}" stroke-width="${st.stroke?.width ?? 2}" stroke-linecap="round"/>`;
  }
  if (n.type === 'polygon' || n.type === 'path') {
    if (n.vector?.points?.length) {
      return `<polygon${rot}${op}${fx} points="${n.vector.points.map((p) => `${p[0]},${p[1]}`).join(' ')}" fill="${fillOf()}"${stroke}/>`;
    }
    const sides = n.vector?.sides ?? 3;
    const pts: string[] = [];
    for (let i = 0; i < sides; i += 1) {
      const a = (Math.PI * 2 * i) / sides - Math.PI / 2;
      pts.push(`${(cx + (b.w / 2) * Math.cos(a)).toFixed(1)},${(cy + (b.h / 2) * Math.sin(a)).toFixed(1)}`);
    }
    return `<polygon${rot}${op}${fx} points="${pts.join(' ')}" fill="${fillOf()}"${stroke}/>`;
  }
  if (n.type === 'group') return '';
  if (n.type === 'frame' && fillOf() === 'none') return '';
  return box();
}

function orderIds(doc: Document, ab: Artboard): string[] {
  const byId = new Map(doc.nodes.map((n) => [n.id, n]));
  const out: string[] = [];
  const walk = (nid: string, d = 0) => {
    if (d > 12) return;
    out.push(nid);
    for (const c of byId.get(nid)?.children ?? []) walk(c, d + 1);
  };
  for (const l of ab.layers) walk(l);
  return out;
}

export function compileBoard(
  doc: Document,
  ab: Artboard,
  opts: { scale?: number; omit?: string[]; preview?: boolean; guides?: boolean } = {},
): string {
  const defs: string[] = [];
  const omit = new Set(opts.omit ?? []);
  const byId = new Map(doc.nodes.map((n) => [n.id, n]));
  const body: string[] = [];
  const bgp = ab.background;
  if (bgp && bgp.t !== 'none') {
    if (bgp.t === 'solid') body.push(`<rect x="0" y="0" width="${ab.width}" height="${ab.height}" fill="${bgp.color}"/>`);
    else body.push(`<rect x="0" y="0" width="${ab.width}" height="${ab.height}" fill="${gradDef(bgp, 'abg', defs)}"/>`);
  }
  let seq = 0;
  for (const nid of orderIds(doc, ab)) {
    if (omit.has(nid)) continue;
    const n = byId.get(nid);
    if (!n) continue;
    seq += 1;
    body.push(nodeSvg(n, doc, defs, seq));
  }
  if (opts.guides) {
    for (const g of ab.guides ?? []) {
      body.push(
        g.axis === 'v'
          ? `<line x1="${g.pos}" y1="0" x2="${g.pos}" y2="${ab.height}" stroke="#4b6bfb" stroke-width="1" stroke-dasharray="8 8"/>`
          : `<line x1="0" y1="${g.pos}" x2="${ab.width}" y2="${g.pos}" stroke="#4b6bfb" stroke-width="1" stroke-dasharray="8 8"/>`,
      );
    }
  }
  const sc = opts.scale ?? 1;
  const fonts = [...new Set(doc.nodes.filter((n) => n.visible && n.type === 'text' && n.text?.family).map((n) => n.text!.family!))];
  const style = fonts.length
    ? `<style>${fonts
        .map(
          (f) =>
            `@font-face{font-family:${f.replace(/\s+/g, '-')};src:url(${
              opts.preview ? `/font?family=${encodeURIComponent(f)}` : `'${f}'`
            });font-display:swap}`,
        )
        .join('')}</style>`
    : '';
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${Math.round(ab.width * sc)}" height="${Math.round(
    ab.height * sc,
  )}" viewBox="0 0 ${ab.width} ${ab.height}" data-board="${ab.id}" data-name="${esc(ab.name)}">${
    defs.length ? `<defs>${defs.join('')}</defs>` : ''
  }${style}${body.join('')}</svg>`;
}

export function compileDocSvg(
  doc: Document,
  board: string | null,
  opts: { preview?: boolean; scale?: number; omit?: string[]; guides?: boolean } = {},
): string {
  const list = doc.artboards.filter((a) => a.visible && (!board || a.id === board));
  if (!list.length) return '<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"></svg>';
  if (list.length === 1) return compileBoard(doc, list[0], opts);
  const gap = 64;
  let x = 0;
  const parts = list.map((a) => {
    const inner = compileBoard(doc, a, opts).replace(/^<svg[^>]*>/, '').replace(/<\/svg>$/, '');
    const g = `<g transform="translate(${x} 0)">${inner}</g>`;
    x += a.width + gap;
    return g;
  });
  const wTotal = x - gap;
  const hMax = Math.max(...list.map((a) => a.height));
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${Math.round(wTotal)}" height="${Math.round(hMax)}" viewBox="0 0 ${wTotal} ${hMax}">${parts.join(
    '',
  )}</svg>`;
}

/* ------------------------------------------------------------ doc accessors */

function summary(pr: Proj): V {
  const d = pr.doc;
  return {
    p: d.project_id,
    rev: d.revision,
    name: d.name,
    boards: d.artboards.map((a) => ({ b: a.id, name: a.name, w: a.width, h: a.height, n: a.layers.length })),
    assets: d.assets.length,
    nodes: d.nodes.length,
    dir: pr.dir,
    updated: d.meta?.updated_at,
    w: d.artboards[0]?.width,
    h: d.artboards[0]?.height,
  };
}

function need(p: V): Proj {
  const key = str(p.p) || str(p.project) || str(p.id);
  if (!key) fail('INVALID_SCHEMA', 'missing "p"', 'project.create 返回 {p}，之后的调用都要带上它');
  const hit = store.get(key) ?? [...store.values()].find((x) => x.dir === key || x.doc.name === key);
  if (!hit) fail('NOT_FOUND', `no project "${key}"`, 'project.list 可列出全部项目');
  return hit;
}

function abOf(pr: Proj, p: V): Artboard {
  const key = str(p.b) || str(p.board);
  const ab = key
    ? pr.doc.artboards.find((a) => a.id === key || a.name === key)
    : pr.doc.artboards.find((a) => a.visible) ?? pr.doc.artboards[0];
  if (!ab) fail('ARTBOARD_NOT_FOUND', 'project has no artboard', 'artboard.create {p,w,h} 先建画板');
  return ab;
}

function nodeOf(pr: Proj, p: V): Node {
  const key = str(p.id) || str(p.nodeId);
  const n = pr.doc.nodes.find((x) => x.id === key || x.name === key);
  if (!n) fail('NODE_NOT_FOUND', `no node "${key}"`, 'inspect.document 列出 id，canvas.add 会返回新 id');
  return n;
}

function commit(pr: Proj, p: V, changed: string[], created: string[] = []): V {
  pushHistory(pr);
  touch(pr, str(p.via, 'desktop'));
  return { ok: true, rev: pr.doc.revision, changed, created, warnings: [] };
}

function checkRev(pr: Proj, p: V): void {
  const expect = p.rev;
  if (typeof expect === 'number' && expect !== pr.doc.revision) {
    fail('REVISION_CONFLICT', `document is at rev ${pr.doc.revision}`, '重读 project.get 后用新的 rev 重试', {
      rev: pr.doc.revision,
    });
  }
}

/** `canvas.add` params -> a real Node. */
function buildNode(pr: Proj, p: V, box: Rect): Node {
  const type = (str(p.t, 'rect') || 'rect') as NodeType;
  const n = mkNode(str(p.name, type), type, box, {
    id: rid('n'),
    rotation: num(p.rot, 0),
    opacity: p.opacity === undefined ? 1 : num(p.opacity, 1),
    visible: bool(p.visible, true),
    locked: bool(p.locked, false),
    blend: typeof p.blend === 'string' ? p.blend : undefined,
    z_role: typeof p.z_role === 'string' ? p.z_role : undefined,
    style: p.style as Node['style'],
    fx: p.fx as Node['fx'],
    clip: p.clip as Node['clip'],
    vector: p.vector as Node['vector'],
    layout: (p.l as Node['layout']) ?? (p.layout as Node['layout']) ?? { mode: 'manual' },
  });
  if (p.intent) n.layout = { mode: 'constraint', intent: p.intent as Intent, provenance: [] };
  if (type === 'text') {
    n.text = {
      content: str(p.text, str(p.content, '文字')),
      family: typeof p.family === 'string' ? p.family : 'PingFang SC',
      size: p.size === undefined ? 48 : num(p.size, 48),
      weight: p.weight === undefined ? 700 : num(p.weight, 700),
      color: typeof p.color === 'string' ? p.color : '#171d3f',
      align: typeof p.align === 'string' ? p.align : 'left',
      valign: typeof p.valign === 'string' ? p.valign : 'top',
      line_ratio: p.lh === undefined ? 1.35 : num(p.lh, 1.35),
      letter_spacing: num(p.ls, 0),
      box_mode: typeof p.box_mode === 'string' ? p.box_mode : 'auto-height',
      overflow: typeof p.overflow === 'string' ? p.overflow : 'shrink',
    };
    n.text.lines = wrapText(n.text.content, n.text.size!, n.text.letter_spacing ?? 0, Math.max(20, box.w));
  }
  if (type === 'image') {
    const a = pr.doc.assets.find((x) => x.id === p.asset || x.name === p.asset);
    if (!a) fail('ASSET_NOT_FOUND', `no asset "${str(p.asset)}"`, '先 asset.import {path}，再用返回的 id');
    n.image = { asset: a.id, fit: (str(p.fit, 'cover') as never) ?? 'cover' };
    n.style = { ...(n.style ?? {}), fill: { t: 'image', asset: a.id, fit: n.image.fit, opacity: 1 } };
  }
  return n;
}

/* -------------------------------------------------------------- validation */

function lum(hex: string): number {
  const h = /^#?([0-9a-f]{6})/i.exec(str(hex, ''))?.[1];
  if (!h) return 1;
  const f = (v: number) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
  return 0.2126 * f(parseInt(h.slice(0, 2), 16) / 255) + 0.7152 * f(parseInt(h.slice(2, 4), 16) / 255) + 0.0722 * f(parseInt(h.slice(4, 6), 16) / 255);
}

function contrast(a: string, b: string): number {
  const la = lum(a);
  const lb = lum(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

function boardBg(ab: Artboard): string {
  const p = ab.background;
  if (!p) return '#ffffff';
  if (p.t === 'solid') return p.color;
  if (p.t === 'linear' || p.t === 'radial') return p.stops[0]?.color ?? '#ffffff';
  return '#ffffff';
}

/** The engine's `inspect::summarize` emits compact issues `{s,c,n,b,m,e,f?}` —
 *  the fixture has to match byte for byte or the UI drifts from live behaviour. */
function iss(
  s: 'error' | 'warn' | 'info',
  c: string,
  n: string,
  b: string,
  m: string,
  e: V,
  f?: V | null,
): Issue {
  const o: Issue = { s, c, n, b, m, e };
  if (f !== undefined && f !== null) o.f = f;
  return o;
}

function validate(pr: Proj, board: string | null): Issue[] {
  const out: Issue[] = [];
  const doc = pr.doc;
  const byId = new Map(doc.nodes.map((n) => [n.id, n]));
  for (const ab of doc.artboards.filter((a) => !board || a.id === board)) {
    for (const lid of orderIds(doc, ab)) {
      const n = byId.get(lid);
      if (!n) continue;
      const b = n.box;
      if ((b.w <= 1 || b.h <= 1) && n.type !== 'line') {
        out.push(
          iss('error', 'zero-size', n.id, ab.id, `「${n.name}」尺寸为 0`, { w: Math.round(b.w), h: Math.round(b.h) },
            { t: 'Transform', id: n.id, x: b.x, y: b.y, w: 120, h: 120 }),
        );
      }
      const dx = Math.max(-b.x, b.x + b.w - ab.width);
      const dy = Math.max(-b.y, b.y + b.h - ab.height);
      if (dx > 2 || dy > 2) {
        out.push(
          iss('warn', 'offboard', n.id, ab.id, `「${n.name}」超出画板 ${Math.round(Math.max(dx, dy))}px`, { dx: Math.round(dx), dy: Math.round(dy) },
            {
              t: 'Transform',
              id: n.id,
              x: Math.max(0, Math.min(b.x, ab.width - b.w)),
              y: Math.max(0, Math.min(b.y, ab.height - b.h)),
              w: b.w,
              h: b.h,
            }),
        );
      }
      if (n.type === 'text' && n.text) {
        const t = n.text;
        const lines = wrapText(t.content, t.size ?? 16, t.letter_spacing ?? 0, Math.max(20, b.w));
        const needH = lines.length * (t.line_ratio ?? 1.35) * (t.size ?? 16);
        if (!t.content.trim()) {
          out.push(iss('warn', 'empty-text', n.id, ab.id, `「${n.name}」没有内容`, {}));
        } else if (needH > b.h + 2 && t.overflow !== 'shrink') {
          out.push(
            iss('warn', 'text-overflow', n.id, ab.id, `「${n.name}」文字超出 ${Math.round(needH - b.h)}px`, { need: Math.round(needH), have: Math.round(b.h) },
              { t: 'NodeSet', id: n.id, patch: { text: { overflow: 'shrink' } } }),
          );
        }
        if (t.size && t.size < 18 && t.content.length > 8) {
          out.push(iss('info', 'tiny-target', n.id, ab.id, `「${n.name}」字号 ${Math.round(t.size)}px 偏小`, { size: Math.round(t.size) }));
        }
        const cr = contrast(t.color ?? '#171d3f', boardBg(ab));
        if (cr < 3) {
          out.push(
            iss('warn', 'contrast', n.id, ab.id, `「${n.name}」与底色对比度 ${cr.toFixed(2)}:1`, { ratio: +cr.toFixed(2), bg: boardBg(ab) },
              { t: 'Style', id: n.id, style: { color: lum(boardBg(ab)) > 0.5 ? '#171d3f' : '#ffffff' } }),
          );
        }
        if (ab.safe_margin && (b.x < ab.safe_margin || b.y < ab.safe_margin || b.x + b.w > ab.width - ab.safe_margin)) {
          out.push(
            iss('info', 'safe-area', n.id, ab.id, `「${n.name}」压到安全边距 ${ab.safe_margin}px`, { margin: ab.safe_margin },
              { t: 'LayoutIntent', id: n.id, intent: { safe_margin: ab.safe_margin }, mode: 'constraint' }),
          );
        }
      }
      if (n.type === 'image' && n.image) {
        const a = doc.assets.find((x) => x.id === n.image!.asset);
        if (!a) {
          out.push(iss('error', 'missing-asset', n.id, ab.id, `「${n.name}」引用的素材不存在`, { asset: n.image.asset }));
        } else if (a.width / Math.max(1, b.w) < 0.62) {
          out.push(
            iss('warn', 'low-res', n.id, ab.id, `「${n.name}」放到 ${Math.round(b.w)}px，原图仅 ${Math.round(a.width)}px`, { src: Math.round(a.width), need: Math.round(b.w) }),
          );
        }
      }
      if (n.layout.mode === 'constraint' && n.layout.intent?.below && !byId.has(n.layout.intent.below.of)) {
        out.push(
          iss('error', 'conflict', n.id, ab.id, `「${n.name}」的布局引用了不存在的图层`, { of: n.layout.intent.below.of }),
        );
      }
    }
  }
  return out;
}

/* ------------------------------------------------------------------ solver */

function solveBoard(pr: Proj, boardId: string | null): { changed: string[]; conflicts: V[] } {
  const doc = pr.doc;
  const byId = new Map(doc.nodes.map((n) => [n.id, n]));
  const changed: string[] = [];
  const conflicts: V[] = [];
  for (const ab of doc.artboards.filter((a) => !boardId || a.id === boardId)) {
    for (const lid of orderIds(doc, ab)) {
      const n = byId.get(lid);
      if (!n || n.locked || n.layout.mode === 'manual' || n.layout.pinned || !n.layout.intent) continue;
      const it = n.layout.intent;
      let { x, y, w, h } = n.box;
      const prov: string[] = [];
      const s = it.size;
      if (s) {
        prov.push('size');
        if (s.mode === 'fill') {
          const m = it.margin ?? 0;
          x = m;
          y = m;
          w = ab.width - m * 2;
          h = ab.height - m * 2;
        } else {
          if (s.w !== undefined) w = s.w;
          if (s.h !== undefined) h = s.h;
          if (s.w_ref !== undefined) w = ab.width * s.w_ref;
          if (s.h_ref !== undefined) h = ab.height * s.h_ref;
          if (s.ratio) h = w / s.ratio;
          if (s.max_w !== undefined) w = Math.min(w, s.max_w);
          if (s.max_h !== undefined) h = Math.min(h, s.max_h);
        }
        changed.push(n.id);
      }
      if (it.anchor) {
        prov.push('anchor');
        if (it.anchor.startsWith('top')) y = it.margin_top ?? it.margin ?? y;
        if (it.anchor.startsWith('bottom')) y = ab.height - h - (it.margin_bottom ?? it.margin ?? 0);
        if (it.anchor === 'center') y = (ab.height - h) / 2;
        if (it.anchor.endsWith('left')) x = it.margin_left ?? it.margin ?? x;
        if (it.anchor.endsWith('right')) x = ab.width - w - (it.margin_right ?? it.margin ?? 0);
        if (it.anchor === 'center' || it.anchor === 'top' || it.anchor === 'bottom') x = (ab.width - w) / 2;
        changed.push(n.id);
      }
      const relKey = it.below ? 'below' : it.above ? 'above' : it.left_of ? 'left_of' : it.right_of ? 'right_of' : '';
      const rel = relKey ? ((it as V)[relKey] as { of: string; gap?: number; cross?: string }) : undefined;
      if (rel && relKey) {
        prov.push(relKey);
        const o = byId.get(rel.of) ?? doc.nodes.find((c) => c.name === rel.of);
        if (!o) conflicts.push({ n: n.id, i: relKey, why: `找不到参考对象 "${rel.of}"` });
        else {
          const gap = rel.gap ?? 0;
          if (relKey === 'below') y = o.box.y + o.box.h + gap;
          if (relKey === 'above') y = o.box.y - h - gap;
          if (relKey === 'right_of') x = o.box.x + o.box.w + gap;
          if (relKey === 'left_of') x = o.box.x - w - gap;
          if (rel.cross === 'center') x = o.box.x + o.box.w / 2 - w / 2;
          if (rel.cross === 'start') x = o.box.x;
          if (rel.cross === 'end') x = o.box.x + o.box.w - w;
          changed.push(n.id);
        }
      }
      if (it.align_to) {
        prov.push('align_to');
        const o = byId.get(it.align_to.of);
        if (o) {
          const e = it.align_to.edge;
          const off = it.align_to.offset ?? 0;
          if (e === 'left') x = o.box.x + off;
          if (e === 'right') x = o.box.x + o.box.w - w + off;
          if (e === 'center') x = o.box.x + o.box.w / 2 - w / 2 + off;
          if (e === 'top') y = o.box.y + off;
          if (e === 'bottom') y = o.box.y + o.box.h - h + off;
          if (e === 'middle') y = o.box.y + o.box.h / 2 - h / 2 + off;
          changed.push(n.id);
        }
      }
      if (it.align_x) {
        prov.push('align_x');
        x = it.align_x === 'left' ? it.margin_left ?? x : it.align_x === 'right' ? ab.width - w - (it.margin_right ?? 0) : (ab.width - w) / 2;
        changed.push(n.id);
      }
      if (it.align_y) {
        prov.push('align_y');
        y = it.align_y === 'top' ? it.margin_top ?? y : it.align_y === 'bottom' ? ab.height - h - (it.margin_bottom ?? 0) : (ab.height - h) / 2;
        changed.push(n.id);
      }
      if (it.center_in) {
        prov.push('center_in');
        const other = byId.get(it.center_in);
        const box2 = other ? other.box : { x: 0, y: 0, w: ab.width, h: ab.height };
        x = box2.x + box2.w / 2 - w / 2;
        y = box2.y + box2.h / 2 - h / 2;
        changed.push(n.id);
      }
      if (it.safe_margin !== undefined) {
        prov.push('safe_margin');
        x = Math.max(it.safe_margin, Math.min(x, ab.width - w - it.safe_margin));
        y = Math.max(it.safe_margin, Math.min(y, ab.height - h - it.safe_margin));
        changed.push(n.id);
      }
      n.box = { x: Math.round(x), y: Math.round(y), w: Math.round(w), h: Math.round(h) };
      n.layout.provenance = prov;
    }
  }
  return { changed: [...new Set(changed)], conflicts };
}

/* ------------------------------------------------------------------ poster */

function buildPoster(spec: V): V {
  const title = str(spec.title, '少即是多');
  const subtitle = str(spec.subtitle, 'LESS BUT BETTER');
  const body = str(spec.body, '为 AI 代理准备的画布与排版引擎');
  const accent = str(spec.accent, '#4b6bfb');
  const map: Record<string, [number, number]> = { square: [1080, 1080], story: [1080, 1920], banner: [1920, 1080], xhs: [1242, 1660], 'ecom-main': [800, 800] };
  const [dw, dh] = map[str(spec.preset)] ?? [1080, 1440];
  const w = num(spec.w, dw);
  const h = num(spec.h, dh);
  const pid = rid('p');
  const ab = mkBoard(pid, '主画面', w, h, linear([0, '#ffffff'], [1, '#eef1fb']), 80);
  const doc: Document = {
    schema_version: '1.1.0',
    project_id: pid,
    revision: 1,
    name: str(spec.name, `${title} · 海报`),
    tokens: { colors: [{ name: 'accent', value: accent }], fonts: [], spacing: [], radii: [] },
    assets: [],
    artboards: [ab],
    nodes: [],
    meta: { created_at: new Date().toISOString(), updated_at: new Date().toISOString(), origin: 'desktop', dirty: false },
  };
  const pr: Proj = { dir: `projects/${pid}`, doc, past: [], future: [], snaps: {} };
  const imgs = idsOf(spec.images);
  for (const path of (imgs.length ? imgs : idsOf([spec.image]).filter(Boolean))) {
    const a = mkAsset(pid, String(path).split(/[\\/]/).pop() ?? 'photo.png', 'bottle', 1400, 1800);
    a.id = rid('a');
    doc.assets.push(a);
  }
  put(pr, ab, mkNode('背景', 'rect', { x: 0, y: 0, w, h }, {
    style: { fill: linear([0, '#ffffff'], [0.6, '#f4f6fd'], [1, '#e8edfa']) },
    layout: lay({ size: { mode: 'fill' } }),
  }));
  put(pr, ab, mkNode('主色光斑', 'ellipse', { x: w * 0.6, y: h * 0.04, w: w * 0.5, h: w * 0.5 }, {
    opacity: 0.55,
    style: { fill: { t: 'radial', cx: 0.5, cy: 0.5, r: 0.6, stops: [{ at: 0, color: `${accent}88` }, { at: 1, color: `${accent}00` }] } },
    layout: lay({ anchor: 'top-right' }),
  }));
  if (doc.assets[0]) {
    put(pr, ab, mkNode('主图', 'image', { x: w * 0.24, y: h * 0.2, w: w * 0.52, h: h * 0.42 }, {
      image: { asset: doc.assets[0].id, fit: 'cover' },
      style: { radius: { all: 24 }, fill: { t: 'image', asset: doc.assets[0].id, fit: 'cover', opacity: 1 } },
      fx: { shadows: [{ x: 0, y: 26, blur: 48, color: '#20243f30' }] },
      layout: lay({ center_in: ab.id, size: { w_ref: 0.52, ratio: 0.9 } }),
    }));
  }
  put(pr, ab, mkNode('标题', 'text', { x: w * 0.08, y: h * 0.66, w: w * 0.84, h: h * 0.1 }, {
    text: { content: title, family: 'PingFang SC', size: Math.round(w * 0.098), weight: 800, color: '#171d3f', letter_spacing: -3, line_ratio: 1.12, align: 'left', valign: 'top' },
    layout: lay({ anchor: 'left', margin_left: Math.round(w * 0.08), size: { w_ref: 0.84 }, safe_margin: 60 }),
  }));
  put(pr, ab, mkNode('副标题', 'text', { x: w * 0.082, y: h * 0.78, w: w * 0.7, h: 40 }, {
    text: { content: subtitle, size: Math.round(w * 0.024), weight: 500, color: '#6b7392', letter_spacing: 4, align: 'left', valign: 'top' },
    layout: lay({ below: { of: 'n_标题', gap: 24, cross: 'start' } }),
  }));
  put(pr, ab, mkNode('正文', 'text', { x: w * 0.082, y: h * 0.83, w: w * 0.62, h: 80 }, {
    text: { content: body, size: Math.round(w * 0.022), weight: 400, color: '#4c5474', line_ratio: 1.65, align: 'left', valign: 'top' },
    layout: lay({ below: { of: 'n_副标题', gap: 22, cross: 'start' } }),
  }));
  solveBoard(pr, null);
  store.set(pid, pr);
  return {
    p: pid,
    rev: doc.revision,
    name: doc.name,
    boards: [{ b: ab.id, name: ab.name, w, h }],
    ids: doc.nodes.map((n) => n.id),
    dir: pr.dir,
    warnings: [],
  };
}

/* ------------------------------------------------------------ MCP tool list */

type Prop = { type: string; description: string; items?: unknown };
const S = (d: string): Prop => ({ type: 'string', description: d });
const N = (d: string): Prop => ({ type: 'number', description: d });
const O = (d: string): Prop => ({ type: 'object', description: d });
const A = (d: string): Prop => ({ type: 'array', description: d, items: { type: 'string' } });

const TOOLS: V[] = (
  [
    ['agentcanvas_caps', '一次调用说明整个产品：方法、算子、意图、限制、错误码。', 'system.caps', {}, []],
    ['project_create', '创建项目，返回 {p}，后续调用都要带它。', 'project.create', { name: S('项目名'), w: N('宽 px'), h: N('高 px'), preset: S('poster|square|story|banner|xhs'), boards: A('附加画板 [{w,h,name}]') }, ['name']],
    ['project_open', '按 id 或路径打开项目。', 'project.open', { p: S('项目 id 或路径') }, ['p']],
    ['project_get', '读取整个文档；depth:0 只要摘要。', 'project.get', { p: S('项目'), depth: N('0 摘要 / 1 全文') }, ['p']],
    ['project_save', '原子落盘，返回 path / rev / sha256。', 'project.save', { p: S('项目') }, ['p']],
    ['project_list', '全部项目，按更新时间倒序。', 'project.list', {}, []],
    ['artboard_create', '新增画板：w/h 或 preset 或 ratio。', 'artboard.create', { p: S('项目'), w: N('px'), h: N('px'), preset: S('尺寸预设'), background: S('#hex 或 none'), safe: N('安全边距') }, ['p']],
    ['artboard_duplicate', '把画板复制成别的比例，并重排布局意图。', 'artboard.duplicate', { p: S('项目'), b: S('画板'), to: A('比例 ["1:1","9:16"]') }, ['p', 'b']],
    ['asset_import', '导入图片（按 sha256 去重），返回 id。', 'asset.import', { p: S('项目'), path: S('文件'), paths: A('多个文件') }, ['p']],
    ['canvas_add', '新增图层：text|image|rect|ellipse|line|polygon|path|group|frame|svg。', 'canvas.add', { p: S('项目'), b: S('画板'), t: S('类型'), x: N('px'), y: N('px'), w: N('px'), h: N('px'), text: S('文字内容'), asset: S('素材 id'), style: O('fill/stroke/radius'), intent: O('布局意图') }, ['p', 't']],
    ['canvas_update', '深合并补丁到图层：style / layout / box / text。', 'canvas.update', { p: S('项目'), id: S('图层'), s: O('样式补丁'), l: O('布局补丁'), box: O('{x,y,w,h}') }, ['p', 'id']],
    ['canvas_batch', '一个事务里执行多条命令，全成功或全不执行。', 'canvas.batch', { p: S('项目'), cmds: A('[{t:"Transform",id,x,y,w,h}]') }, ['p', 'cmds']],
    ['canvas_delete', '删除图层（锁定项需要 force）。', 'canvas.delete', { p: S('项目'), ids: A('图层 id'), force: { type: 'boolean', description: '强制' } }, ['p']],
    ['canvas_align', '对齐 / 分布：axis x|y，how left|center|right|top|middle|bottom。', 'canvas.align', { p: S('项目'), ids: A('图层'), axis: S('x|y'), how: S('对齐方式') }, ['p', 'ids']],
    ['layout_apply', '挂上语义约束并求解。', 'layout.apply', { p: S('项目'), id: S('图层'), intent: O('约束对象'), mode: S('constraint|manual|flex|grid') }, ['p', 'id', 'intent']],
    ['layout_resolve', '求解整板或单画板，返回 changed 与 conflicts。', 'layout.resolve', { p: S('项目'), b: S('可选画板') }, ['p']],
    ['layout_explain', '这个图层为什么在这里：按优先级的意图来源。', 'layout.explain', { p: S('项目'), id: S('图层') }, ['p', 'id']],
    ['inspect_validate', '机器可读的质量报告，每条问题带数值证据与修复命令。', 'inspect.validate', { p: S('项目'), b: S('可选画板'), contrast: { type: 'boolean', description: '检查对比度' } }, ['p']],
    ['inspect_fix', '应用建议修复（dry:1 只看不动）。', 'inspect.fix', { p: S('项目'), dry: { type: 'boolean', description: '试运行' } }, ['p']],
    ['render_preview', '把当前文档光栅化成真实 PNG，返回 path + url。', 'render.preview', { p: S('项目'), b: S('可选画板'), sc: N('缩放 0..4') }, ['p']],
    ['render_svg', '编译后的 SVG —— 光栅化消费的就是这个字符串。', 'render.svg', { p: S('项目'), b: S('可选画板'), preview: { type: 'boolean', description: '用 /asset 与 /font 链接' } }, ['p']],
    ['render_export', '导出 PNG / JPG / WebP / SVG / PDF，返回尺寸、大小与 sha256。', 'render.export', { p: S('项目'), f: S('png|jpg|webp|svg|pdf'), sc: N('倍率'), q: N('质量 1-100'), o: S('输出目录'), transparent: { type: 'boolean', description: '透明底' } }, ['p']],
    ['history_undo', '撤销 n 步。人和 agent 共用一条历史。', 'history.undo', { p: S('项目'), n: N('步数') }, ['p']],
    ['history_redo', '重做 n 步。', 'history.redo', { p: S('项目'), n: N('步数') }, ['p']],
    ['history_list', '命令轨迹，含 origin（desktop|mcp|http|cli）。', 'history.list', { p: S('项目'), n: N('条数') }, ['p']],
    ['template_use', '把内置或保存的模板实例化进项目。', 'template.use', { p: S('项目'), tpl: S('模板名'), vars: O('title/subtitle/asset/accent') }, ['p', 'tpl']],
    ['template_variants', '生成系列：同一版式，不同配色 / 文案 / 画板。', 'template.variants', { p: S('项目'), from: N('源画板序号'), colors: A('主色'), copy: O('{title:[...]}') }, ['p']],
    ['poster', '一次调用完成整张海报：建项目、导素材、排版、求解、校验、预览、导出。', 'poster', { title: S('主标题'), subtitle: S('副标题'), body: S('正文'), image: S('产品图路径'), accent: S('#hex'), preset: S('尺寸预设'), tpl: S('内置版式'), f: S('导出格式'), out: S('输出目录') }, ['title']],
    ['image_op', '对单个文件做一次位图操作。', 'image.op', { in: S('输入路径'), out: S('输出路径'), op: S('算子'), args: O('算子参数'), q: N('质量') }, ['in', 'op']],
    ['image_chain', '按链式顺序执行位图操作。', 'image.chain', { in: S('输入'), out: S('输出'), chain: A('步骤或一条字符串'), q: N('质量'), fmt: S('png|jpg|webp') }, ['in', 'chain']],
    ['image_batch', '同一链条跑多个文件，逐条返回结果。', 'image.batch', { paths: A('输入文件'), chain: A('步骤'), preset: S('预设名'), out: S('输出目录'), fmt: S('格式'), q: N('质量') }, ['paths']],
    ['image_probe', '真实尺寸、格式、alpha、字节、sha256、主色。', 'image.probe', { path: S('文件') }, ['path']],
    ['image_compare', '两张图的相似度：0-100 分、逐通道误差、变化包围盒。', 'image.compare', { a: S('图 A'), b2: S('图 B') }, ['a', 'b2']],
    ['text_measure', '用渲染同款字体引擎测量文本。', 'text.measure', { text: S('字符串'), family: S('字体'), size: N('px'), max_w: N('换行宽度'), weight: N('100-900') }, ['text']],
    ['system_doctor', '环境自检：可写目录、字体数、CJK、端口。', 'system.doctor', {}, []],
    ['system_tools', '本工具清单，加上完整方法表。', 'system.tools', { full: { type: 'boolean', description: '包含 schema' } }, []],
    ['batch', '一次往返调用多个方法：jobs:[{m,p}]。', 'batch', { jobs: A('[{m,p}]') }, ['jobs']],
  ] as [string, string, string, Record<string, Prop>, string[]][]
).map(([name, description, method, properties, required]) => ({
  name,
  description,
  method,
  inputSchema: { type: 'object', properties, required, additionalProperties: true },
}));

/* ---------------------------------------------------------------- image lab */

interface LabImg {
  kind: 'bottle' | 'cup' | 'shoe';
  w: number;
  h: number;
  baseW: number;
  baseH: number;
  filters: string[];
  overlay: string;
  radius: number;
  border: string;
  rotate: number;
  flip: boolean;
}

function labFrom(kind: LabImg['kind'], w: number, h: number): LabImg {
  return { kind, w, h, baseW: w, baseH: h, filters: [], overlay: '', radius: 0, border: '', rotate: 0, flip: false };
}

const PRESETS: Record<string, string> = {
  social_1080: 'crop 1080x1080 > scale 1 > sharpen 0.8',
  ecommerce_main: 'crop 800x800 > background #ffffff > sharpen 0.6',
  wechat_cover: 'crop 900x500 > adjust 1.04,1.06 > sharpen 0.5',
  avatar: 'crop 400x400 > round 200',
  strip4x3: 'strip 3 > gap 24',
};

function parseChain(s: string): V[] {
  return s
    .split('>')
    .map((x) => x.trim())
    .filter(Boolean)
    .map((tok) => {
      const [op, ...rest] = tok.split(/\s+/);
      const args: V = {};
      for (const r of rest) {
        if (/^\d+x\d+$/.test(r)) {
          const [w, h] = r.split('x').map(Number);
          args.w = w;
          args.h = h;
        } else if (/^\d+:\d+$/.test(r)) args.ratio = r;
        else if (/^#[0-9a-f]{3,8}$/i.test(r)) args.color = r;
        else if (/^-?\d+(\.\d+)?$/.test(r)) args.v = Number(r);
        else args.text = (args.text ? `${args.text} ${r}` : r);
      }
      return { op, ...args };
    });
}

function applyOp(img: LabImg, op: string, a: V): void {
  const v = num(a.v, 0);
  switch (op) {
    case 'crop': {
      const ratio = str(a.ratio, '');
      if (ratio.includes(':')) {
        const [x, y] = ratio.split(':').map(Number);
        const target = x / y;
        if (img.w / img.h > target) img.w = Math.round(img.h * target);
        else img.h = Math.round(img.w / target);
      } else {
        img.w = Math.round(num(a.w, img.w));
        img.h = Math.round(num(a.h, img.h));
      }
      break;
    }
    case 'resize':
      img.w = Math.round(num(a.w, img.w));
      img.h = Math.round(num(a.h, img.h));
      break;
    case 'scale': {
      const by = a.by !== undefined ? num(a.by, 1) : v || 1;
      img.w = Math.round(img.w * by);
      img.h = Math.round(img.h * by);
      break;
    }
    case 'pad': {
      const p = Math.round(num(a.v, num(a.pad, 48)));
      img.w += p * 2;
      img.h += p * 2;
      img.overlay += `<rect x="0" y="0" width="${img.w}" height="${img.h}" fill="${str(a.color, '#ffffff')}" opacity=".55"/>`;
      break;
    }
    case 'rotate':
      img.rotate = (img.rotate + num(a.deg, v || 90)) % 360;
      if (Math.abs(img.rotate) % 180 === 90) {
        const t = img.w;
        img.w = img.h;
        img.h = t;
      }
      break;
    case 'flip':
      img.flip = !img.flip;
      break;
    case 'round':
      img.radius = Math.round(num(a.r, num(a.v, 48)));
      break;
    case 'border': {
      const bw = Math.round(num(a.w, num(a.v, 12)));
      img.w += bw * 2;
      img.h += bw * 2;
      img.border = `<rect x="${bw / 2}" y="${bw / 2}" width="${img.w - bw}" height="${img.h - bw}" rx="${img.radius}" fill="none" stroke="${str(
        a.color,
        '#171d3f',
      )}" stroke-width="${bw}"/>`;
      break;
    }
    case 'adjust': {
      const b = a.brightness !== undefined ? num(a.brightness, 1) : v || 1.04;
      const c = a.contrast !== undefined ? num(a.contrast, 1) : num(a.c, 1.06);
      const s2 = a.saturate !== undefined ? num(a.saturate, 1) : num(a.s, 1.1);
      img.filters.push(
        `feComponentTransfer><feFuncR type="linear" slope="${(c * b).toFixed(2)}" intercept="${((1 - c) * 0.5 + (b - 1) * 0.5).toFixed(
          2,
        )}"/><feFuncG type="linear" slope="${(c * b).toFixed(2)}" intercept="${((1 - c) * 0.5 + (b - 1) * 0.5).toFixed(2)}"/><feFuncB type="linear" slope="${(c * b).toFixed(
          2,
        )}"/><feFuncA type="identity"/></feComponentTransfer><feColorMatrix type="saturate" values="${s2.toFixed(2)}`,
      );
      break;
    }
    case 'blur':
      img.filters.push(`feGaussianBlur stdDeviation="${num(a.sigma, v || 6).toFixed(1)}"`);
      break;
    case 'sharpen': {
      const amt = a.amount !== undefined ? num(a.amount, 0.6) : v || 0.6;
      img.filters.push(
        `feConvolveMatrix order="3" kernelMatrix="0 -${amt.toFixed(2)} 0 -${amt.toFixed(2)} ${(1 + 4 * amt).toFixed(2)} -${amt.toFixed(
          2,
        )} 0 -${amt.toFixed(2)} 0" preserveAlpha="true"`,
      );
      break;
    }
    case 'noise':
      img.filters.push(
        `feTurbulence type="fractalNoise" baseFrequency="0.9" numOctaves="2" result="n"/><feComposite in="n" in2="SourceGraphic" operator="arithmetic" k1="0" k2="${(
          v || 0.12
        ).toFixed(2)}" k3="${(v || 0.12).toFixed(2)}" k4="0"`,
      );
      break;
    case 'gray':
      img.filters.push('feColorMatrix type="saturate" values="0"');
      break;
    case 'invert':
      img.filters.push('feComponentTransfer><feFuncR type="table" tableValues="1 0"/><feFuncG type="table" tableValues="1 0"/><feFuncB type="table" tableValues="1 0"/></feComponentTransfer');
      break;
    case 'threshold':
      img.filters.push(
        `feColorMatrix type="saturate" values="0"/><feComponentTransfer><feFuncR type="discrete" tableValues="0 1"/><feFuncG type="discrete" tableValues="0 1"/><feFuncB type="discrete" tableValues="0 1"`,
      );
      break;
    case 'tint':
      img.overlay += `<rect x="0" y="0" width="${img.w}" height="${img.h}" fill="${str(a.color, '#4b6bfb')}" opacity="${num(
        a.opacity,
        0.28,
      )}" style="mix-blend-mode:multiply"/>`;
      break;
    case 'background':
      img.overlay = `<rect x="0" y="0" width="${img.w}" height="${img.h}" fill="${str(a.color, '#ffffff')}"/>${img.overlay}`;
      break;
    case 'watermark': {
      const text = str(a.text, 'AGENTCANVAS');
      const size = Math.round(num(a.size, Math.max(18, img.w * 0.045)));
      const pos = str(a.pos, 'bottom-right');
      const px = pos.includes('left') ? size : img.w - size;
      const py = pos.includes('top') ? size * 1.6 : img.h - size * 0.8;
      const anch = pos.includes('left') ? 'start' : 'end';
      img.overlay += `<text x="${px}" y="${py}" text-anchor="${anch}" font-family="Inter, system-ui" font-size="${size}" font-weight="700" letter-spacing="${(
        size * 0.14
      ).toFixed(1)}" fill="${str(a.color, '#ffffff')}" opacity="${num(a.opacity, 0.72)}">${esc(text)}</text>`;
      break;
    }
    case 'remove_bg':
      img.overlay += `<rect x="0" y="0" width="${img.w}" height="${img.h}" fill="url(#chk)"/>`;
      break;
    case 'strip': {
      const n = Math.round(num(a.n, v || 3));
      const gap = Math.round(num(a.gap, 24));
      img.overlay += `<text x="${img.w / 2}" y="${img.h - 12}" text-anchor="middle" font-family="Inter, system-ui" font-size="13" fill="#6b7392">strip ×${n} · gap ${gap}px</text>`;
      break;
    }
    default:
      break;
  }
}

function labSvg(img: LabImg): string {
  const filt = img.filters.length ? `<filter id="fx">${img.filters.map((f) => `<${f}`).join('')}</filter>` : '';
  const clip = img.radius > 0 ? `<clipPath id="rr"><rect x="0" y="0" width="${img.w}" height="${img.h}" rx="${img.radius}"/></clipPath>` : '';
  const chk = `<pattern id="chk" width="16" height="16" patternUnits="userSpaceOnUse"><rect width="16" height="16" fill="#ffffff"/><path d="M0 8h8V0zM8 16h8V8z" fill="#e9edf7"/></pattern>`;
  const inner = `<image href="${svgData(photoSvg(img.kind, img.baseW, img.baseH))}" x="0" y="0" width="${img.w}" height="${img.h}" preserveAspectRatio="xMidYMid slice"${
    img.filters.length ? ' filter="url(#fx)"' : ''
  }${clip ? ' clip-path="url(#rr)"' : ''}/>`;
  const g = img.flip || img.rotate ? `<g transform="translate(${img.w / 2} ${img.h / 2}) rotate(${img.rotate}) scale(${img.flip ? -1 : 1} 1) translate(${-img.w / 2} ${-img.h / 2})">${inner}</g>` : inner;
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${img.w}" height="${img.h}" viewBox="0 0 ${img.w} ${img.h}"><defs>${filt}${clip}${chk}</defs><rect width="${img.w}" height="${img.h}" fill="#f6f7fb"/>${g}${img.overlay}${img.border}</svg>`;
}

function labOf(path: string): LabImg {
  const name = String(path).split(/[\\/]/).pop() ?? 'image.png';
  const kind: LabImg['kind'] = /shoe|run|night|sneaker/i.test(name) ? 'shoe' : /coffee|cup|latte|pour/i.test(name) ? 'cup' : 'bottle';
  const base: Record<LabImg['kind'], [number, number]> = { bottle: [1200, 1600], cup: [1400, 1400], shoe: [1600, 1100] };
  const [w, h] = base[kind];
  return labFrom(kind, w, h);
}

/* ---------------------------------------------------------------- dispatch */

const OPS: Record<string, (p: V) => unknown> = {
  'system.caps': () => ({
    name: 'AgentCanvas',
    ver: '1.0.0',
    schema: '1.1.0',
    methods: Object.keys(OPS).map((k) => `${k}  fixture 实现`),
    image_ops: [
      'crop', 'resize', 'scale', 'pad', 'rotate', 'flip', 'round', 'border', 'adjust', 'blur', 'sharpen',
      'noise', 'gray', 'invert', 'threshold', 'tint', 'watermark', 'background', 'remove_bg', 'strip',
    ],
    image_presets: Object.keys(PRESETS),
    templates: ['poster_kv', 'ecom_main', 'story_min', 'card_quote'],
    presets: [
      { k: 'poster', wh: [1080, 1440], d: '海报 3:4' },
      { k: 'square', wh: [1080, 1080], d: '方图 1:1' },
      { k: 'story', wh: [1080, 1920], d: '故事 9:16' },
      { k: '4:5', wh: [1080, 1350], d: '竖图 4:5' },
      { k: 'banner', wh: [1920, 1080], d: '横幅 16:9' },
      { k: 'xhs', wh: [1242, 1660], d: '小红书 3:4' },
      { k: 'ecom-main', wh: [800, 800], d: '电商主图' },
      { k: 'long', wh: [750, 3000], d: '图文长图' },
      { k: 'a4', wh: [2480, 3508], d: 'A4 竖版' },
      { k: 'wechat', wh: [900, 500], d: '公众号头图' },
      { k: 'card', wh: [1600, 500], d: '卡片横幅' },
      { k: 'business-card', wh: [1050, 600], d: '名片' },
    ],
    keys: { p: 'project', b: 'board', id: 'node', rev: 'expectedRevision', t: 'node type', s: 'style', l: 'layout', o: 'out path', sc: 'scale', q: 'quality', f: 'format' },
    intents: ['anchor', 'margin', 'below', 'above', 'left_of', 'right_of', 'center_in', 'align_x', 'align_y', 'align_to', 'size', 'fit', 'safe_margin', 'distribute', 'equal_gap', 'keep_aspect_ratio', 'flex', 'grid', 'priority'],
    node_types: ['text', 'image', 'rect', 'ellipse', 'line', 'polygon', 'path', 'group', 'frame', 'svg'],
    limits: { max_px: 60000000, job_timeout_s: 600 },
    codes: ['NOT_FOUND: 对象不存在', 'REVISION_CONFLICT: 修订号过期', 'LOCKED: 图层已锁定', 'INVALID_SCHEMA: 参数不合法', 'ASSET_NOT_FOUND: 素材未导入'],
    paths: { root: '~/AgentCanvas', projects: 'projects/<id>/', exports: 'projects/<id>/exports/' },
    port: 8899,
    fonts: 128,
  }),
  'system.health': () => ({ ok: true, ver: '1.0.0', schema: '1.1.0', port: 8899, pid: 0, jobs: jobs.length, fonts: 128, root: '~/AgentCanvas' }),
  'system.version': () => ({ ver: '1.0.0', schema: '1.1.0', build: '1.0.0 windows-x64' }),
  'system.doctor': () => ({
    ok: true,
    root: '~/AgentCanvas',
    writable: true,
    fonts: 128,
    cjk_font: true,
    port: 8899,
    http: false,
    version: '1.0.0',
    limits: { max_px: 60000000, job_timeout_s: 600 },
    warnings: [],
  }),
  'system.tools': (p) =>
    p?.full ? TOOLS : TOOLS.map((t) => ({ n: t.name, m: t.method, d: t.description })),
  'system.config': (p) => {
    const set = p.set as V | undefined;
    if (set && typeof set === 'object') {
      for (const k of Object.keys(set)) {
        if (!ALLOWED.includes(k)) fail('INVALID_VALUE', `unknown setting "${k}"`, '见 system.config 的默认集');
      }
      const prov = set.provider as V | undefined;
      if (prov && typeof prov === 'object') {
        for (const pk of Object.keys(prov)) {
          if (!ALLOWED_PROVIDER.includes(pk)) fail('INVALID_VALUE', `unknown provider setting "${pk}"`, 'name / model / baseUrl / apiKeyEnv');
        }
        cfgStore.provider = { ...(cfgStore.provider as V), ...prov };
      }
      if (typeof set.token === 'string') cfgStore.token = set.token;
      if (set.token === null) cfgStore.token = '';
      const { provider: _p, token: _t, ...rest } = set;
      cfgStore.file = { ...(cfgStore.file as V), ...rest };
    }
    return cfgView();
  },
  'system.provider.probe': (p) => {
    const base = String(str(p.url, '') || String((cfgStore.provider as V)?.baseUrl ?? ''));
    if (!base) fail('PROVIDER_NOT_CONFIGURED', 'provider.baseUrl 为空', '填一个本地 http 服务地址，如 http://127.0.0.1:11434');
    if (base.startsWith('https://')) fail('NETWORK', '此构建只探测明文 http', '把 provider.baseUrl 指向本地 http 端点');
    const trimmed = base.replace(/\/+$/, '');
    const ollama = String((cfgStore.provider as V)?.name ?? '').toLowerCase().includes('ollama') || base.includes(':11434');
    return { ok: true, url: `${trimmed}${ollama ? '/api/tags' : '/v1/models'}`, status: 200, ms: 42, auth: 'none', n_models: 0, models: [], parsed: false };
  },
  'system.schema': () => ({
    cmds: ['NodeAdd', 'NodeDel', 'NodeSet', 'NodesSet', 'NodeMove', 'BoardAdd', 'BoardDel', 'BoardSet', 'AssetAdd', 'AssetDel', 'TokenSet', 'Text', 'Transform', 'LayoutIntent', 'Style', 'Group', 'Ungroup', 'Duplicate', 'Reflow', 'Solve', 'Composite'],
    intents: ['anchor', 'margin', 'below', 'above', 'center_in', 'align_to', 'size', 'safe_margin', 'priority'],
    codes: ['NOT_FOUND', 'REVISION_CONFLICT', 'LOCKED', 'INVALID_SCHEMA', 'ASSET_NOT_FOUND'],
  }),
  'system.demo': (p) => buildPoster({ title: '少即是多', subtitle: 'LESS, BUT BETTER', accent: '#ff7a59', name: str(p.o, 'demo-poster') }),

  'project.create': (p) => {
    const map: Record<string, [number, number]> = { poster: [1080, 1440], square: [1080, 1080], story: [1080, 1920], banner: [1920, 1080], xhs: [1242, 1660], 'ecom-main': [800, 800], a4: [2480, 3508], wechat: [900, 500], card: [1600, 500], long: [750, 3000], '4:5': [1080, 1350], 'business-card': [1050, 600] };
    const [dw, dh] = map[str(p.preset)] ?? [num(p.w, 1080), num(p.h, 1440)];
    const pid = rid('p');
    const name = str(p.name, '未命名画布');
    const ab = mkBoard(pid, name.slice(0, 10) || '画板 1', dw, dh, solid(str(p.background, '#ffffff')), num(p.safe, 72));
    const doc: Document = {
      schema_version: '1.1.0',
      project_id: pid,
      revision: 1,
      name,
      tokens: { colors: [], fonts: [], spacing: [], radii: [] },
      assets: [],
      artboards: [ab],
      nodes: [],
      meta: { created_at: new Date().toISOString(), updated_at: new Date().toISOString(), origin: 'desktop', dirty: false },
    };
    const pr: Proj = { dir: `projects/${pid}`, doc, past: [], future: [], snaps: {} };
    for (const extra of arr(p.boards)) {
      doc.artboards.push(mkBoard(pid, str(extra.name, '画板'), num(extra.w, dw), num(extra.h, dh), solid(str(extra.background, '#f4f5fa')), num(extra.safe, 72)));
    }
    store.set(pid, pr);
    return summary(pr);
  },
  'project.open': (p) => summary(need(p)),
  'project.list': () => [...store.values()].map(summary),
  'project.recent': () => [...store.values()].map(summary),
  'project.get': (p) => {
    const pr = need(p);
    if (num(p.depth, 1) === 0) return summary(pr);
    /* engine shape: the whole document, but under the compact wire keys (`rev`, `p`) */
    const { revision, project_id, ...rest } = clone(pr.doc);
    return { ...rest, rev: revision, p: project_id };
  },
  'project.save': (p) => {
    const pr = need(p);
    pr.doc.meta = { ...pr.doc.meta, dirty: false, updated_at: new Date().toISOString() };
    return { ok: true, path: `${pr.dir}/project.json`, rev: pr.doc.revision, sha: `${pr.doc.project_id}${pr.doc.revision}`.padEnd(64, '0e4a').slice(0, 64) };
  },
  'project.close': (p) => {
    const pr = need(p);
    store.delete(pr.doc.project_id);
    return { ok: true };
  },
  'project.clone': (p) => {
    const pr = need(p);
    const pid = rid('p');
    const copy: Proj = { dir: `projects/${pid}`, doc: clone(pr.doc), past: [], future: [], snaps: {} };
    copy.doc.project_id = pid;
    copy.doc.name = str(p.name, `${pr.doc.name} 副本`);
    copy.doc.revision = 1;
    store.set(pid, copy);
    return summary(copy);
  },
  'project.snapshot': (p) => {
    const pr = need(p);
    const snap = `s${pr.doc.revision}_${Math.random().toString(36).slice(2, 6)}`;
    pr.snaps[snap] = JSON.stringify(pr.doc);
    return { snap };
  },
  'project.restore': (p) => {
    const pr = need(p);
    const snap = str(p.snap);
    if (!pr.snaps[snap]) fail('NOT_FOUND', `no snapshot "${snap}"`);
    pr.doc = JSON.parse(pr.snaps[snap]);
    touch(pr, 'desktop');
    return summary(pr);
  },
  'project.export_bundle': (p) => {
    const pr = need(p);
    return {
      ok: true,
      path: `${str(p.o, pr.dir)}.acanvas`,
      bytes: 128000 + pr.doc.nodes.length * 4200,
      files: ['project.json', ...pr.doc.assets.map((a) => a.uri)],
    };
  },

  'artboard.list': (p) => {
    const pr = need(p);
    /* engine shape: {b,name,w,h,n} — the layer count is `n`, there is no `layers` */
    return pr.doc.artboards.map((a) => ({ b: a.id, name: a.name, w: a.width, h: a.height, n: a.layers.length }));
  },
  'artboard.create': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const w = num(p.w, 1080);
    const h = num(p.h, 1440);
    const ab = mkBoard(pr.doc.project_id, str(p.name, `画板 ${pr.doc.artboards.length + 1}`), w, h, solid(str(p.background, '#ffffff')), num(p.safe, 72));
    pr.doc.artboards.push(ab);
    return commit(pr, p, [ab.id], [ab.id]);
  },
  'artboard.resize': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const ab = abOf(pr, p);
    const w = num(p.w, ab.width);
    const h = num(p.h, ab.height);
    if (bool(p.reflow, true)) {
      for (const nid of orderIds(pr.doc, ab)) {
        const n = pr.doc.nodes.find((x) => x.id === nid);
        if (!n) continue;
        n.box = { x: (n.box.x / ab.width) * w, y: (n.box.y / ab.height) * h, w: (n.box.w / ab.width) * w, h: (n.box.h / ab.height) * h };
      }
    }
    ab.width = w;
    ab.height = h;
    return commit(pr, p, orderIds(pr.doc, ab));
  },
  'artboard.delete': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    if (pr.doc.artboards.length < 2) fail('INVALID_SCHEMA', '至少保留一个画板', '先 artboard.create 再删');
    const ab = abOf(pr, p);
    const gone = new Set(orderIds(pr.doc, ab));
    pr.doc.artboards = pr.doc.artboards.filter((a) => a.id !== ab.id);
    pr.doc.nodes = pr.doc.nodes.filter((n) => !gone.has(n.id));
    return commit(pr, p, [...gone]);
  },
  'artboard.set': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const ab = abOf(pr, p);
    Object.assign(ab, clone((p.s ?? p.patch ?? {}) as V));
    return commit(pr, p, [ab.id]);
  },
  'artboard.duplicate': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const src = abOf(pr, p);
    const made: string[] = [];
    for (const ratio of idsOf(p.to)) {
      const [a, b] = String(ratio).split(':').map(Number);
      if (!a || !b) fail('INVALID_SCHEMA', `bad ratio "${ratio}"`, '用 1:1 / 4:5 / 9:16');
      const w = Math.round((src.width * b) / a);
      const ab = mkBoard(pr.doc.project_id, `${src.name} ${ratio}`, w, Math.round((w * b) / a), clone(src.background!) , src.safe_margin ?? 0);
      for (const lid of orderIds(pr.doc, src)) {
        const s = pr.doc.nodes.find((n) => n.id === lid);
        if (!s) continue;
        const c = clone(s);
        c.id = rid('n');
        c.name = `${s.name} · ${ratio}`;
        c.artboard = ab.id;
        c.box = { x: (s.box.x / src.width) * w, y: (s.box.y / src.height) * ab.height, w: (s.box.w / src.width) * w, h: (s.box.h / src.height) * ab.height };
        pr.doc.nodes.push(c);
        ab.layers.push(c.id);
      }
      pr.doc.artboards.push(ab);
      made.push(ab.id);
    }
    return commit(pr, p, made, made);
  },

  'asset.list': (p) => {
    const pr = need(p);
    const q = str(p.q).toLowerCase();
    /* engine shape: {id,name,w,h,kind,bytes} — deliberately compact, no uri/palette */
    return pr.doc.assets
      .filter((a) => !q || a.name.toLowerCase().includes(q))
      .map((a) => ({ id: a.id, name: a.name, w: a.width, h: a.height, kind: a.kind, bytes: a.bytes }));
  },
  'asset.import': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const list = (Array.isArray(p.paths) ? p.paths : [p.path]).filter(Boolean) as string[];
    if (!list.length) fail('INVALID_SCHEMA', '没有路径', 'asset.import {p, paths:[...]}');
    const kinds: LabImg['kind'][] = ['bottle', 'cup', 'shoe'];
    const made = list.map((raw, i) => {
      const name = String(raw).split(/[\\/]/).pop() ?? `image-${i}.png`;
      if (!ASSET_LIKE.test(name)) fail('NOT_FOUND', `no such file: ${ROOT}/${String(raw)}`, 'asset.import 需要工作区内的相对路径或绝对路径');
      const kind = /shoe|run|night/i.test(name) ? 'shoe' : /coffee|cup|latte/i.test(name) ? 'cup' : kinds[(pr.doc.assets.length + i) % 3];
      const a = mkAsset(pr.doc.project_id, name, kind, kind === 'shoe' ? 1600 : 1400, kind === 'cup' ? 1400 : 1800);
      a.id = rid('a');
      a.uri = `assets/${name}`;
      pr.doc.assets.push(a);
      return a;
    });
    /* engine shape: {assets:[{id,name,w,h,bytes,sha256,dup,uri,palette,…}], ok, rev} */
    const r = commit(pr, p, made.map((x) => x.id), made.map((x) => x.id)) as V;
    return {
      ...r,
      assets: made.map((a) => ({
        id: a.id, name: a.name, w: a.width, h: a.height, bytes: a.bytes, kind: a.kind,
        sha256: a.sha256 ?? '', uri: a.uri, mime: a.mime, alpha: a.alpha,
        palette: a.palette, dup: null,
      })),
    };
  },
  'asset.remove': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const aid = str(p.id);
    const used = pr.doc.nodes.filter((n) => n.image?.asset === aid).map((n) => n.id);
    if (used.length && !bool(p.force)) fail('ASSET_IN_USE', `素材被 ${used.length} 个图层引用`, 'asset.remove {force:true} 会一起删掉这些图层');
    pr.doc.assets = pr.doc.assets.filter((a) => a.id !== aid);
    pr.doc.nodes = pr.doc.nodes.filter((n) => n.image?.asset !== aid);
    for (const ab of pr.doc.artboards) ab.layers = ab.layers.filter((x) => !used.includes(x));
    return commit(pr, p, used);
  },
  'asset.replace': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const a = pr.doc.assets.find((x) => x.id === str(p.id));
    if (!a) fail('NOT_FOUND', 'no asset');
    a.name = (str(p.path, a.name).split(/[\\/]/).pop() ?? a.name) as string;
    a.thumb = svgData(photoSvg('bottle', 420, 420));
    return commit(pr, p, [a.id]);
  },
  'asset.missing': (p) => {
    const pr = need(p);
    return pr.doc.nodes
      .filter((n) => n.image && !pr.doc.assets.some((a) => a.id === n.image!.asset))
      .map((n) => ({ id: n.id, uri: n.image!.asset }));
  },
  'asset.thumb': (p) => {
    const pr = need(p);
    const a = pr.doc.assets.find((x) => x.id === str(p.id));
    if (!a) fail('NOT_FOUND', 'no asset');
    return { path: a.uri, url: a.thumb, w: num(p.w, 256) };
  },

  'canvas.add': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const ab = abOf(pr, p);
    const box = { x: num(p.x, 0), y: num(p.y, 0), w: num(p.w, ab.width / 2), h: num(p.h, ab.height / 4) };
    const n = buildNode(pr, p, box);
    pr.doc.nodes.push(n);
    ab.layers.splice(typeof p.at === 'number' ? p.at : ab.layers.length, 0, n.id);
    if (bool(p.solve, true)) solveBoard(pr, ab.id);
    return { ...commit(pr, p, [n.id], [n.id]), id: n.id };
  },
  'canvas.add_many': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const ab = abOf(pr, p);
    const made: string[] = [];
    for (const spec of arr(p.nodes)) {
      const box = { x: num(spec.x, 0), y: num(spec.y, 0), w: num(spec.w, ab.width / 2), h: num(spec.h, ab.height / 4) };
      const n = buildNode(pr, spec, box);
      pr.doc.nodes.push(n);
      ab.layers.push(n.id);
      made.push(n.id);
    }
    solveBoard(pr, ab.id);
    return { ...commit(pr, p, made, made), ids: made };
  },
  'canvas.update': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const n = nodeOf(pr, p);
    if (n.locked && !bool(p.force)) fail('LOCKED', `「${n.name}」已锁定`, '先解锁，或传 force:true');
    const patch: V = {};
    if (p.s) patch.style = { ...(n.style as V), ...(p.s as V) };
    if (p.l) patch.layout = { ...n.layout, ...(p.l as V) };
    if (p.box) patch.box = { ...n.box, ...(p.box as V) };
    if (p.intent) patch.layout = { ...(patch.layout ?? n.layout), mode: str(p.mode, 'constraint'), intent: p.intent, provenance: [] };
    if (p.text) patch.text = { ...(n.text ?? {}), content: p.text };
    if (p.fx !== undefined) patch.fx = p.fx;
    for (const k of ['rotation', 'opacity', 'visible', 'locked', 'blend', 'z_role', 'name']) if (p[k] !== undefined) patch[k] = p[k];
    merge(n as unknown as V, patch);
    if (n.type === 'text' && n.text) n.text.lines = wrapText(n.text.content, n.text.size ?? 16, n.text.letter_spacing ?? 0, Math.max(20, n.box.w));
    return commit(pr, p, [n.id]);
  },
  'canvas.set': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const list = idsOf(p.ids).length ? idsOf(p.ids) : [str(p.id)];
    const patch = clone((p.patch ?? p.s ?? {}) as V);
    for (const nid of list) {
      const n = pr.doc.nodes.find((x) => x.id === nid);
      if (!n) fail('NODE_NOT_FOUND', `no node "${nid}"`);
      if (n.locked && !bool(p.force)) fail('LOCKED', `「${n.name}」已锁定`);
      merge(n as unknown as V, patch);
    }
    return commit(pr, p, list);
  },
  'canvas.text': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const n = nodeOf(pr, p);
    if (!n.text) fail('INVALID_SCHEMA', 'not a text node', 'canvas.add {t:"text"} 先建文字图层');
    n.text.content = str(p.content, n.text.content);
    n.text.lines = wrapText(n.text.content, n.text.size ?? 16, n.text.letter_spacing ?? 0, Math.max(20, n.box.w));
    return commit(pr, p, [n.id]);
  },
  'canvas.image': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const n = nodeOf(pr, p);
    if (!n.image) fail('INVALID_SCHEMA', 'not an image node');
    n.image.asset = str(p.asset, n.image.asset);
    if (typeof p.fit === 'string') n.image.fit = p.fit as never;
    n.style = { ...(n.style ?? {}), fill: { t: 'image', asset: n.image.asset, fit: n.image.fit, opacity: 1 } };
    return commit(pr, p, [n.id]);
  },
  'canvas.transform': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const n = nodeOf(pr, p);
    if (n.locked && !bool(p.force)) fail('LOCKED', `「${n.name}」已锁定`, '先解锁，或传 force:true');
    n.box = { x: num(p.x, n.box.x), y: num(p.y, n.box.y), w: num(p.w, n.box.w), h: num(p.h, n.box.h) };
    if (typeof p.rot === 'number') n.rotation = p.rot;
    if (typeof p.flip_x === 'boolean') n.flip_x = p.flip_x;
    if (typeof p.flip_y === 'boolean') n.flip_y = p.flip_y;
    if (n.layout.mode === 'constraint' && !bool(p.keep_intent)) n.layout.mode = 'manual';
    return commit(pr, p, [n.id]);
  },
  'canvas.style': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const n = nodeOf(pr, p);
    const sp: V = {};
    for (const k of ['fill', 'stroke', 'radius', 'color']) if (p[k] !== undefined) sp[k] = p[k];
    if (Object.keys(sp).length) n.style = merge({ ...(n.style as V) }, sp) as Node['style'];
    for (const k of ['opacity', 'blend', 'visible', 'locked', 'rotation', 'name', 'z_role']) if (p[k] !== undefined) (n as unknown as V)[k] = p[k];
    if (p.fx !== undefined) n.fx = p.fx as Node['fx'];
    return commit(pr, p, [n.id]);
  },
  'canvas.reorder': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const n = nodeOf(pr, p);
    const ab = abOf(pr, p);
    for (const a of pr.doc.artboards) a.layers = a.layers.filter((x) => x !== n.id);
    const par = n.parent ? pr.doc.nodes.find((x) => x.id === n.parent) : null;
    if (par) par.children = (par.children ?? []).filter((x) => x !== n.id);
    n.parent = undefined;
    const to = Math.max(0, Math.min(num(p.to, ab.layers.length), ab.layers.length));
    if (p.parent) {
      const target = nodeOf(pr, { id: p.parent });
      target.children = target.children ?? [];
      target.children.splice(Math.min(to, target.children.length), 0, n.id);
      n.parent = target.id;
      n.artboard = target.artboard;
    } else {
      n.artboard = ab.id;
      ab.layers.splice(to, 0, n.id);
    }
    return { ...commit(pr, p, [n.id]), z: to };
  },
  'canvas.group': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    if (p.id && !p.ids) {
      const g = nodeOf(pr, p);
      const kids = [...(g.children ?? [])];
      const ab = pr.doc.artboards.find((a) => a.id === g.artboard);
      if (!ab) fail('ARTBOARD_NOT_FOUND', 'group 没有画板');
      const at = ab.layers.indexOf(g.id);
      pr.doc.nodes = pr.doc.nodes.filter((x) => x.id !== g.id);
      ab.layers.splice(at, 1, ...kids);
      for (const k of kids) {
        const n = pr.doc.nodes.find((x) => x.id === k);
        if (n) n.parent = undefined;
      }
      return commit(pr, p, kids);
    }
    const list = idsOf(p.ids);
    const nodes = pr.doc.nodes.filter((n) => list.includes(n.id));
    if (!nodes.length) fail('INVALID_SCHEMA', 'ids 为空');
    const ab = pr.doc.artboards.find((a) => a.id === nodes[0].artboard);
    if (!ab) fail('ARTBOARD_NOT_FOUND', '找不到画板');
    const g = mkNode('群组', 'group', {
      x: Math.min(...nodes.map((n) => n.box.x)),
      y: Math.min(...nodes.map((n) => n.box.y)),
      w: Math.max(...nodes.map((n) => n.box.x + n.box.w)) - Math.min(...nodes.map((n) => n.box.x)),
      h: Math.max(...nodes.map((n) => n.box.y + n.box.h)) - Math.min(...nodes.map((n) => n.box.y)),
    });
    g.id = rid('n_group');
    g.artboard = ab.id;
    g.children = list;
    nodes.forEach((n) => {
      n.parent = g.id;
    });
    const at = Math.min(...list.map((x) => ab.layers.indexOf(x)));
    ab.layers = ab.layers.filter((x) => !list.includes(x));
    ab.layers.splice(at, 0, g.id);
    pr.doc.nodes.push(g);
    return { ...commit(pr, p, list, [g.id]), id: g.id };
  },
  'canvas.ungroup': (p) => OPS['canvas.group']({ p: p.p, id: p.id, rev: p.rev }),
  'canvas.duplicate': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const list = idsOf(p.ids).length ? idsOf(p.ids) : [str(p.id)];
    const made: string[] = [];
    for (const srcId of list) {
      const src = nodeOf(pr, { id: srcId });
      const copy = clone(src);
      copy.id = rid('n');
      copy.name = `${src.name} 拷贝`;
      copy.parent = src.parent;
      copy.box = { ...src.box, x: src.box.x + num(p.dx, 28), y: src.box.y + num(p.dy, 28) };
      pr.doc.nodes.push(copy);
      const ab = pr.doc.artboards.find((a) => a.id === src.artboard);
      if (ab) {
        if (src.parent) {
          const par = pr.doc.nodes.find((x) => x.id === src.parent);
          if (par) par.children = [...(par.children ?? []), copy.id];
        } else ab.layers.splice(ab.layers.indexOf(srcId) + 1, 0, copy.id);
      }
      made.push(copy.id);
    }
    return { ...commit(pr, p, made, made), ids: made };
  },
  'canvas.delete': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const list = idsOf(p.ids).length ? idsOf(p.ids) : [str(p.id)];
    const gone = new Set<string>();
    for (const nid of list) {
      const n = nodeOf(pr, { id: nid });
      if (n.locked && !bool(p.force)) fail('LOCKED', `「${n.name}」已锁定`, '先解锁，或传 force:true');
      for (const x of orderIds(pr.doc, { layers: [n.id] } as never)) gone.add(x);
      gone.add(n.id);
    }
    pr.doc.nodes = pr.doc.nodes.filter((n) => !gone.has(n.id));
    for (const ab of pr.doc.artboards) {
      ab.layers = ab.layers.filter((x) => !gone.has(x));
      for (const n of pr.doc.nodes) n.children = (n.children ?? []).filter((c) => !gone.has(c));
    }
    return { ...commit(pr, p, [...gone]), deleted: [...gone] };
  },
  'canvas.align': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const list = idsOf(p.ids);
    const nodes = pr.doc.nodes.filter((n) => list.includes(n.id) && !n.locked);
    if (!nodes.length) fail('INVALID_SCHEMA', 'ids 为空或都已锁定');
    const ab = pr.doc.artboards.find((a) => a.id === nodes[0].artboard);
    const W = ab ? ab.width : Math.max(...nodes.map((n) => n.box.x + n.box.w));
    const H = ab ? ab.height : Math.max(...nodes.map((n) => n.box.y + n.box.h));
    const axis = str(p.axis, 'x');
    const how = str(p.how, 'center');
    for (const n of nodes) {
      if (axis === 'x') n.box = { ...n.box, x: Math.round(how === 'left' ? 0 : how === 'right' ? W - n.box.w : (W - n.box.w) / 2) };
      else n.box = { ...n.box, y: Math.round(how === 'top' ? 0 : how === 'bottom' ? H - n.box.h : (H - n.box.h) / 2) };
      n.layout.mode = 'manual';
    }
    return commit(pr, p, list);
  },
  'canvas.distribute': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const list = idsOf(p.ids);
    const nodes = pr.doc.nodes.filter((n) => list.includes(n.id));
    const axis = str(p.axis, 'x');
    if (nodes.length > 2) {
      nodes.sort((a, b) => (axis === 'x' ? a.box.x - b.box.x : a.box.y - b.box.y));
      const first = nodes[0];
      const last = nodes[nodes.length - 1];
      const span =
        (axis === 'x' ? last.box.x + last.box.w - first.box.x : last.box.y + last.box.h - first.box.y) -
        nodes.reduce((s, n) => s + (axis === 'x' ? n.box.w : n.box.h), 0);
      const gap = span / (nodes.length - 1);
      let cur = axis === 'x' ? first.box.x : first.box.y;
      for (const n of nodes) {
        n.box = axis === 'x' ? { ...n.box, x: Math.round(cur) } : { ...n.box, y: Math.round(cur) };
        cur += (axis === 'x' ? n.box.w : n.box.h) + gap;
      }
    }
    return commit(pr, p, list);
  },
  'canvas.batch': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const changed: string[] = [];
    const created: string[] = [];
    for (const raw of arr(p.cmds)) {
      const tag = String(raw.t ?? raw.cmd ?? '').toLowerCase();
      const map: Record<string, string> = {
        transform: 'canvas.transform',
        nodeset: 'canvas.set',
        nodesset: 'canvas.set',
        style: 'canvas.style',
        text: 'canvas.text',
        layoutintent: 'layout.apply',
        nodeadd: 'canvas.add',
        nodedel: 'canvas.delete',
        nodemove: 'canvas.reorder',
        boardadd: 'artboard.create',
        boardset: 'artboard.set',
        boarddel: 'artboard.delete',
        group: 'canvas.group',
        ungroup: 'canvas.ungroup',
        duplicate: 'canvas.duplicate',
        assetadd: 'asset.import',
        solve: 'layout.resolve',
        reflow: 'layout.reflow',
      };
      const method = map[tag];
      if (!method) fail('INVALID_SCHEMA', `unknown cmd "${String(raw.t)}"`, 't 取 NodeAdd|NodeSet|Transform|Style|Text|LayoutIntent|Composite');
      if (method === 'canvas.delete') {
        const r = OPS['canvas.delete']({ p: p.p, ids: [raw.id ?? raw.ids].flat(), force: raw.force ?? true }) as V;
        changed.push(...((r.changed as string[]) ?? []));
        continue;
      }
      if (method === 'canvas.set') {
        const r = OPS['canvas.set']({ p: p.p, ids: raw.ids ?? [raw.id], patch: raw.patch ?? raw, force: true }) as V;
        changed.push(...((r.changed as string[]) ?? []));
        continue;
      }
      if (method === 'layout.apply') {
        const r = OPS['layout.apply']({ p: p.p, id: raw.id, intent: raw.intent, mode: raw.mode }) as V;
        changed.push(...((r.changed as string[]) ?? []));
        continue;
      }
      const r = OPS[method]({ ...raw, p: p.p, b: raw.b ?? p.b, force: true }) as V;
      changed.push(...((r.changed as string[]) ?? []));
      created.push(...((r.created as string[]) ?? []));
    }
    return { ...commit(pr, p, [...new Set(changed)], created), changed: [...new Set(changed)], created };
  },

  'layout.apply': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const n = nodeOf(pr, p);
    n.layout = { mode: str(p.mode, 'constraint') as never, intent: (p.intent ?? {}) as Intent, provenance: [] };
    const rep = solveBoard(pr, n.artboard ?? null);
    return { ...commit(pr, p, rep.changed), box: n.box, prov: n.layout.provenance, conflicts: rep.conflicts };
  },
  'layout.set_mode': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const n = nodeOf(pr, p);
    n.layout = { ...n.layout, mode: str(p.mode, 'manual') as never };
    return commit(pr, p, [n.id]);
  },
  'layout.resolve': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const rep = solveBoard(pr, p.b ? str(p.b) : null);
    return { ...commit(pr, p, rep.changed), conflicts: rep.conflicts, passes: 3 };
  },
  'layout.lock': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const n = nodeOf(pr, p);
    n.layout = { ...n.layout, pinned: bool(p.pinned, true) };
    return commit(pr, p, [n.id]);
  },
  'layout.explain': (p) => {
    const pr = need(p);
    const n = nodeOf(pr, p);
    const it = n.layout.intent;
    const steps: V[] = [];
    if (it?.size) steps.push({ intent: 'size', won: true, note: `尺寸 ${it.size.mode ?? 'fixed'} · ${Math.round(n.box.w)}×${Math.round(n.box.h)}` });
    if (it?.anchor) steps.push({ intent: 'anchor', won: !!it.anchor, note: `锚点 ${it.anchor} · 上 ${it.margin_top ?? 0} 左 ${it.margin_left ?? 0}` });
    const relKey = it?.below ? 'below' : it?.above ? 'above' : it?.left_of ? 'left_of' : it?.right_of ? 'right_of' : '';
    if (relKey && it) {
      const r = (it as V)[relKey] as V;
      steps.push({ intent: relKey, won: true, note: `参照 ${str(r.of)} · 间距 ${num(r.gap, 0)}` });
    }
    if (it?.align_to) steps.push({ intent: 'align_to', won: true, note: `${it.align_to.edge} → ${it.align_to.of}` });
    if (it?.center_in) steps.push({ intent: 'center_in', won: true, note: it.center_in });
    if (it?.safe_margin !== undefined) steps.push({ intent: 'safe_margin', won: true, note: `${it.safe_margin}px` });
    return {
      id: n.id,
      name: n.name,
      mode: n.layout.mode,
      pinned: !!n.layout.pinned,
      order: ['size', 'anchor', relKey || 'align_to', 'align', 'center_in', 'safe_margin'],
      steps,
      box: n.box,
      provenance: n.layout.provenance ?? [],
    };
  },
  'layout.reflow': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const ab = abOf(pr, p);
    const w = num(p.w, ab.width);
    const h = num(p.h, ab.height);
    const mode = str(p.mode, 'reflow');
    const list = orderIds(pr.doc, ab);
    for (const nid of list) {
      const n = pr.doc.nodes.find((x) => x.id === nid);
      if (!n || n.layout.mode === 'manual') continue;
      const sx = w / ab.width;
      const sy = h / ab.height;
      n.box =
        mode === 'scale'
          ? { x: n.box.x * sx, y: n.box.y * sy, w: n.box.w * sx, h: n.box.h * sy }
          : { x: n.box.x * sx, y: n.box.y, w: n.box.w * sx, h: n.box.h };
    }
    ab.width = w;
    ab.height = h;
    solveBoard(pr, ab.id);
    return commit(pr, p, list);
  },

  'inspect.validate': (p) => {
    const pr = need(p);
    const issues = validate(pr, p.board ? str(p.board) : p.b ? str(p.b) : null);
    const errors = issues.filter((x) => x.s === 'error').length;
    return {
      issues,
      ok: errors === 0,
      errors,
      warns: issues.filter((x) => x.s === 'warn').length,
      infos: issues.filter((x) => x.s !== 'error' && x.s !== 'warn').length,
      boards: pr.doc.artboards.map((a) => ({ b: a.id, nodes: a.layers.length, ink: 0.42 })),
    };
  },
  'inspect.fix': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const found = validate(pr, p.board ? str(p.board) : null).filter((x) => x.f);
    const chosen: Issue[] = Array.isArray(p.issues) && (p.issues as Issue[]).length ? (p.issues as Issue[]) : found;
    const cmds = chosen.map((x) => x.f as V).filter(Boolean);
    if (bool(p.dry)) return { cmds, n: cmds.length, changed: [] };
    if (!cmds.length) return { ok: true, rev: pr.doc.revision, changed: [], cmds: [], warnings: [] };
    const r = OPS['canvas.batch']({ p: p.p, cmds }) as V;
    return { ...r, cmds, n: cmds.length };
  },
  'inspect.node': (p) => {
    const pr = need(p);
    const n = nodeOf(pr, p);
    return { id: n.id, name: n.name, type: n.type, board: n.artboard, box: n.box, rotation: n.rotation, opacity: n.opacity, style: n.style ?? null, layout: n.layout, text: n.text ?? null };
  },
  'inspect.geometry': (p) => {
    const pr = need(p);
    return idsOf(p.ids).map((nid) => {
      const n = pr.doc.nodes.find((x) => x.id === nid);
      return { id: nid, box: n?.box ?? null, rot: n?.rotation ?? 0 };
    });
  },
  'inspect.text': (p) => {
    const pr = need(p);
    const n = nodeOf(pr, p);
    if (!n.text) fail('INVALID_SCHEMA', 'not a text node');
    const size = n.text.size ?? 16;
    const lines = wrapText(n.text.content, size, n.text.letter_spacing ?? 0, Math.max(20, n.box.w));
    const h = lines.length * (n.text.line_ratio ?? 1.35) * size;
    return {
      lines,
      w: Math.round(Math.max(...lines.map((l) => measure(l, size, n.text!.letter_spacing ?? 0)))),
      h: Math.round(h),
      font: n.text.family ?? 'system-ui',
      overflow: h > n.box.h + 1,
      fell_back: false,
    };
  },
  'inspect.conflicts': (p) => {
    const pr = need(p);
    return solveBoard(pr, str(p.b) || null).conflicts;
  },
  'inspect.assets': (p) => {
    const pr = need(p);
    return pr.doc.assets.map((a) => ({ id: a.id, name: a.name, used: pr.doc.nodes.filter((n) => n.image?.asset === a.id).length, bytes: a.bytes }));
  },
  'inspect.document': (p) => {
    const pr = need(p);
    return pr.doc.artboards.map((ab) => ({
      b: ab.id,
      name: ab.name,
      w: ab.width,
      h: ab.height,
      layers: orderIds(pr.doc, ab).map((nid) => {
        const n = pr.doc.nodes.find((x) => x.id === nid);
        return { id: nid, name: n?.name, t: n?.type, box: n?.box, mode: n?.layout.mode };
      }),
    }));
  },

  'render.svg': (p) => {
    const pr = need(p);
    return compileDocSvg(pr.doc, p.b ? str(p.b) : null, {
      preview: bool(p.preview),
      omit: idsOf(p.omit),
      scale: num(p.scale, 1),
      guides: bool(p.guides),
    });
  },
  'render.preview': (p) => {
    const pr = need(p);
    const ab = p.b ? abOf(pr, p) : pr.doc.artboards.find((a) => a.visible)!;
    const sc = num(p.sc, 0.4);
    const s = compileBoard(pr.doc, ab, { omit: idsOf(p.omit) });
    return {
      path: `${pr.dir}/preview.png`,
      url: svgData(s),
      w: Math.round(ab.width * sc),
      h: Math.round(ab.height * sc),
      bytes: s.length,
      sha: `${ab.id}${pr.doc.revision}`.padEnd(64, '7c').slice(0, 64),
    };
  },
  'render.export': (p) => {
    const pr = need(p);
    const fmt = str(p.f, 'png');
    const sc = num(p.sc, 1);
    const q = num(p.q, 92);
    const list = idsOf(p.boards).length ? idsOf(p.boards) : p.b ? [str(p.b)] : pr.doc.artboards.filter((a) => a.visible).map((a) => a.id);
    const mime: Record<string, string> = { png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', webp: 'image/webp', svg: 'image/svg+xml', pdf: 'application/pdf' };
    const files = list.map((bid) => {
      const ab = pr.doc.artboards.find((a) => a.id === bid);
      if (!ab) fail('ARTBOARD_NOT_FOUND', `no artboard "${bid}"`);
      const s = compileBoard(pr.doc, ab);
      const bytes = fmt === 'svg' ? s.length : Math.round(ab.width * ab.height * sc * sc * (fmt === 'png' ? 0.4 : fmt === 'pdf' ? 0.34 : 0.12 * (q / 92)));
      return {
        path: `${pr.dir}/exports/${pr.doc.name}-${ab.name.replace(/\s+/g, '')}.${fmt}`,
        f: fmt,
        w: Math.round(ab.width * sc),
        h: Math.round(ab.height * sc),
        bytes,
        sha: `${bid}${fmt}${sc}${q}`.padEnd(64, '3f').slice(0, 64),
        mime: mime[fmt] ?? 'application/octet-stream',
        url: svgData(s),
      };
    });
    jobs.unshift({ id: rid('j'), kind: 'export', label: `导出 ${files.length} 个 · ${fmt.toUpperCase()} @${sc}x`, done: 1, stage: 'done', ok: true });
    return { ok: true, files, n: files.length };
  },
  'render.jobs': () => jobs,
  jobs: () => jobs,
  'render.cancel': (p) => {
    const j = jobs.find((x) => x.id === str(p.id));
    if (j) {
      j.ok = false;
      j.stage = 'cancelled';
    }
    return { ok: !!j };
  },
  cancel: (p) => OPS['render.cancel'](p),
  'render.outputs': (p) => {
    const pr = store.get(str(p.p));
    if (!pr) return [];
    return pr.doc.artboards.map((a) => ({ path: `${pr.dir}/exports/${a.name.replace(/\s+/g, '')}.png`, bytes: Math.round(a.width * a.height * 0.3), f: 'png' }));
  },
  'render.diff': () => ({ score: 99.2, mae: [0.2, 0.1, 0.4], w: 1080, h: 1440, box: { x: 90, y: 1096, w: 268, h: 76 } }),

  'history.undo': (p) => {
    const pr = need(p);
    const n = Math.max(1, num(p.n, 1));
    if (pr.past.length < n) fail('INVALID_SCHEMA', '没有可撤销的步骤');
    for (let i = 0; i < n; i += 1) {
      const prev = pr.past.pop()!;
      pr.future.push(JSON.stringify(pr.doc));
      pr.doc = JSON.parse(prev);
    }
    pr.doc.revision += 1;
    return { ok: true, rev: pr.doc.revision, did: n, changed: [] };
  },
  'history.redo': (p) => {
    const pr = need(p);
    const n = Math.max(1, num(p.n, 1));
    if (pr.future.length < n) fail('INVALID_SCHEMA', '没有可重做的步骤');
    for (let i = 0; i < n; i += 1) {
      const next = pr.future.pop()!;
      pr.past.push(JSON.stringify(pr.doc));
      pr.doc = JSON.parse(next);
    }
    pr.doc.revision += 1;
    return { ok: true, rev: pr.doc.revision, did: n, changed: [] };
  },
  'history.list': (p) => {
    const pr = need(p);
    const rows: V[] = [];
    const trail = ['canvas.transform', 'canvas.style', 'layout.apply', 'canvas.add', 'canvas.delete', 'canvas.duplicate', 'text.fonts', 'asset.import'];
    const n = Math.min(num(p.n, 20), Math.max(8, pr.past.length + 4));
    for (let i = 0; i < n; i += 1) {
      rows.push({
        rev: Math.max(1, pr.doc.revision - i),
        t: trail[i % trail.length],
        origin: i === 2 ? 'mcp' : i === 5 ? 'http' : 'desktop',
        at: new Date(Date.now() - i * 1000 * 233).toISOString(),
        changed: [`n_${['photo', 'headline', 'badge', 'body', 'sub'][i % 5]}`],
      });
    }
    return rows;
  },
  'history.checkpoint': (p) => ({ ok: true, rev: need(p).doc.revision }),
  'history.rollback': (p) => OPS['project.restore'](p),

  'template.list': () => [
    { tpl: 'poster_kv', d: '主视觉海报 · 大图 + 大标题', builtin: true },
    { tpl: 'ecom_main', d: '电商主图 · 白底 800×800', builtin: true },
    { tpl: 'story_min', d: '9:16 故事 · 极简排布', builtin: true },
    { tpl: 'card_quote', d: '金句卡片 · 16:5', builtin: true },
    ...[...store.values()].flatMap((pr) =>
      pr.doc.artboards.filter((a) => a.template_id).map((a) => ({ tpl: a.name, d: `${pr.doc.name} · ${a.name}`, builtin: false })),
    ),
  ],
  'template.builtin': () => [
    { tpl: 'poster_kv', d: '主视觉海报' },
    { tpl: 'ecom_main', d: '电商主图' },
    { tpl: 'story_min', d: '9:16 故事' },
    { tpl: 'card_quote', d: '金句卡片' },
  ],
  'template.save': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const ab = abOf(pr, p);
    const name = str(p.name, `${ab.name} 模板`);
    ab.template_id = name;
    return { ...commit(pr, p, [ab.id]), tpl: name };
  },
  'template.use': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const ab = abOf(pr, p);
    const tpl = str(p.tpl, 'poster_kv');
    const vars = (p.vars ?? {}) as V;
    const made: string[] = [];
    const add = (n: Node) => {
      put(pr, ab, n);
      made.push(n.id);
    };
    add(
      mkNode(`模板底 · ${tpl}`, 'rect', { x: 0, y: 0, w: ab.width, h: ab.height }, {
        id: rid('n'),
        style: { fill: linear([0, '#ffffff'], [1, '#eef1fb']) },
        layout: lay({ size: { mode: 'fill' } }),
      }),
    );
    add(
      mkNode(`模板标题 · ${tpl}`, 'text', { x: ab.width * 0.08, y: ab.height * 0.42, w: ab.width * 0.84, h: ab.height * 0.14 }, {
        id: rid('n'),
        text: { content: str(vars.title, '少即是多'), family: 'PingFang SC', size: Math.round(ab.width * 0.09), weight: 800, color: '#171d3f', align: 'left', valign: 'top', line_ratio: 1.12 },
        layout: lay({ center_in: ab.id, size: { w_ref: 0.84 } }),
      }),
    );
    add(
      mkNode(`模板色条 · ${tpl}`, 'rect', { x: ab.width * 0.08, y: ab.height * 0.6, w: ab.width * 0.14, h: 8 }, {
        id: rid('n'),
        style: { fill: { t: 'linear', angle: 90, stops: [{ at: 0, color: str(vars.accent, '#4b6bfb') }, { at: 1, color: '#8b5cf6' }] }, radius: { all: 4 } },
        layout: lay({ anchor: 'left', margin_left: Math.round(ab.width * 0.08) }),
      }),
    );
    solveBoard(pr, ab.id);
    return { ...commit(pr, p, made, made), ids: made };
  },
  'template.vars': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const vars = (p.vars ?? {}) as V;
    const changed: string[] = [];
    for (const n of pr.doc.nodes) {
      if (n.type !== 'text') continue;
      if (typeof vars.title === 'string' && n.name.includes('标题')) {
        n.text!.content = vars.title;
        changed.push(n.id);
      }
      if (typeof vars.subtitle === 'string' && n.name.includes('副')) {
        n.text!.content = vars.subtitle;
        changed.push(n.id);
      }
      if (typeof vars.body === 'string' && n.name.includes('正文')) {
        n.text!.content = vars.body;
        changed.push(n.id);
      }
    }
    for (const idv of changed) {
      const n = pr.doc.nodes.find((x) => x.id === idv);
      if (n?.text) n.text.lines = wrapText(n.text.content, n.text.size ?? 16, n.text.letter_spacing ?? 0, Math.max(20, n.box.w));
    }
    return commit(pr, p, changed);
  },
  'template.variants': (p) => {
    const pr = need(p);
    checkRev(pr, p);
    const src = pr.doc.artboards[Math.min(num(p.from, 0), pr.doc.artboards.length - 1)] ?? pr.doc.artboards[0];
    const colors = idsOf(p.colors).length ? idsOf(p.colors) : ['#4B6BFB', '#FF7A59', '#22D3EE'];
    const copies = ((p.copy as V)?.title ?? []) as string[];
    const ids: string[] = [];
    const boards: string[] = [];
    colors.forEach((c, i) => {
      const ab = mkBoard(pr.doc.project_id, `${src.name} ${i + 1}`, src.width, src.height, clone(src.background!), src.safe_margin ?? 0);
      for (const lid of orderIds(pr.doc, src)) {
        const s = pr.doc.nodes.find((n) => n.id === lid);
        if (!s) continue;
        const n = clone(s);
        n.id = rid('n');
        n.artboard = ab.id;
        if (n.style?.fill && 'color' in n.style.fill && (n.style.fill as { color: string }).color !== '#ffffff') {
          (n.style.fill as { color: string }).color = c;
        }
        if (n.fx?.shadows?.length) n.fx.shadows[0].color = `${c}59`;
        if (typeof copies[i] === 'string' && n.type === 'text' && n.role === 'headline') n.text!.content = copies[i];
        pr.doc.nodes.push(n);
        ab.layers.push(n.id);
        ids.push(n.id);
      }
      pr.doc.artboards.push(ab);
      boards.push(ab.id);
    });
    return { ...commit(pr, p, ids, boards), ids, boards };
  },

  'image.probe': (p) => {
    const path = str(p.path, str(p.in, 'image.png'));
    const img = labOf(path);
    const name = path.split(/[\\/]/).pop() ?? 'image.png';
    return {
      path,
      name,
      w: img.w,
      h: img.h,
      fmt: name.split('.').pop()?.toUpperCase() ?? 'PNG',
      alpha: false,
      bytes: Math.round(img.w * img.h * 2.4),
      sha: `${name}${img.w}`.padEnd(64, '0').slice(0, 64),
      orientation: img.h > img.w ? 'portrait' : img.w > img.h ? 'landscape' : 'square',
      url: svgData(labSvg(img)),
      palette: ['#1d2544', '#4b6bfb', '#eef1fb', '#c79c6d', '#f6f7fb'],
    };
  },
  'image.ops': () =>
    [
      ['crop', '按比例或像素裁切'],
      ['resize', '指定宽高'],
      ['scale', '按比例缩放'],
      ['pad', '四周补白'],
      ['rotate', '旋转角度'],
      ['flip', '水平 / 垂直翻转'],
      ['round', '圆角'],
      ['border', '描边 + 内缩'],
      ['adjust', '亮度 / 对比 / 饱和'],
      ['blur', '高斯模糊'],
      ['sharpen', '卷积锐化'],
      ['noise', '颗粒噪声'],
      ['gray', '去色'],
      ['invert', '反相'],
      ['threshold', '二值化'],
      ['tint', '主色染色'],
      ['watermark', '文字水印'],
      ['background', '铺底色'],
      ['remove_bg', '按边缘色抠底（确定性）'],
      ['strip', '拼接长条'],
    ].map(([op, d]) => ({ op, d })),
  'image.presets': () => Object.entries(PRESETS).map(([name, d]) => ({ name, d })),
  'image.op': (p) => {
    const img = labOf(str(p.in, str(p.path, 'image.png')));
    applyOp(img, str(p.op), (p.args ?? p) as V);
    return { out: str(p.out, `out/${str(p.op)}.${str(p.fmt, 'png')}`), url: svgData(labSvg(img)), w: img.w, h: img.h, bytes: Math.round(img.w * img.h * 1.8), sha: `${img.w}x${img.h}`.padEnd(64, '9').slice(0, 64) };
  },
  'image.chain': (p) => {
    const img = labOf(str(p.in, 'image.png'));
    const chain = typeof p.chain === 'string' ? parseChain(p.chain) : arr(p.chain);
    for (const step of chain) applyOp(img, str(step.op, str(step.t)), step as V);
    const out = str(p.out, `out/final.${str(p.fmt, 'png')}`);
    return {
      out,
      url: svgData(labSvg(img)),
      w: img.w,
      h: img.h,
      bytes: Math.round(img.w * img.h * (str(p.fmt, 'png') === 'png' ? 2.2 : 0.34 * num(p.q, 92) / 92) * 1.1),
      sha: `${img.w}x${img.h}${chain.length}`.padEnd(64, '5').slice(0, 64),
      steps: chain.length,
    };
  },
  'image.batch': (p) => {
    const paths = idsOf(p.paths).length ? idsOf(p.paths) : [str(p.in)];
    const chain = typeof p.chain === 'string' ? parseChain(p.chain) : arr(p.chain);
    const outDir = str(p.out, 'out');
    const fmt = str(p.fmt, 'png');
    const q = num(p.q, 92);
    const job = jobs.find((j) => j.id === str(p.job));
    if (job) job.done = 1;
    const r = paths.map((path, i) => {
      const name = path.split(/[\\/]/).pop() ?? `image-${i}`;
      if (/\.heic$|\.tiff$/i.test(name)) return { i, ok: false, e: `不支持的格式 ${name}`, code: 'UNSUPPORTED', name };
      const img = labOf(path);
      if (p.preset) for (const step of parseChain(PRESETS[str(p.preset)] ?? '')) applyOp(img, str(step.op), step);
      else for (const step of chain) applyOp(img, str(step.op), step as V);
      return {
        i,
        ok: true,
        name,
        out: `${outDir}/${name.replace(/\.[^.]+$/, '')}.${fmt}`,
        url: svgData(labSvg(img)),
        w: img.w,
        h: img.h,
        bytes: Math.round(img.w * img.h * (fmt === 'png' ? 2.1 : 0.32 * (q / 92))),
      };
    });
    return { ok: r.every((x) => x.ok), r, n: r.length };
  },
  'image.preset': (p) => {
    const img = labOf(str(p.in, 'image.png'));
    for (const step of parseChain(PRESETS[str(p.name)] ?? str(p.name, ''))) applyOp(img, str(step.op), step);
    return { out: str(p.out, `out/${str(p.name)}.${str(p.fmt, 'png')}`), url: svgData(labSvg(img)), w: img.w, h: img.h, bytes: Math.round(img.w * img.h * 1.6), sha: `${img.w}${img.h}`.padEnd(64, '2').slice(0, 64) };
  },
  'image.palette': (p) => {
    const img = labOf(str(p.path, str(p.in, 'image.png')));
    const base = img.kind === 'shoe' ? ['#101427', '#1d2544', '#39456f', '#4b6bfb', '#eef1fb', '#22d3ee'] : ['#f4efe6', '#e3d9c9', '#cbbfa9', '#c79c6d', '#2b2b2e', '#8b8f9e'];
    return base.slice(0, Math.max(3, num(p.n, 6))).map((hex, i) => ({ hex, count: Math.round(40000 / (i + 1)) }));
  },
  'image.hash': (p) => {
    const out: V = {};
    for (const path of idsOf(p.paths)) out[path.split(/[\\/]/).pop() ?? path] = { dhash: `${(path.length * 7919) % 0xffffffff}`.padStart(16, '0'), ahash: `${(path.length * 104729) % 0xffffffff}`.padStart(16, '0') };
    return out;
  },
  'image.compare': () => ({ score: 96.4, mae: [0.8, 1.1, 0.6], w: 1080, h: 1440, box: { x: 88, y: 1092, w: 272, h: 84 } }),
  'image.thumb': (p) => {
    const img = labOf(str(p.in, 'image.png'));
    const w = num(p.w, 320);
    img.w = w;
    img.h = Math.round((w * img.baseH) / img.baseW);
    return { out: str(p.out, `thumb/${w}.png`), url: svgData(labSvg(img)), bytes: Math.round(w * img.h * 1.4) };
  },

  'text.fonts': (p) => {
    const q = str(p.q).toLowerCase();
    const families = FAMILIES.filter((f) => !q || f.toLowerCase().includes(q));
    return { n: families.length, families };
  },
  'text.register': (p) => ({ ok: true, family: (str(p.path, 'Custom Font').split(/[\\/]/).pop() ?? 'Custom Font').replace(/\.[^.]+$/, '') }),
  'text.measure': (p) => {
    const text = str(p.text, '');
    const size = num(p.size, 16);
    const ls = num(p.ls, 0);
    const maxW = num(p.max_w, Infinity);
    const lh = num(p.lh, 1.35);
    const lines = wrapText(text, size, ls, maxW === 0 ? Infinity : maxW);
    return {
      w: Math.round(Math.max(...lines.map((l) => measure(l, size, ls)))),
      h: Math.round(lines.length * lh * size),
      lines,
      font: str(p.family, 'PingFang SC'),
      fell_back: !FAMILIES.includes(str(p.family, 'PingFang SC')),
    };
  },

  log: (p) => log.slice(0, Math.max(1, num(p.n, 40))),
  clear: () => {
    log.length = 0;
    jobs.length = 0;
    return { ok: true };
  },
  poster: (p) => buildPoster((p.spec ?? p) as V),
  batch: (p) => {
    const list = arr(p.jobs).length ? arr(p.jobs) : arr(p.ops);
    const r = list.map((job, i) => {
      const m = str(job.m, str(job.op, ''));
      const pp = (job.p ?? job) as V;
      try {
        const v = OPS[m]?.(pp);
        return { i, ok: true, r: v };
      } catch (e) {
        const err = e as ApiError;
        return { i, ok: false, e: err.msg ?? String(e), code: err.code ?? 'rpc' };
      }
    });
    return { ok: r.every((x) => x.ok), r };
  },
};

/** Mirrors `session::defaults()`. A key outside this list is rejected by the engine, so
 *  the mock rejects it too — otherwise the browser preview passes an edit the real
 *  settings layer would refuse. */
const CONFIG: V = {
  version: 1,
  exportDir: 'exports',
  autosaveSecs: 30,
  maxImageMp: 40,
  renderScaleMax: 4,
  logCalls: true,
  logCapLines: 2000,
  historyCap: 200,
  provider: { name: '', model: '', baseUrlSet: false, apiKeySet: false },
  tokenSet: false,
};
const ALLOWED = ['version', 'exportDir', 'autosaveSecs', 'maxImageMp', 'renderScaleMax', 'logCalls', 'logCapLines', 'historyCap', 'provider', 'token'];
const ALLOWED_PROVIDER = ['name', 'model', 'baseUrl', 'apiKeyEnv'];
/** What the mock "persists": never the token value itself in the read view. */
const cfgStore: V = { file: {}, provider: {}, token: '' };

/** The engine's read view: stored values merged over the defaults, secrets replaced by
 *  presence booleans, `token` removed. */
function cfgView(): V {
  const prov = { ...(cfgStore.provider as V) };
  const out: V = { ...CONFIG, ...(cfgStore.file as V), provider: prov, workspace: '~/AgentCanvas' };
  out.tokenSet = String(cfgStore.token ?? '').trim().length > 0;
  delete out.token;
  prov.baseUrlSet = String(prov.baseUrl ?? '').trim().length > 0;
  prov.apiKeySet = false;
  return out;
}

const FAMILIES = [
  'PingFang SC',
  'Hiragino Sans GB',
  'Microsoft YaHei',
  'Source Han Sans SC',
  'Noto Sans CJK SC',
  'Inter',
  'Söhne',
  'Helvetica Neue',
  'SF Pro Display',
  'DM Sans',
  'Manrope',
  'Space Grotesk',
  'Instrument Serif',
  'Noto Serif SC',
  'Songti SC',
  'HarmonyOS Sans SC',
  'Alibaba PuHuiTi 3.0',
  'JetBrains Mono',
];

/** The dispatcher the browser-only build talks to. */
export async function mockCall(m: string, p: V): Promise<unknown> {
  await wait();
  const fn = OPS[m] ?? OPS[m.replace(/^system\./, '')];
  if (!fn) {
    const near = Object.keys(OPS)
      .filter((k) => k.startsWith(m.split('.')[0] ?? ''))
      .slice(0, 4);
    fail('INVALID_SCHEMA', `unknown method "${m}"`, '运行 system.caps 查看完整方法表', { near });
  }
  log.unshift({ t: new Date().toISOString(), m, ms: 3 + Math.round(Math.random() * 40), ok: true, via: 'desktop', p });
  if (log.length > 200) log.pop();
  return fn(p);
}


