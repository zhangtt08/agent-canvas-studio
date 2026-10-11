// Generates app/icons/* from scratch: no network, no image library, no binary
// committed to the repo. The mark is the product's own material language — a
// rounded gradient slab with a glass panel over it — and the PNG/ICO/bytes are
// written with hand-rolled chunks so a fresh clone can rebuild them.
import { deflateSync } from 'node:zlib';
import { writeFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const outDir = join(here, '..', 'app', 'icons');
mkdirSync(outDir, { recursive: true });

const CRC_TABLE = (() => {
  const t = new Int32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c;
  }
  return t;
})();
const crc32 = (buf) => {
  let c = ~0;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return ~c >>> 0;
};
const chunk = (type, data) => {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
};
const png = (w, h, rgba) => {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // truecolour + alpha
  const raw = Buffer.alloc((w * 4 + 1) * h);
  for (let y = 0; y < h; y++) {
    raw[y * (w * 4 + 1)] = 0; // filter: none
    rgba.copy(raw, y * (w * 4 + 1) + 1, y * w * 4, (y + 1) * w * 4);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
};

const lerp = (a, b, t) => a + (b - a) * t;
const mix = (c1, c2, t) => [lerp(c1[0], c2[0], t), lerp(c1[1], c2[1], t), lerp(c1[2], c2[2], t)];
const BLUE = [59, 91, 251];
const VIOLET = [150, 78, 235];
const CORAL = [255, 122, 89];

/** signed distance helpers, all in unit space (-1..1) */
const roundedBox = (x, y, hw, hh, r) => {
  const dx = Math.abs(x) - (hw - r);
  const dy = Math.abs(y) - (hh - r);
  const ax = Math.max(dx, 0);
  const ay = Math.max(dy, 0);
  return Math.hypot(ax, ay) + Math.min(Math.max(dx, dy), 0) - r;
};
const seg = (x, y, x1, y1, x2, y2, x3, y3) => {
  const [px, py] = [x - x1, y - y1];
  const [bx, by] = [x2 - x1, y2 - y1];
  const [cx, cy] = [x3 - x1, y3 - y1];
  const d = bx * cx + by * cy;
  const e = cx * cx + cy * cy;
  const t = Math.max(0, Math.min(1, d / (e || 1)));
  return Math.hypot(px - cx * t, py - cy * t);
};

function draw(size) {
  const buf = Buffer.alloc(size * size * 4);
  const R = 0.30; // slab corner radius
  for (let py = 0; py < size; py++) {
    for (let px = 0; px < size; px++) {
      const x = ((px + 0.5) / size) * 2 - 1;
      const y = ((py + 0.5) / size) * 2 - 1;
      const i = (py * size + px) * 4;
      const aa = (d) => Math.max(0, Math.min(1, (d - 0.004) / -0.008 + 0.5));

      // --- gradient slab
      const inSlab = aa(roundedBox(x, y, 0.94, 0.94, R));
      if (inSlab <= 0) {
        buf[i + 3] = 0;
        continue;
      }
      const tg = Math.max(0, Math.min(1, (x + y + 1.6) / 3.2));
      let [r, g, b] = mix(BLUE, VIOLET, tg);
      // coral bloom bottom-right
      const bloom = Math.max(0, 1 - Math.hypot(x - 0.62, y - 0.66) / 0.85) ** 2;
      [r, g, b] = mix([r, g, b], CORAL, bloom * 0.75);
      // top-left sheen
      const sheen = Math.max(0, 1 - Math.hypot(x + 0.55, y + 0.62) / 1.15) ** 2;
      [r, g, b] = mix([r, g, b], [255, 255, 255], sheen * 0.28);
      let a = 255 * inSlab;

      // --- glass panel, slightly rotated
      const ang = -0.20;
      const gx = x * Math.cos(ang) + y * Math.sin(ang) - 0.1;
      const gy = -x * Math.sin(ang) + y * Math.cos(ang) + 0.06;
      const gd = roundedBox(gx, gy, 0.52, 0.40, 0.14);
      const inGlass = aa(gd);
      if (inGlass > 0) {
        const gr = 255, gg = 255, gb = 255;
        const fill = 0.30;
        let [nr, ng, nb] = [lerp(r, gr, fill), lerp(g, gg, fill), lerp(b, gb, fill)];
        // rim light on the panel edge
        const rim = Math.max(0, 1 - Math.abs(gd) / 0.02);
        [nr, ng, nb] = mix([nr, ng, nb], [255, 255, 255], rim * 0.75);
        const edge = Math.max(0, 1 - Math.abs(gd - 0.012) / 0.012);
        [nr, ng, nb] = mix([nr, ng, nb], [255, 255, 255], edge * 0.35);
        r = lerp(r, nr, inGlass);
        g = lerp(g, ng, inGlass);
        b = lerp(b, nb, inGlass);
      }

      // --- "A" mark inside the panel: two strokes + crossbar
      const s = 0.115;
      const stroke = (d) => aa((s * 0.42 - d) / 0.012);
      const l1 = seg(gx, gy, -0.24, 0.22, 0.0, -0.26);
      const l2 = seg(gx, gy, 0.24, 0.22, 0.0, -0.26);
      const l3 = seg(gx, gy, -0.11, 0.06, 0.11, 0.06);
      const mark = Math.max(stroke(l1), Math.max(stroke(l2), stroke(l3)));
      if (mark > 0) {
        [r, g, b] = mix([r, g, b], [255, 255, 255], mark * 0.96);
      }

      // --- outer rim of the slab
      const outer = Math.max(0, 1 - Math.abs(roundedBox(x, y, 0.94, 0.94, R)) / 0.02);
      [r, g, b] = mix([r, g, b], [255, 255, 255], outer * 0.35 * inSlab);

      buf[i] = Math.round(r);
      buf[i + 1] = Math.round(g);
      buf[i + 2] = Math.round(b);
      buf[i + 3] = Math.round(a);
    }
  }
  return buf;
}

const down = (src, sw, sh, dw, dh) => {
  const out = Buffer.alloc(dw * dh * 4);
  for (let y = 0; y < dh; y++) {
    for (let x = 0; x < dw; x++) {
      // box filter: icons get downscaled, nearest leaves jaggies on the rim
      let r = 0, g = 0, b = 0, a = 0, n = 0;
      const x0 = Math.floor((x * sw) / dw), x1 = Math.max(x0 + 1, Math.floor(((x + 1) * sw) / dw));
      const y0 = Math.floor((y * sh) / dw), y1 = Math.max(y0 + 1, Math.floor(((y + 1) * sh) / dw));
      for (let yy = y0; yy < y1; yy++) {
        for (let xx = x0; xx < x1; xx++) {
          const i = (yy * sw + xx) * 4;
          r += src[i]; g += src[i + 1]; b += src[i + 2]; a += src[i + 3];
          n++;
        }
      }
      const j = (y * dw + x) * 4;
      out[j] = Math.round(r / n);
      out[j + 1] = Math.round(g / n);
      out[j + 2] = Math.round(b / n);
      out[j + 3] = Math.round(a / n);
    }
  }
  return out;
};

const N = 512;
const big = draw(N);
const sizes = [32, 64, 128, 256];
const files = { 'icon.png': png(N, N, big) };
for (const s of sizes) {
  const px = down(big, N, N, s, s);
  const name = s === 128 ? '128x128.png' : s === 32 ? '32x32.png' : s === 64 ? '64x64.png' : '256x256.png';
  files[name] = png(s, s, px);
}
files['128x128@2x.png'] = files['256x256.png'];

// ICO: one entry per PNG payload (Vista+ accepts PNG inside ICO).
const entries = [256, 128, 64, 32].map((s) => ({ s, data: files[s === 256 ? '256x256.png' : s === 128 ? '128x128.png' : s === 64 ? '64x64.png' : '32x32.png'] }));
const head = Buffer.alloc(6);
head.writeUInt16LE(0, 0);
head.writeUInt16LE(1, 2); // type: icon
head.writeUInt16LE(entries.length, 4);
let off = 6 + 16 * entries.length;
const dir = Buffer.alloc(16 * entries.length);
entries.forEach((e, k) => {
  const i = k * 16;
  dir[i] = e.s === 256 ? 0 : e.s; // 0 means 256
  dir[i + 1] = e.s === 256 ? 0 : e.s;
  dir.writeUInt16LE(1, i + 4); // planes
  dir.writeUInt16LE(32, i + 6); // bpp
  dir.writeUInt32LE(e.data.length, i + 8);
  dir.writeUInt32LE(off, i + 12);
  off += e.data.length;
});
files['icon.ico'] = Buffer.concat([head, dir, ...entries.map((e) => e.data)]);

for (const [name, data] of Object.entries(files)) {
  writeFileSync(join(outDir, name), data);
  console.log(`${name.padEnd(18)} ${data.length} bytes`);
}
console.log(`-> ${outDir}`);
