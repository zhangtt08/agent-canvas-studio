/** Display helpers. Numbers are shown exactly as the engine stores them:
 *  pixels are integers-ish, opacity is 0..1, rotation is degrees. */

export const clamp = (v: number, lo: number, hi: number): number =>
  Math.min(hi, Math.max(lo, v));

export const round = (v: number, dp = 1): number => {
  const f = 10 ** dp;
  return Math.round((v + Number.EPSILON) * f) / f;
};

/** 1 → "1", 1.05 → "1.05", 123.456 → "123.5" */
export function num(v: number | undefined, dp = 1): string {
  if (v === undefined || v === null || !Number.isFinite(v)) return '—';
  const r = round(v, dp);
  return Number.isInteger(r) ? String(r) : String(r);
}

export function fmtBytes(b: number | undefined): string {
  const n = b ?? 0;
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${round(n / 1024, n < 10240 ? 1 : 0)} KB`;
  return `${round(n / 1048576, 2)} MB`;
}

export const shortSha = (s: string | undefined): string => (s ? s.slice(0, 8) : '—');

export function ms(n: number): string {
  if (n < 1000) return `${Math.round(n)}ms`;
  return `${round(n / 1000, 2)}s`;
}

export function fmtTime(iso: string | undefined): string {
  if (!iso) return '—';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const p = (x: number) => String(x).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** The engine gives timestamps in two shapes: `log`/`project.list` send
 *  `"YYYY-MM-DD HH:MM"` strings, `history.list` sends epoch milliseconds. */
export function timeAgo(iso: string | number | undefined): string {
  if (iso === undefined || iso === null || iso === '') return '';
  const t = typeof iso === 'number' ? iso : new Date(iso.replace(' ', 'T')).getTime();
  if (!Number.isFinite(t)) return typeof iso === 'string' ? iso : '';
  const s = Math.max(0, (Date.now() - t) / 1000);
  if (s < 60) return '刚刚';
  if (s < 3600) return `${Math.floor(s / 60)} 分钟前`;
  if (s < 86400) return `${Math.floor(s / 3600)} 小时前`;
  if (s < 86400 * 30) return `${Math.floor(s / 86400)} 天前`;
  return typeof iso === 'number' ? new Date(iso).toISOString().slice(0, 10) : fmtTime(iso).slice(0, 10);
}

export function gcd(a: number, b: number): number {
  let x = Math.round(Math.abs(a));
  let y = Math.round(Math.abs(b));
  while (y) {
    const t = y;
    y = x % y;
    x = t;
  }
  return x || 1;
}

export function ratioLabel(w: number, h: number): string {
  const g = gcd(w, h);
  const a = round(w / g, 2);
  const b = round(h / g, 2);
  return `${a}:${b}`;
}

export const baseName = (p: string): string => {
  const clean = String(p || '').replace(/[\\/]+$/, '');
  const parts = clean.split(/[\\/]/);
  return parts[parts.length - 1] || clean;
};

export const stem = (p: string): string => {
  const n = baseName(p);
  const i = n.lastIndexOf('.');
  return i > 0 ? n.slice(0, i) : n;
};

export const extOf = (p: string): string => {
  const m = /\.([a-z0-9]+)$/i.exec(String(p || ''));
  return m ? m[1].toLowerCase() : '';
};

export const dirOf = (p: string): string => {
  const i = Math.max(p.lastIndexOf('/'), p.lastIndexOf('\\'));
  return i > 0 ? p.slice(0, i) : '.';
};

export const joinPath = (dir: string, name: string): string => {
  if (!dir) return name;
  const sep = dir.includes('\\') && !dir.includes('/') ? '\\' : '/';
  return dir.endsWith(sep) || dir.endsWith('/') || dir.endsWith('\\')
    ? `${dir}${name}`
    : `${dir}${sep}${name}`;
};

/** `#rgb` → `#rrggbb`; anything unparseable comes back as `#000000`. */
export function normHex(c: string | undefined): string {
  const t = (c || '').trim();
  if (/^#[0-9a-f]{6}$/i.test(t)) return t.toLowerCase();
  const m = /^#?([0-9a-f])([0-9a-f])([0-9a-f])$/i.exec(t);
  if (m) return `#${m[1]}${m[1]}${m[2]}${m[2]}${m[3]}${m[3]}`.toLowerCase();
  const m8 = /^#([0-9a-f]{6})[0-9a-f]{2}$/i.exec(t);
  if (m8) return `#${m8[1]}`.toLowerCase();
  return '#000000';
}

/** `#rrggbbaa` or `#rrggbb` + alpha → css rgba() */
export function cssColor(c: string | undefined, fallback = 'transparent'): string {
  const t = (c || '').trim();
  if (!t) return fallback;
  if (/^#[0-9a-f]{3,8}$/i.test(t)) {
    const hex = t.slice(1);
    const full =
      hex.length <= 4
        ? hex
            .split('')
            .map((x) => x + x)
            .join('')
        : hex;
    const r = parseInt(full.slice(0, 2), 16);
    const g = parseInt(full.slice(2, 4), 16);
    const b = parseInt(full.slice(4, 6), 16);
    const a = full.length === 8 ? parseInt(full.slice(6, 8), 16) / 255 : 1;
    return `rgba(${r},${g},${b},${round(a, 3)})`;
  }
  return t;
}

export const hexAlpha = (c: string): number => {
  const m = /^#([0-9a-f])([0-9a-f])([0-9a-f])([0-9a-f])$/i.exec(c);
  if (m) return parseInt(m[4] + m[4], 16) / 255;
  const m8 = /^#[0-9a-f]{6}([0-9a-f]{2})$/i.exec(c);
  return m8 ? parseInt(m8[1], 16) / 255 : 1;
};

/** Luminance-based readable text colour for a swatch. */
export function inkOn(c: string): string {
  const t = cssColor(c, 'rgba(0,0,0,1)').match(/[\d.]+/g);
  if (!t) return '#171d3f';
  const [r, g, b] = [Number(t[0]), Number(t[1]), Number(t[2])];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b > 150 ? '#171d3f' : '#ffffff';
}

export const pct = (v: number): string => `${Math.round(v * 100)}%`;

export const uid = (p: string): string =>
  `${p}_${Date.now().toString(36).slice(-5)}${Math.random().toString(36).slice(2, 6)}`;
