/** Image-op argument schema: it turns `caps.image_ops` strings into real controls
 *  and renders the chain back into the compact string form `image.chain` accepts. */

import type { V } from '../types';
import type { IconName } from '../components/Icons';

export interface Arg {
  k: string;
  label: string;
  kind: 'num' | 'text' | 'color' | 'select' | 'ratio';
  min?: number;
  max?: number;
  step?: number;
  dp?: number;
  options?: { v: string; label: string }[];
  def: string | number;
}

export const OP_LABEL: Record<string, string> = {
  crop: '裁切',
  resize: '指定尺寸',
  scale: '等比缩放',
  pad: '补白',
  rotate: '旋转',
  flip: '翻转',
  round: '圆角',
  border: '描边',
  adjust: '影调',
  blur: '模糊',
  sharpen: '锐化',
  noise: '颗粒',
  gray: '去色',
  invert: '反相',
  threshold: '二值化',
  tint: '染色',
  watermark: '文字水印',
  background: '铺底',
  remove_bg: '抠底',
  strip: '拼长条',
};

const N = (k: string, label: string, def: number, min = 0, max = 4096, step = 1, dp = 0): Arg => ({ k, label, kind: 'num', def, min, max, step, dp });
const T = (k: string, label: string, def: string): Arg => ({ k, label, kind: 'text', def });
const C = (k: string, label: string, def: string): Arg => ({ k, label, kind: 'color', def });
const S = (k: string, label: string, def: string, options: { v: string; label: string }[]): Arg => ({ k, label, kind: 'select', def, options });

export const OP_ARGS: Record<string, Arg[]> = {
  crop: [S('ratio', '比例', '3:4', [{ v: '1:1', label: '1:1' }, { v: '3:4', label: '3:4' }, { v: '4:5', label: '4:5' }, { v: '16:9', label: '16:9' }, { v: '9:16', label: '9:16' }]), N('w', '宽', 1080), N('h', '高', 1440)],
  resize: [N('w', '宽', 1080), N('h', '高', 1350)],
  scale: [N('by', '倍率', 0.8, 0.05, 4, 0.05, 2)],
  pad: [N('v', '四周', 64), C('color', '补色', '#ffffff')],
  rotate: [N('deg', '角度', 90, -360, 360, 15)],
  flip: [S('axis', '轴向', 'x', [{ v: 'x', label: '水平' }, { v: 'y', label: '垂直' }])],
  round: [N('r', '半径', 48)],
  border: [N('w', '粗细', 12), C('color', '颜色', '#171d3f')],
  adjust: [N('brightness', '亮度', 1.04, 0, 2, 0.02, 2), N('contrast', '对比', 1.06, 0, 3, 0.02, 2), N('saturate', '饱和', 1.1, 0, 3, 0.02, 2)],
  blur: [N('sigma', '半径', 6, 0, 60, 0.5, 1)],
  sharpen: [N('amount', '强度', 0.7, 0, 3, 0.05, 2)],
  noise: [N('v', '强度', 0.12, 0, 1, 0.01, 2)],
  gray: [],
  invert: [],
  threshold: [N('v', '阈值', 0.5, 0, 1, 0.02, 2)],
  tint: [C('color', '颜色', '#4b6bfb'), N('opacity', '浓度', 0.28, 0, 1, 0.02, 2)],
  watermark: [T('text', '文字', 'AGENTCANVAS'), N('size', '字号', 42), C('color', '颜色', '#ffffff'), N('opacity', '不透明', 0.6, 0, 1, 0.05, 2), S('pos', '位置', 'bottom-right', [{ v: 'top-left', label: '左上' }, { v: 'top-right', label: '右上' }, { v: 'bottom-left', label: '左下' }, { v: 'bottom-right', label: '右下' }])],
  background: [C('color', '底色', '#f6f7fb')],
  remove_bg: [N('v', '容差', 24, 0, 120)],
  strip: [N('n', '段数', 3, 2, 8), N('gap', '间距', 24)],
};

export function argsOf(op: string): Arg[] {
  return OP_ARGS[op] ?? [];
}

export function defaultsOf(op: string): V {
  const out: V = {};
  for (const a of argsOf(op)) out[a.k] = a.def;
  return out;
}

/** `crop ratio=3:4 > scale by=0.8 > watermark 品牌` — the human-readable chain. */
export function chainString(chain: { op: string; args: V }[]): string {
  return chain
    .map((s) => {
      const args = argsOf(s.op).filter((a) => s.args[a.k] !== undefined);
      if (!args.length) return s.op;
      const parts = args.map((a) => {
        const v = s.args[a.k];
        if (a.kind === 'text') return String(v);
        if (a.kind === 'color') return String(v);
        return `${a.k}=${typeof v === 'number' ? +v.toFixed(3) : v}`;
      });
      return `${s.op} ${parts.join(' ')}`;
    })
    .join(' > ');
}

export const OP_ICON: Record<string, IconName> = {
  crop: 'crop',
  resize: 'fit',
  scale: 'zoomIn',
  pad: 'boards',
  rotate: 'refresh',
  flip: 'swap',
  round: 'circle',
  border: 'square',
  adjust: 'sliders',
  blur: 'sliders',
  sharpen: 'bolt',
  noise: 'sparkle',
  gray: 'style',
  invert: 'swap',
  threshold: 'sliders',
  tint: 'style',
  watermark: 'text',
  background: 'square',
  remove_bg: 'wand',
  strip: 'layers',
};
