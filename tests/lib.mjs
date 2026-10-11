// Shared harness for the acceptance suites. Everything here is real: the CLI
// binary is spawned, MCP speaks stdio JSON-RPC, HTTP is a socket, and PNG/JPEG/PDF
// outputs are decoded and hashed. Nothing in these tests is mocked.
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFileSync, existsSync, mkdirSync, rmSync, writeFileSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname, resolve, basename } from 'node:path';
import { fileURLToPath } from 'node:url';
import { deflateSync, inflateSync } from 'node:zlib';

export const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');

/** The suites drive the *build output*. `dist/` is a copy made by scripts/build.mjs,
 *  so testing it here would grade a stale artifact whenever a source lands after the
 *  last package run — the gate's own freshness check (tools/check-dist.mjs) is what
 *  proves the shipped copy is current. */
export const BIN =
  process.env.AGENTCANVAS_BIN ||
  [
    join(ROOT, 'target', 'x86_64-pc-windows-gnu', 'release', 'agentcanvas.exe'),
    join(ROOT, 'target', 'release', 'agentcanvas.exe'),
    join(ROOT, 'dist', 'bin', 'agentcanvas.exe'),
    join(ROOT, 'target', 'debug', 'agentcanvas.exe'),
  ].find((p) => existsSync(p)) ||
  join(ROOT, 'target', 'x86_64-pc-windows-gnu', 'release', 'agentcanvas.exe');

/** Scratch workspace per suite, so parallel runs cannot see each other's projects. */
export function sandbox(name) {
  const dir = join(tmpdir(), `agentcanvas-test-${name}`);
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  return dir;
}

let passed = 0;
let failed = 0;
const failures = [];

export function check(label, cond, detail) {
  if (cond) {
    passed++;
    console.log(`  ok   ${label}`);
  } else {
    failed++;
    failures.push({ label, detail });
    console.log(`  FAIL ${label}${detail ? `  ->  ${typeof detail === 'string' ? detail : JSON.stringify(detail)}` : ''}`);
  }
  return !!cond;
}

export function eq(label, got, want) {
  return check(label, JSON.stringify(got) === JSON.stringify(want), { got, want });
}

export function section(name) {
  console.log(`\n== ${name} ==`);
}

export function summary(suite) {
  console.log(`\n${suite}: ${passed} passed, ${failed} failed`);
  if (failed) {
    for (const f of failures) console.log(`  - ${f.label}: ${typeof f.detail === 'string' ? f.detail : JSON.stringify(f.detail)}`);
    process.exitCode = 1;
    return false;
  }
  return true;
}

export function counts() {
  return { passed, failed };
}

/** Run one CLI method. Returns the parsed JSON (always exactly one value on stdout). */
export function cli(method, args = {}, opts = {}) {
  const kv = [];
  for (const [k, v] of Object.entries(args)) {
    kv.push(`${k}=${typeof v === 'string' ? v : JSON.stringify(v)}`);
  }
  const argv = [method, ...kv];
  if (opts.root) argv.push('--root', opts.root);
  if (opts.pretty) argv.push('--pretty');
  const r = spawnSync(BIN, argv, { encoding: 'buffer', cwd: opts.cwd || ROOT, env: { ...process.env, ...(opts.env || {}) } });
  const out = r.stdout.toString('utf8').trim();
  const err = r.stderr.toString('utf8').trim();
  let json = null;
  try {
    json = JSON.parse(out);
  } catch {
    /* leave null so the caller can assert on the raw text */
  }
  return { json, out, err, code: r.status, raw: out, method };
}

/** The JSON-in form, which is what an agent uses for nested payloads. */
export function rpc(m, p = {}, opts = {}) {
  const payload = JSON.stringify({ m, p });
  const r = spawnSync(BIN, ['rpc', payload, ...(opts.root ? ['--root', opts.root] : [])], {
    encoding: 'buffer',
    cwd: ROOT,
    env: { ...process.env, ...(opts.env || {}) },
    input: '',
  });
  const out = r.stdout.toString('utf8').trim();
  let json = null;
  try {
    json = JSON.parse(out);
  } catch {}
  return { json, out, err: r.stderr.toString('utf8'), code: r.status };
}

// ------------------------------------------------------------------ MCP client

export class McpClient {
  constructor(opts = {}) {
    this.args = opts.args || ['mcp'];
    this.root = opts.root;
    this.buf = '';
    this.pending = new Map();
    this.id = 0;
    this.stderr = '';
  }
  start() {
    const env = { ...process.env, ...(this.root ? { AGENTCANVAS_HOME: this.root } : {}) };
    this.p = spawn(BIN, this.args, { env, cwd: ROOT });
    this.p.stdout.on('data', (d) => this._onData(d));
    this.p.stderr.on('data', (d) => {
      this.stderr += d.toString('utf8');
    });
    return this;
  }
  _onData(d) {
    this.buf += d.toString('utf8');
    let i;
    while ((i = this.buf.indexOf('\n')) >= 0) {
      const line = this.buf.slice(0, i).trim();
      this.buf = this.buf.slice(i + 1);
      if (!line) continue;
      let msg;
      try {
        msg = JSON.parse(line);
      } catch {
        continue;
      }
      if (msg.id !== undefined && this.pending.has(msg.id)) {
        const { res } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        res(msg);
      }
    }
  }
  send(method, params = {}) {
    const id = ++this.id;
    const msg = { jsonrpc: '2.0', id, method, params };
    this.p.stdin.write(JSON.stringify(msg) + '\n');
    return new Promise((res, rej) => {
      this.pending.set(id, { res, rej });
      setTimeout(() => {
        if (this.pending.has(id)) {
          this.pending.delete(id);
          rej(new Error(`timeout waiting for ${method}`));
        }
      }, 120_000);
    });
  }
  notify(method, params = {}) {
    this.p.stdin.write(JSON.stringify({ jsonrpc: '2.0', method, params }) + '\n');
  }
  async init() {
    const r = await this.send('initialize', {
      protocolVersion: '2025-06-18',
      capabilities: {},
      clientInfo: { name: 'agentcanvas-acceptance', version: '1' },
    });
    this.notify('notifications/initialized', {});
    return r.result;
  }
  async tools() {
    const r = await this.send('tools/list', {});
    return r.result.tools;
  }
  async call(name, args = {}) {
    const r = await this.send('tools/call', { name, arguments: args });
    const res = r.result || {};
    const text = (res.content || []).find((c) => c.type === 'text')?.text || '{}';
    let json = null;
    try {
      json = JSON.parse(text);
    } catch {}
    return { json, isError: !!res.isError, res };
  }
  /** The escape hatch: any method through the generic `agentcanvas` tool. */
  async rpc(m, p = {}) {
    return this.call('agentcanvas', { m, p });
  }
  stop() {
    try {
      this.p.stdin.end();
      this.p.kill();
    } catch {}
  }
}

// ------------------------------------------------------------------ HTTP

let httpPort = 0;
export function httpPortOf() {
  return httpPort;
}

export async function startHttp(root, port) {
  // Readiness requires the pid *we* spawned: a server left over from an earlier run
  // would answer the health probe and the whole suite would silently grade that one.
  httpPort = port || 20000 + (process.pid % 20000);
  const p = spawn(BIN, ['http', '--port', String(httpPort), '--root', root], { cwd: ROOT, env: process.env });
  const t0 = Date.now();
  while (Date.now() - t0 < 20_000) {
    try {
      const r = await req('GET', '/health');
      if (r.json?.ok && r.json?.pid === p.pid) return { port: httpPort, proc: p };
    } catch {}
    await new Promise((r) => setTimeout(r, 250));
  }
  throw new Error(`http server (pid ${p.pid}) did not come up on ${httpPort}`);
}

export async function req(method, path, body, headers = {}) {
  const { request } = await import('node:http');
  return await new Promise((res, rej) => {
    const data = body === undefined ? null : Buffer.from(typeof body === 'string' ? body : JSON.stringify(body));
    const r = request(
      { host: '127.0.0.1', port: httpPort, method, path, headers: { ...(data ? { 'Content-Length': data.length, 'Content-Type': 'application/json' } : {}), ...headers } },
      (resp) => {
        const chunks = [];
        resp.on('data', (c) => chunks.push(c));
        resp.on('end', () => {
          const buf = Buffer.concat(chunks);
          let json = null;
          try {
            json = JSON.parse(buf.toString('utf8'));
          } catch {}
          res({ status: resp.statusCode, headers: resp.headers, buf, json });
        });
      },
    );
    r.on('error', rej);
    if (data) r.write(data);
    r.end();
  });
}

export async function rpcHttp(m, p = {}) {
  const r = await req('POST', '/rpc', { m, p });
  return r.json;
}

// ------------------------------------------------------------------ image facts

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

/** Minimal PNG writer so fixtures need no external tool. */
export function writePng(path, w, h, painter) {
  const raw = Buffer.alloc((w * 4 + 1) * h);
  for (let y = 0; y < h; y++) {
    raw[y * (w * 4 + 1)] = 0;
    for (let x = 0; x < w; x++) {
      const [r, g, b, a] = painter(x, y, w, h);
      const i = y * (w * 4 + 1) + 1 + x * 4;
      raw[i] = r;
      raw[i + 1] = g;
      raw[i + 2] = b;
      raw[i + 3] = a === undefined ? 255 : a;
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  const png = Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 6 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, png);
  return { path, bytes: png.length };
}

/** Read the real dimensions out of whatever the engine wrote. */
export function imageDims(path) {
  const b = readFileSync(path);
  if (b.length < 24) return null;
  if (b[0] === 0x89 && b[1] === 0x50) {
    return { fmt: 'png', w: b.readUInt32BE(16), h: b.readUInt32BE(20) };
  }
  if (b[0] === 0xff && b[1] === 0xd8) {
    let i = 2;
    while (i < b.length - 9) {
      if (b[i] !== 0xff) {
        i++;
        continue;
      }
      const marker = b[i + 1];
      const len = b.readUInt16BE(i + 2);
      if (marker >= 0xc0 && marker <= 0xcf && ![0xc4, 0xc8, 0xcc].includes(marker)) {
        return { fmt: 'jpeg', h: b.readUInt16BE(i + 5), w: b.readUInt16BE(i + 7) };
      }
      i += 2 + len;
    }
    return { fmt: 'jpeg' };
  }
  if (b[0] === 0x52 && b[1] === 0x49 && b[2] === 0x46 && b[3] === 0x46) {
    const fourcc = b.slice(12, 16).toString('ascii');
    if (fourcc === 'VP8X') {
      return { fmt: 'webp', w: (b[24] | (b[25] << 8) | (b[26] << 16)) + 1, h: (b[27] | (b[28] << 8) | (b[29] << 16)) + 1 };
    }
    if (fourcc === 'VP8L') {
      // lossless: 0x2f then 14-bit (w-1) and 14-bit (h-1) packed little-endian
      const bits = b.readUInt32LE(21);
      return { fmt: 'webp', w: (bits & 0x3fff) + 1, h: ((bits >> 14) & 0x3fff) + 1 };
    }
    if (fourcc.startsWith('VP8 ')) {
      // lossy: sync code 0x9d 0x01 0x2a then 16-bit (w-1)/(h-1) at 26/28
      return { fmt: 'webp', w: (b[26] | (b[27] << 8)) & 0x3fff, h: (b[28] | (b[29] << 8)) & 0x3fff };
    }
    return { fmt: 'webp' };
  }
  if (b.slice(0, 5).toString() === '%PDF-') return { fmt: 'pdf' };
  if (b.slice(0, 5).toString() === '<?xml' || b.slice(0, 4).toString() === '<svg') return { fmt: 'svg' };
  return { fmt: 'unknown' };
}

export const sha256 = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');

export function nonEmpty(path) {
  return existsSync(path) && statSync(path).size > 0;
}

/** Count distinct colours in a PNG's IDAT by decoding it with zlib. */
export function pngHasContent(path) {
  const b = readFileSync(path);
  if (b.length < 8 || b[0] !== 0x89) return false;
  let i = 8;
  let idat = [];
  let w = 0;
  let h = 0;
  let bpp = 4;
  while (i < b.length) {
    const len = b.readUInt32BE(i);
    const type = b.slice(i + 4, i + 8).toString('ascii');
    const data = b.slice(i + 8, i + 8 + len);
    if (type === 'IHDR') {
      w = data.readUInt32BE(0);
      h = data.readUInt32BE(4);
      bpp = [2, 3, 4, 6][data[9] - 0] ? [1, 2, 3, 4][data[9] - 2] || 4 : 4;
    }
    if (type === 'IDAT') idat.push(data);
    if (type === 'IEND') break;
    i += 12 + len;
  }
  if (!w || !h) return false;
  const raw = inflate(Buffer.concat(idat));
  const seen = new Set();
  const stride = w * bpp;
  let prev = Buffer.alloc(stride);
  for (let y = 0; y < h; y++) {
    const line = Buffer.alloc(stride);
    const filter = raw[y * (stride + 1)];
    const row = raw.slice(y * (stride + 1) + 1, y * (stride + 1) + 1 + stride);
    for (let x = 0; x < stride; x++) {
      const a = x >= bpp ? line[x - bpp] : 0;
      const b2 = prev[x];
      const c = x >= bpp ? prev[x - bpp] : 0;
      let v = row[x];
      switch (filter) {
        case 1: v += a; break;
        case 2: v += b2; break;
        case 3: v += (a + b2) >> 1; break;
        case 4: {
          const p = a + b2 - c;
          const pa = Math.abs(p - a), pb = Math.abs(p - b2), pc = Math.abs(p - c);
          v += pa <= pb && pa <= pc ? a : pb <= pc ? b2 : c;
          break;
        }
        default: break;
      }
      line[x] = v & 0xff;
    }
    prev = line;
    for (let x = 0; x < w; x++) {
      const o = x * bpp;
      seen.add(`${line[o]},${line[o + 1]},${line[o + 2]},${bpp === 4 ? line[o + 3] : 255}`);
      if (seen.size > 40) return true;
    }
  }
  return seen.size > 1;
}

function inflate(buf) {
  return inflateSync(buf);
}

export function touch(path) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, '');
}

export const base = basename;

/** Engine paths are root-relative; accept an absolute one too. */
export function abs(root, p) {
  if (!p) return '';
  if (/^[a-zA-Z]:[\/]/.test(p) || p.startsWith('/')) return p;
  return join(root, p);
}
