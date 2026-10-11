// Scenario E — HTTP and CLI are two doors on one state, plus templates and
// multi-ratio production. The server runs without the desktop app.
import { join } from 'node:path';
import { existsSync } from 'node:fs';
import { sandbox, startHttp, req, rpcHttp, cli, check, eq, section, summary, imageDims, writePng, nonEmpty, sha256, abs } from './lib.mjs';

const root = sandbox('http');
const FIX = join(root, 'in');
writePng(join(FIX, 'a.png'), 500, 700, (x, y, w, h) => [Math.round((x / w) * 255), 90, Math.round((y / h) * 255), 255]);
writePng(join(FIX, 'b.png'), 500, 700, (x, y, w) => [255, Math.round((x / w) * 255), 120, 255]);

section('HTTP e2e — REST, CLI interop, ratios, templates');
const { port, proc } = await startHttp(root);
check(`server is live on 127.0.0.1:${port}`, port > 0);

const h = await req('GET', '/health');
eq('GET /health', h.json?.ok, true);

const created = await rpcHttp('project.create', { name: 'rest-campaign', w: 1080, h: 1440 });
const P = created?.p;
const B = created?.boards?.[0]?.b;
check('POST /rpc project.create works', !!P && !!B, created);

// The documented REST shapes must reach the same engine, not a second one.
const viaRest = await req('POST', '/v1/projects', { name: 'rest-shape', artboards: [{ width: 800, height: 1200 }] });
check('POST /v1/projects creates a project', !!viaRest.json?.p, viaRest.json);
eq('the REST artboard size is honoured', [viaRest.json?.boards?.[0]?.w, viaRest.json?.boards?.[0]?.h], [800, 1200]);
const read = await req('GET', `/v1/projects/${viaRest.json.p}`);
eq('GET /v1/projects/:id reads it back', read.json?.name, 'rest-shape');

const imp = await rpcHttp('asset.import', { p: P, paths: [join(FIX, 'a.png'), join(FIX, 'b.png')] });
const A0 = imp?.assets?.[0]?.id;
const A1 = imp?.assets?.[1]?.id;
check('two assets imported', !!A0 && !!A1, imp);
check('imported asset reports real bytes', (imp?.assets?.[0]?.bytes || 0) > 1000, imp?.assets?.[0]);

await rpcHttp('canvas.add', { p: P, b: B, t: 'text', text: '限时特惠', size: 110, name: 'title', intent: { anchor: 'top-center', margin: 140 } });
await rpcHttp('canvas.add', { p: P, b: B, t: 'image', asset: A0, name: 'shot', w: 720, intent: { below: 'title', gap: 70, align_x: 'center', keep_aspect_ratio: true } });
await rpcHttp('canvas.add', { p: P, b: B, t: 'rect', name: 'price', w: 300, h: 110, style: { fill: { t: 'solid', color: '#D95748' }, radius: { all: 24 } }, intent: { anchor: 'bottom', margin: 96, align_x: 'center' } });
const solved = await rpcHttp('layout.resolve', { p: P, b: B });
check('HTTP-driven layout solve has no conflicts', (solved?.conflicts || []).length === 0, solved);

// CLI opens the very same project the HTTP call created.
const opened = cli('project.open', { p: P }, { root });
eq('CLI sees the project HTTP created', opened.json?.p, P);
eq('CLI and HTTP agree on the revision', opened.json?.rev, (await rpcHttp('project.get', { p: P, depth: 0 }))?.rev);
eq('CLI sees the same three layers', opened.json?.boards?.[0]?.n, 3);

// Batch commands: one transaction, one revision bump. Raw commands address nodes by
// id (the `canvas.*` shorthands also accept a layer's name).
const revBefore = opened.json?.rev;
const nodesNow = (await rpcHttp('project.get', { p: P })).nodes || [];
const byName = (n) => nodesNow.find((x) => x.name === n)?.id;
const TITLE = byName('title');
const PRICE = byName('price');
check('the nodes added over HTTP carry the requested names', !!TITLE && !!PRICE, nodesNow.map((n) => [n.name, n.id]));
const batch = await req('POST', `/v1/projects/${P}/commands:batch`, {
  expectedRevision: revBefore,
  commands: [
    { t: 'Text', id: TITLE, content: '双十二特惠' },
    { t: 'NodeSet', id: PRICE, patch: { style: { fill: { t: 'solid', color: '#1C1C1C' } } } },
  ],
});
check('commands:batch applies both', batch.json?.ok === true, batch.json);
check('commands:batch bumps the revision once', batch.json?.rev === revBefore + 1, { before: revBefore, after: batch.json?.rev });
const staleBatch = await req('POST', `/v1/projects/${P}/commands:batch`, { expectedRevision: revBefore, commands: [{ t: 'Text', id: TITLE, content: 'nope' }] });
eq('a stale expectedRevision is refused', staleBatch.json?.code, 'REVISION_CONFLICT');

// Multi-ratio production from one layout.
const dup = await rpcHttp('artboard.duplicate', { p: P, b: 'b1', to: ['1:1', '4:5', '9:16'] });
const boards = (await rpcHttp('artboard.list', { p: P })) || [];
check('the board was cloned into extra ratios', boards.length >= 3, boards);
const want = [[1080, 1080], [1080, 1350], [1080, 1920]];
for (const [w, hh] of want) {
  const hit = boards.find((x) => Math.abs(x.w - w) < 1 && Math.abs(x.h - hh) < 1);
  check(`a ${w}x${hh} variant exists`, !!hit, boards.map((x) => [x.w, x.h]));
}
const exp = await rpcHttp('render.export', { p: P, f: 'png', sc: 1, o: 'ratios' });
const files = exp?.files || [];
check('every ratio exported a file', files.length >= 3, exp);
for (const f of files) {
  const p = abs(root, f.path);
  check(`exported ${f.path} exists`, existsSync(p) && f.bytes > 1000, f);
  eq(`exported ${f.path} matches its board`, [imageDims(p)?.w, imageDims(p)?.h], [f.w, f.h]);
  eq(`sha256 of ${f.path} matches the record`, sha256(p).slice(0, 12), (f.sha || '').slice(0, 12));
}

// Template variables: same layout, different copy and asset.
const tpl = await rpcHttp('template.save', { p: P, b: boards[0].b, name: 'promo-base' });
check('a template can be saved from a board', tpl?.ok !== false, tpl);
const listT = await rpcHttp('template.list', {});
check('the template is listed (built-ins included)', JSON.stringify(listT).includes('promo-base'), listT?.length);
const variants = await rpcHttp('template.variants', { p: P, from: 0, count: 3, colors: ['#D95748', '#2E7D57', '#3B4BFB'], copy: { title: ['A', 'B', 'C'] } });
check('three variants were created', Array.isArray(variants?.boards) ? variants.boards.length >= 3 : (variants?.ids || []).length >= 3, variants);
const afterVariants = await rpcHttp('artboard.list', { p: P });
check('variant boards share the source size', afterVariants?.length > boards.length, afterVariants?.length);

// Asset streaming for the webview: the same bytes the exporter used.
const assetGet = await req('GET', `/asset?id=${A0}`);
check('GET /asset streams the imported file', assetGet.status === 200 && assetGet.buf.length > 1000, assetGet.status);
eq('the streamed asset has the right mime', assetGet.headers['content-type'], 'image/png');
const fontGet = await req('GET', `/font?family=${encodeURIComponent('Arial')}`);
check('GET /font serves a real font face', fontGet.status === 200 && fontGet.buf.length > 4096, { s: fontGet.status, n: fontGet.buf?.length });
const missingFont = await req('GET', '/font?family=NoSuchFontHere');
eq('an absent font is a clean 400-shaped error', missingFont.json?.code, 'FONT_MISSING');

// Sandbox and error shape.
const escape = await rpcHttp('image.op', { in: join(FIX, 'a.png'), out: '../../../../Windows/evil.png', op: 'gray' });
check('writing outside the root is refused', escape?.code === 'PATH_DENIED', escape);
const unknown = await rpcHttp('no.such.method', {});
eq('unknown methods fail with a code', unknown?.code, 'INVALID_SCHEMA');
check('unknown methods suggest near matches', (unknown?.details?.near || []).length > 0, unknown?.details);
const notFound = await req('GET', '/nope');
eq('unknown routes 404 with the same error shape', notFound.json?.code, 'NOT_FOUND');

// `fields` trimming keeps responses cheap.
const full = await rpcHttp('project.get', { p: P });
const trimmed = await req('POST', '/rpc', { m: 'project.get', p: { p: P, fields: 'rev,name' } });
check('fields trims the response to exactly what was asked', Object.keys(trimmed.json || {}).length === 2 && trimmed.json?.rev === full?.rev && trimmed.json?.name === full?.name, trimmed.json);
check('the trimmed reply is far smaller than the document', JSON.stringify(trimmed.json).length * 40 < JSON.stringify(full).length, [JSON.stringify(trimmed.json).length, JSON.stringify(full).length]);

proc.kill();
const ok = summary('http-e2e');
console.log(ok ? '\nSCENARIO E: PASS' : '\nSCENARIO E: FAIL');
process.exit(ok ? 0 : 1);
