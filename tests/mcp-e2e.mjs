// Scenario B — a pure background agent. No GUI, no HTTP, only the MCP stdio
// server, building a real poster and exporting real files.
import { join } from 'node:path';
import { openSync, readSync, closeSync } from 'node:fs';
import { McpClient, BIN, sandbox, check, eq, section, summary, imageDims, sha256, nonEmpty, writePng, pngHasContent } from './lib.mjs';

const root = sandbox('mcp');
const FIX = join(root, 'in');
writePng(join(FIX, 'product.png'), 600, 800, (x, y, w, h) => {
  const u = x / w;
  const v = y / h;
  const disc = Math.hypot(u - 0.5, v - 0.42) < 0.3 ? 1 : 0;
  return disc ? [40 + 180 * u, 60 + 120 * v, 220 - 120 * u, 255] : [250, 248, 244, 255];
});

section('MCP e2e — headless, no GUI');
console.log(`  binary: ${BIN}`);
if (!nonEmpty(BIN)) {
  console.log('FATAL: binary missing — build it first (scripts/build.mjs)');
  process.exit(2);
}

const c = new McpClient({ root });
c.start();
const info = await c.init();
check('initialize returns serverInfo.name=agentcanvas', info?.serverInfo?.name === 'agentcanvas', info);
check('instructions are provided to the client', typeof info?.instructions === 'string' && info.instructions.length > 80);

const tools = await c.tools();
check('tools/list is aggregated, not hundreds of tiny methods', Array.isArray(tools) && tools.length >= 25 && tools.length <= 80, tools?.length);
check('every tool has an inputSchema', (tools || []).every((t) => t.inputSchema?.type === 'object'), (tools || []).filter((t) => !t.inputSchema).map((t) => t.name));
check('the poster tool exists', (tools || []).some((t) => t.name === 'poster'));
check('no tool leaks a method field into the schema', (tools || []).every((t) => !('method' in t)));

const caps = await c.rpc('system.caps');
check('caps answers over MCP', caps.json?.name === 'AgentCanvas', caps.json?.e);
check('caps lists image ops', Array.isArray(caps.json?.image_ops) && caps.json.image_ops.length > 15);
check('caps lists layout intents', (caps.json?.intents || []).includes('below') && (caps.json?.intents || []).includes('center_in'));
const capsBytes = JSON.stringify(caps.json).length;
check(`caps stays small enough to read first (${(capsBytes / 1024).toFixed(1)} KB)`, capsBytes < 12_000, capsBytes);

const created = await c.rpc('project.create', { name: 'mcp-campaign', w: 1080, h: 1440 });
const P = created.json?.p;
check('project.create returns an id', !!P, created.json);
const B = created.json?.boards?.[0]?.b;
check('project.create returns its first board', !!B, created.json);

const imp = await c.rpc('asset.import', { p: P, path: join(FIX, 'product.png') });
const A = imp.json?.assets?.[0]?.id;
check('asset.import returns an asset id', !!A, imp.json);
eq('imported asset carries real dimensions', [imp.json?.assets?.[0]?.w, imp.json?.assets?.[0]?.h], [600, 800]);

const title = await c.rpc('canvas.add', {
  p: P, b: B, t: 'text', text: '秋季新品', size: 96, color: '#1C1C1C', name: 'headline',
  intent: { anchor: 'top', margin: 120, align_x: 'center' },
});
const TID = title.json?.created?.[0] || title.json?.id;
check('canvas.add text returns the new node id', !!TID, title.json);

const img = await c.rpc('canvas.add', {
  p: P, b: B, t: 'image', asset: A, name: 'product', w: 700,
  intent: { below: TID, gap: 80, align_x: 'center', keep_aspect_ratio: true },
});
const IID = img.json?.created?.[0] || img.json?.id;
check('canvas.add image placed below the title', !!IID, img.json);

const solved = await c.rpc('layout.resolve', { p: P, b: B });
check('layout.resolve reports no conflicts', (solved.json?.conflicts || []).length === 0, solved.json?.conflicts);
const doc = await c.rpc('project.get', { p: P });
const nodeOf = (id) => (doc.json?.nodes || []).find((n) => n.id === id);
const tn = nodeOf(TID);
const inn = nodeOf(IID);
check('the solved title sits at the requested top margin', Math.abs(tn?.box?.y - 120) < 1.5, tn?.box);
check('the product is below the title', inn?.box.y >= tn?.box.y + tn?.box.h - 1, { img: inn?.box, txt: tn?.box });
check('the product is horizontally centred', Math.abs(inn?.box.x + inn?.box.w / 2 - 540) < 2, inn?.box);
check('aspect ratio was preserved', Math.abs(inn?.box.w / inn?.box.h - 600 / 800) < 0.02, inn?.box);

const explained = await c.rpc('layout.explain', { p: P, id: IID });
check('layout.explain names the intents that placed the node', JSON.stringify(explained.json).includes('below'), explained.json);

const val = await c.rpc('inspect.validate', { p: P });
check('inspect.validate returns a machine-readable report', Array.isArray(val.json?.issues), val.json);
check('validate is ok or tells us why', val.json?.ok === true || (val.json?.issues || []).every((i) => i.s && i.c), val.json?.issues?.slice(0, 3));

const prev = await c.rpc('render.preview', { p: P, sc: 0.5 });
const prevPath = join(root, prev.json?.path || '');
check('render.preview writes a real file', nonEmpty(prevPath), prev.json);
eq('preview dimensions follow the scale', [imageDims(prevPath)?.w, imageDims(prevPath)?.h], [540, 720]);
check('preview is not a blank rectangle', pngHasContent(prevPath), prevPath);

const before = prevPath;
await c.rpc('canvas.text', { p: P, id: TID, content: '冬季新品上市' });
const after = await c.rpc('render.preview', { p: P, sc: 0.5 });
const cmp = await c.rpc('image.compare', { a: prev.json.path, b2: after.json.path });
check('changing the copy actually changes pixels', (cmp.json?.score ?? 100) < 100, cmp.json);
check('and the diff localises the change', (cmp.json?.box?.w ?? 0) > 0, cmp.json);
const same = await c.rpc('image.compare', { a: after.json.path, b2: after.json.path });
eq('identical images score exactly 100', same.json?.score, 100);

const exp = await c.rpc('render.export', { p: P, f: 'png', sc: 1, o: 'out' });
const files = exp.json?.files || [];
check('render.export returns at least one file record', files.length >= 1, exp.json);
const f0 = files[0];
const abs = join(root, f0?.path || '');
check('exported PNG exists on disk', nonEmpty(abs), f0);
eq('exported PNG is full scale', [imageDims(abs)?.w, imageDims(abs)?.h], [1080, 1440]);
eq('the reported sha256 matches the bytes on disk', sha256(abs).slice(0, 12), (f0?.sha || '').slice(0, 12));

for (const [fmt, dims] of [['jpg', [1080, 1440]], ['webp', [1080, 1440]], ['pdf', null], ['svg', null]]) {
  const r = await c.rpc('render.export', { p: P, f: fmt, sc: 1, o: `out-${fmt}` });
  const rec = (r.json?.files || [])[0];
  const p = join(root, rec?.path || '');
  check(`export ${fmt} writes a real file`, nonEmpty(p), r.json);
  if (dims) eq(`export ${fmt} keeps the board size`, [imageDims(p)?.w, imageDims(p)?.h], dims);
  if (fmt === 'pdf') check('pdf has a real header', readHead(p, 5) === '%PDF-', p);
  if (fmt === 'svg') check('svg output parses as svg', readHead(p, 5).includes('<svg') || readHead(p, 6).includes('<?xml'), p);
}

const hist = await c.rpc('history.list', { p: P });
// Only command-bearing calls enter the undo trail (adds and the text edit);
// reads, imports and solves are in the call log instead. Both must be non-empty.
check('the command trail recorded every structural edit', (hist.json?.length || 0) >= 3, hist.json);
const trail = JSON.stringify(hist.json);
check('trail records mcp as the origin', trail.includes('"origin":"mcp"'), hist.json?.[0]);
const logs = await c.rpc('log', { n: 50 });
check('the call log covers the whole session, not just commands', (logs.json?.length || logs.json?.r?.length || 0) > 8, logs.json?.length);
check('the log names the door each call came from', JSON.stringify(logs.json).includes('mcp'), logs.json?.[0]);

const locked = await c.rpc('canvas.update', { p: P, id: IID, s: { locked: true } });
check('a node can be locked', locked.json?.ok !== false, locked.json);
const moved = await c.rpc('canvas.transform', { p: P, id: IID, x: 10, y: 10, w: 100, h: 100 });
check('moving a locked node is refused, not silently applied', moved.json?.code === 'LOCKED', moved.json);

const stale = await c.rpc('canvas.text', { p: P, id: TID, content: 'stale write', rev: 1 });
check('a stale revision is rejected', stale.json?.code === 'REVISION_CONFLICT', stale.json);
check('the rejection carries the current revision so the caller can retry', typeof stale.json?.details?.rev === 'number', stale.json?.details);

const bad = await c.rpc('canvas.add', { p: P, b: B, t: 'image', asset: 'does_not_exist' });
check('an unknown asset fails with a locatable code', bad.json?.code === 'ASSET_NOT_FOUND', bad.json);
check('errors carry a hint', typeof bad.json?.hint === 'string' && bad.json.hint.length > 10, bad.json);

const ai = await c.rpc('image.op', { in: join(FIX, 'product.png'), out: 'cut.png', op: 'remove_bg', args: { mode: 'ai' } });
check('AI background removal says it needs a provider', ai.json?.code === 'PROVIDER_NOT_CONFIGURED', ai.json);

// Provider reachability and the settings contract, from the MCP door.
const probeNone = await c.rpc('system.provider.probe', {});
check('an unconfigured provider answers with a code, not a fake ok', probeNone.json?.code === 'PROVIDER_NOT_CONFIGURED', probeNone.json);
const probeDead = await c.rpc('system.provider.probe', { url: 'http://127.0.0.1:1', timeout: 700 });
check('an unreachable endpoint is named as NETWORK', probeDead.json?.code === 'NETWORK', probeDead.json);
const probeJunk = await c.rpc('system.provider.probe', { url: 'ftp://nowhere' });
check('a non-http scheme is a schema error', probeJunk.json?.code === 'INVALID_VALUE', probeJunk.json);
const badKey = await c.rpc('system.config', { set: { colour: 'blue' } });
check('an unknown setting is refused with the allowed list', badKey.json?.code === 'INVALID_VALUE' && String(JSON.stringify(badKey.json?.details ?? '')).includes('exportDir'), badKey.json);
const tok = await c.rpc('system.config', { set: { token: 'loopback-secret' } });
check('the access token is stored as presence only', tok.json?.tokenSet === true && tok.json?.token === undefined, tok.json);
const tokFile = readHead(join(root, 'settings.json'), 4096);
check('the token value is really on disk for the server to read', tokFile.includes('loopback-secret'), tokFile.slice(0, 120));
check('and a listing of the whole document never carries it', !JSON.stringify(tok.json).includes('loopback-secret'), JSON.stringify(tok.json).slice(0, 200));
const cleared = await c.rpc('system.config', { set: { token: null } });
check('the token can be removed again', cleared.json?.tokenSet === false, cleared.json);

c.stop();
const ok = summary('mcp-e2e');
console.log(ok ? '\nSCENARIO B: PASS' : '\nSCENARIO B: FAIL');
process.exit(ok ? 0 : 1);

function readHead(p, n) {
  try {
    const fd = openSync(p, 'r');
    const buf = Buffer.alloc(n);
    readSync(fd, buf, 0, n, 0);
    closeSync(fd);
    return buf.toString('utf8');
  } catch {
    return '';
  }
}
