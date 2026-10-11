/** Semantic layout intents <-> editable rows for the 布局 tab.
 *  The row model is deliberately generic: every row can emit a patch for its own
 *  intent key, which the inspector merges and sends to `layout.apply`. */

import type { Intent, Node, Origin } from '../types';

export type IntentKey = keyof Intent | 'mode';

export type RowKind = 'ref' | 'num' | 'enum' | 'bool' | 'size' | 'text';

export interface IRow {
  key: IntentKey;
  /** 中文标签，用作控件名 */
  label: string;
  /** 展示文本，如 "在 headline 下方 · 间距 90" */
  value: string;
  kind: RowKind;
  ref?: string;
  num?: number;
  num2?: number;
  text?: string;
  options?: { v: string; label: string }[];
}

const REL_ZH: Record<string, string> = {
  below: '在…下方',
  above: '在…上方',
  left_of: '在…左侧',
  right_of: '在…右侧',
};

const ANCHORS: Origin[] = [
  'top-left',
  'top',
  'top-right',
  'left',
  'center',
  'right',
  'bottom-left',
  'bottom',
  'bottom-right',
];

const ANCHOR_ZH: Record<string, string> = {
  'top-left': '左上',
  top: '上中',
  'top-right': '右上',
  left: '左中',
  center: '居中',
  right: '右中',
  'bottom-left': '左下',
  bottom: '下中',
  'bottom-right': '右下',
};

const EDGE_ZH: Record<string, string> = {
  left: '左边',
  center: '水平中线',
  right: '右边',
  top: '上边',
  middle: '垂直中线',
  bottom: '下边',
};

export function nameOf(doc: { nodes: Node[] } | null, id: string | undefined): string {
  if (!doc || !id) return id ?? '';
  const n = doc.nodes.find((x) => x.id === id);
  return n?.name || id;
}

export function intentRows(intent: Intent | undefined, nodes: Node[] | null): IRow[] {
  const out: IRow[] = [];
  if (!intent) return out;
  const nm = (id: string | undefined) =>
    (nodes ?? []).find((n) => n.id === id)?.name || id || '';

  for (const key of ['below', 'above', 'left_of', 'right_of'] as const) {
    const r = intent[key];
    if (!r) continue;
    out.push({
      key,
      label: REL_ZH[key],
      kind: 'ref',
      ref: r.of,
      num: r.gap,
      value: `在 ${nm(r.of)} ${REL_ZH[key].replace('在…', '')} · 间距 ${Math.round(r.gap ?? 0)}`,
    });
  }

  if (intent.center_in) {
    out.push({
      key: 'center_in',
      label: '在…内居中',
      kind: 'text',
      text: intent.center_in,
      value: `在 ${nm(intent.center_in)} 内居中`,
    });
  }

  if (intent.anchor) {
    out.push({
      key: 'anchor',
      label: '锚点',
      kind: 'enum',
      text: intent.anchor,
      value: ANCHOR_ZH[intent.anchor] ?? intent.anchor,
      options: ANCHORS.map((a) => ({ v: a, label: ANCHOR_ZH[a] })),
    });
  }

  const sides: [string, string, number | undefined][] = [
    ['margin_top', '上边距', intent.margin_top],
    ['margin_right', '右边距', intent.margin_right],
    ['margin_bottom', '下边距', intent.margin_bottom],
    ['margin_left', '左边距', intent.margin_left],
  ];
  for (const [k, label, v] of sides) {
    if (v === undefined) continue;
    out.push({ key: k as IntentKey, label, kind: 'num', num: v, value: String(Math.round(v)) });
  }
  if (intent.margin !== undefined) {
    out.push({ key: 'margin', label: '四边留白', kind: 'num', num: intent.margin, value: String(Math.round(intent.margin)) });
  }

  if (intent.align_x) {
    out.push({
      key: 'align_x',
      label: '水平对齐',
      kind: 'enum',
      text: intent.align_x,
      value: intent.align_x === 'left' ? '左' : intent.align_x === 'center' ? '中' : '右',
      options: [
        { v: 'left', label: '左' },
        { v: 'center', label: '中' },
        { v: 'right', label: '右' },
      ],
    });
  }
  if (intent.align_y) {
    out.push({
      key: 'align_y',
      label: '垂直对齐',
      kind: 'enum',
      text: intent.align_y,
      value: intent.align_y === 'top' ? '上' : intent.align_y === 'middle' ? '中' : '下',
      options: [
        { v: 'top', label: '上' },
        { v: 'middle', label: '中' },
        { v: 'bottom', label: '下' },
      ],
    });
  }

  if (intent.align_to) {
    out.push({
      key: 'align_to',
      label: '边对齐',
      kind: 'ref',
      ref: intent.align_to.of,
      text: intent.align_to.edge,
      num: intent.align_to.offset ?? 0,
      value: `${EDGE_ZH[intent.align_to.edge] ?? intent.align_to.edge} 对齐 ${nm(intent.align_to.of)} · 偏移 ${Math.round(
        intent.align_to.offset ?? 0,
      )}`,
    });
  }

  const s = intent.size;
  if (s) {
    const parts: string[] = [];
    if (s.mode) parts.push(s.mode === 'fixed' ? '固定' : s.mode === 'fill' ? '填满' : s.mode === 'hug' ? '包裹' : s.mode);
    if (s.w !== undefined) parts.push(`宽 ${Math.round(s.w)}`);
    if (s.h !== undefined) parts.push(`高 ${Math.round(s.h)}`);
    if (s.w_ref !== undefined) parts.push(`宽 ${Math.round(s.w_ref * 100)}%`);
    if (s.h_ref !== undefined) parts.push(`高 ${Math.round(s.h_ref * 100)}%`);
    if (s.ratio !== undefined) parts.push(`比例 ${Math.round(s.ratio * 1000) / 1000}`);
    out.push({
      key: 'size',
      label: '尺寸意图',
      kind: 'size',
      num: s.w,
      num2: s.h,
      text: s.mode ?? 'fixed',
      value: parts.join(' · ') || '未设定',
    });
  }

  if (intent.fit) {
    out.push({
      key: 'fit',
      label: '填充方式',
      kind: 'enum',
      text: intent.fit,
      value: intent.fit,
      options: [
        { v: 'contain', label: '完整显示' },
        { v: 'cover', label: '铺满裁切' },
        { v: 'fill', label: '拉伸' },
        { v: 'actual', label: '原始' },
      ],
    });
  }

  if (intent.safe_margin !== undefined) {
    out.push({
      key: 'safe_margin',
      label: '安全边距',
      kind: 'num',
      num: intent.safe_margin,
      value: String(Math.round(intent.safe_margin)),
    });
  }

  if (intent.distribute?.length) {
    out.push({
      key: 'distribute',
      label: '等距排列',
      kind: 'text',
      text: intent.distribute.join(','),
      value: intent.distribute.map(nm).join(' · '),
    });
  }
  if (intent.equal_gap) {
    out.push({
      key: 'equal_gap',
      label: '等间距',
      kind: 'num',
      num: intent.equal_gap.gap ?? 0,
      text: intent.equal_gap.axis,
      value: `${intent.equal_gap.axis === 'y' ? '纵向' : '横向'} · ${Math.round(intent.equal_gap.gap ?? 0)}`,
    });
  }
  if (intent.keep_aspect_ratio !== undefined) {
    out.push({
      key: 'keep_aspect_ratio',
      label: '锁定比例',
      kind: 'bool',
      num: intent.keep_aspect_ratio ? 1 : 0,
      value: intent.keep_aspect_ratio ? '开' : '关',
    });
  }
  if (intent.flex) {
    const f = intent.flex;
    out.push({
      key: 'flex',
      label: '弹性容器',
      kind: 'enum',
      text: f.direction ?? 'column',
      num: f.gap,
      value: `${f.direction === 'row' ? '横向' : '纵向'} · 间距 ${Math.round(f.gap ?? 0)} · 内边距 ${Math.round(f.padding ?? 0)}`,
      options: [
        { v: 'row', label: '横向' },
        { v: 'column', label: '纵向' },
      ],
    });
  }
  if (intent.grid) {
    const g = intent.grid;
    out.push({
      key: 'grid',
      label: '网格容器',
      kind: 'num',
      num: g.cols,
      num2: g.gap,
      value: `${Math.round(g.cols ?? 0)} 列 · 间距 ${Math.round(g.gap ?? 0)}`,
    });
  }
  if (intent.priority !== undefined) {
    out.push({
      key: 'priority',
      label: '优先级',
      kind: 'num',
      num: intent.priority,
      value: String(Math.round(intent.priority)),
    });
  }
  return out;
}

/** One edited row → the intent fragment to deep-merge (or `null` to delete). */
export function rowPatch(
  row: IRow,
  next: { ref?: string; num?: number; num2?: number; text?: string; on?: boolean },
): { key: string; value: unknown } {
  const { key } = row;
  switch (key) {
    case 'below':
    case 'above':
    case 'left_of':
    case 'right_of':
      return { key, value: { of: next.ref ?? row.ref ?? '', gap: next.num ?? row.num ?? 0, cross: 'center' } };
    case 'center_in':
      return { key, value: next.ref ?? next.text ?? row.text ?? '' };
    case 'anchor':
      return { key, value: (next.text ?? row.text) as Origin };
    case 'align_to':
      return { key, value: { of: next.ref ?? row.ref ?? '', edge: next.text ?? row.text ?? 'left', offset: next.num ?? 0 } };
    case 'size':
      return {
        key,
        value: {
          mode: next.text ?? row.text ?? 'fixed',
          w: next.num ?? row.num,
          h: next.num2 ?? row.num2,
        },
      };
    case 'equal_gap':
      return { key, value: { axis: row.text ?? 'y', gap: next.num ?? row.num ?? 0 } };
    case 'flex':
      return {
        key,
        value: {
          direction: next.text ?? row.text ?? 'column',
          gap: next.num ?? row.num ?? 24,
          padding: 90,
          align: 'center',
        },
      };
    case 'grid':
      return { key, value: { cols: next.num ?? row.num ?? 2, gap: next.num2 ?? row.num2 ?? 24, padding: 90 } };
    case 'keep_aspect_ratio':
      return { key, value: next.on ?? (next.num ?? 0) > 0 };
    case 'priority':
      return { key, value: Math.max(0, Math.min(100, Math.round(next.num ?? row.num ?? 50))) };
    default:
      return { key, value: next.num ?? next.text ?? null };
  }
}

export const LAYOUT_MODES = [
  { v: 'constraint', label: '约束' },
  { v: 'manual', label: '手动' },
  { v: 'flex', label: '弹性' },
  { v: 'grid', label: '网格' },
];

export const ADDABLE_INTENTS: { key: IntentKey; label: string; seed: unknown }[] = [
  { key: 'below', label: '在…下方', seed: { of: '', gap: 48, cross: 'center' } },
  { key: 'above', label: '在…上方', seed: { of: '', gap: 48, cross: 'center' } },
  { key: 'right_of', label: '在…右侧', seed: { of: '', gap: 48, cross: 'center' } },
  { key: 'center_in', label: '在…内居中', seed: '' },
  { key: 'anchor', label: '锚点', seed: 'top-left' },
  { key: 'margin_top', label: '上边距', seed: 90 },
  { key: 'margin_left', label: '左边距', seed: 90 },
  { key: 'align_x', label: '水平对齐', seed: 'center' },
  { key: 'align_to', label: '边对齐', seed: { of: '', edge: 'left', offset: 0 } },
  { key: 'size', label: '尺寸意图', seed: { mode: 'fill', w_ref: 0.84 } },
  { key: 'safe_margin', label: '安全边距', seed: 60 },
  { key: 'keep_aspect_ratio', label: '锁定比例', seed: true },
  { key: 'priority', label: '优先级', seed: 70 },
];

export function rowsToText(rows: IRow[]): string {
  return rows.map((r) => r.value).join('\n');
}
