/** Read-only projections over the engine document. Nothing here mutates: every
 *  edit path in the app goes back through `rpc()`. */

import type { Artboard, Document, Node, NodeType, Paint, Rect } from '../types';

export type TreeRow = { node: Node; depth: number };

/** The engine answers `project.get depth:1` with its compact wire keys (`rev`, `p`).
 *  The store keeps the document's own names so the rest of the app has one vocabulary. */
export type WireDoc = Omit<Document, 'revision' | 'project_id'> & { rev?: number; p?: string };

export function wireDoc(raw: WireDoc): Document {
  const { rev, p, ...rest } = raw;
  return { ...rest, revision: rev ?? 0, project_id: p ?? '' } as Document;
}

export function nodeMap(doc: Document | null): Map<string, Node> {
  const m = new Map<string, Node>();
  if (doc) for (const n of doc.nodes) m.set(n.id, n);
  return m;
}

export function findNode(doc: Document | null, id: string | null): Node | null {
  if (!doc || !id) return null;
  return doc.nodes.find((n) => n.id === id) ?? null;
}

export function findBoard(doc: Document | null, id: string | null): Artboard | null {
  if (!doc || !id) return null;
  return doc.artboards.find((a) => a.id === id) ?? null;
}

export function visibleBoards(doc: Document | null): Artboard[] {
  if (!doc) return [];
  return doc.artboards.filter((a) => a.visible);
}

/** Top-down layer order for one artboard: last layer is on top, so paint it first
 *  in the tree the way design tools do. */
export function layerRows(doc: Document, board: Artboard): TreeRow[] {
  const out: TreeRow[] = [];
  const byId = nodeMap(doc);
  const walk = (id: string, depth: number) => {
    const n = byId.get(id);
    if (!n) return;
    out.push({ node: n, depth });
    for (let i = (n.children ?? []).length - 1; i >= 0; i -= 1) {
      const c = n.children![i];
      if (c) walk(c, depth + 1);
    }
  };
  for (let i = board.layers.length - 1; i >= 0; i -= 1) walk(board.layers[i], 0);
  return out;
}

/** Paint order (bottom first) — used for hit testing, top wins. */
export function paintOrder(doc: Document, board: Artboard): string[] {
  const out: string[] = [];
  const byId = nodeMap(doc);
  const walk = (id: string) => {
    out.push(id);
    for (const c of byId.get(id)?.children ?? []) walk(c);
  };
  for (const id of board.layers) walk(id);
  return out;
}

export function nodeInBoard(doc: Document, boardId: string): Node[] {
  const ids = new Set(paintOrder(doc, findBoard(doc, boardId) ?? { layers: [] } as never));
  return doc.nodes.filter((n) => ids.has(n.id));
}

export function unionBox(nodes: Node[]): Rect | null {
  if (!nodes.length) return null;
  let x = Infinity;
  let y = Infinity;
  let x2 = -Infinity;
  let y2 = -Infinity;
  for (const n of nodes) {
    x = Math.min(x, n.box.x);
    y = Math.min(y, n.box.y);
    x2 = Math.max(x2, n.box.x + n.box.w);
    y2 = Math.max(y2, n.box.y + n.box.h);
  }
  return { x, y, w: x2 - x, h: y2 - y };
}

export const TYPE_LABEL: Record<NodeType, string> = {
  text: '文字',
  image: '图片',
  rect: '矩形',
  ellipse: '椭圆',
  line: '直线',
  polygon: '多边形',
  path: '路径',
  group: '群组',
  frame: '容器',
  svg: 'SVG',
};

/** Colour dot per node type in the layer tree. */
export const TYPE_COLOR: Record<NodeType, string> = {
  text: '#4B6BFB',
  image: '#8B5CF6',
  rect: '#22D3EE',
  ellipse: '#FF7A59',
  line: '#94A3B8',
  polygon: '#F59E0B',
  path: '#10B981',
  group: '#64748B',
  frame: '#0EA5E9',
  svg: '#D946EF',
};

export function nodeLabel(n: Node): string {
  if (n.name) return n.name;
  if (n.type === 'text') return (n.text?.content ?? '').slice(0, 14) || TYPE_LABEL.text;
  return TYPE_LABEL[n.type] ?? n.type;
}

/** The first opaque colour of a paint, for swatch buttons. */
export function paintCss(p: Paint | undefined): string {
  if (!p) return 'transparent';
  switch (p.t) {
    case 'none':
      return 'transparent';
    case 'solid':
      return p.color;
    case 'linear':
      return `linear-gradient(${Math.round(p.angle ?? 90)}deg, ${stops(p.stops)})`;
    case 'radial':
      return `radial-gradient(circle, ${stops(p.stops)})`;
    case 'image':
      return 'repeating-linear-gradient(45deg,#dfe4f5 0 6px,#eef1fb 6px 12px)';
    default:
      return 'transparent';
  }
}

export function stops(list: { at: number; color: string }[] | undefined): string {
  const s = (list ?? []).slice().sort((a, b) => a.at - b.at);
  if (!s.length) return '#ffffff 0%,#000000 100%';
  if (s.length === 1) return `${s[0].color} 0%,${s[0].color} 100%`;
  const total = s[s.length - 1].at - s[0].at || 1;
  return s
    .map((x) => `${x.color} ${Math.round(((x.at - s[0].at) / total) * 100)}%`)
    .join(',');
}

export const BOX_HANDLES = [
  'nw',
  'n',
  'ne',
  'e',
  'se',
  's',
  'sw',
  'w',
] as const;

/** `Rect` is plain data on the wire; these are the Rust helpers `Rect::cx/cy`. */
export const cx = (r: Rect): number => r.x + r.w / 2;
export const cy = (r: Rect): number => r.y + r.h / 2;

/** `Radius::uniform` — true when the four corners are not set separately. */
export const isUniform = (r: { tl?: number; tr?: number; br?: number; bl?: number }): boolean =>
  r.tl === undefined && r.tr === undefined && r.br === undefined && r.bl === undefined;

export type HandleId = (typeof BOX_HANDLES)[number];

export const HANDLE_CURSOR: Record<HandleId, string> = {
  nw: 'nwse-resize',
  n: 'ns-resize',
  ne: 'nesw-resize',
  e: 'ew-resize',
  se: 'nwse-resize',
  s: 'ns-resize',
  sw: 'nesw-resize',
  w: 'ew-resize',
};

export const WEIGHTS = [100, 200, 300, 400, 500, 600, 700, 800, 900];

export const WEIGHT_LABEL: Record<number, string> = {
  100: '极细',
  200: '特细',
  300: '细',
  400: '常规',
  500: '中等',
  600: '半粗',
  700: '粗',
  800: '特粗',
  900: '黑体',
};

export const BLENDS = [
  'normal',
  'multiply',
  'screen',
  'overlay',
  'darken',
  'lighten',
  'color-dodge',
  'color-burn',
  'soft-light',
  'hard-light',
  'difference',
  'exclusion',
  'hue',
  'saturation',
  'color',
  'luminosity',
];

export const OVERFLOWS = [
  { v: 'clip', label: '裁切' },
  { v: 'shrink', label: '缩字' },
  { v: 'scale', label: '缩放' },
  { v: 'overflow', label: '溢出' },
];

export const BOX_MODES = [
  { v: 'fixed', label: '固定' },
  { v: 'auto-height', label: '自动高' },
  { v: 'auto', label: '自动' },
];
