// Scenarios A, C and D. These run through the CLI process, which dispatches the
// exact same `rpc::call` the desktop window invokes over `invoke("rpc")` — one
// engine, so a pass here is a pass for the human path too (the GUI itself is
// exercised separately by tests/ui.mjs).
import { join } from 'node:path';
import { existsSync, readFileSync } from 'node:fs';
import { sandbox, cli, rpc, check, eq, section, summary, imageDims, writePng, nonEmpty, sha256 } from './lib.mjs';

const root = sandbox('accept');
const FIX = join(root, 'in');
// A synthetic "product photo": a soft disc on a warm ground, plus a logo SVG.
writePng(join(FIX, 'product.png'), 640, 860, (x, y, w, h) => {
  const u = (x - w / 2) / w;
  const v = (y - h * 0.45) / h;
  const inDisc = Math.hypot(u, v) < 0.31;
  const edge = Math.abs(Math.hypot(u, v) - 0.31) < 0.006;
  if (edge) return [20, 20, 30, 255];
  if (inDisc) return [Math.round(60 + 160 * (1 - v)), 90, Math.round(200 - 90 * u * u * 4), 255];
  return [247, 245, 241, 255];
});
writePng(join(FIX, 'second.png'), 640, 860, (x, y, w) => (x / w < 0.5 ? [24, 160, 120, 255] : [250, 250, 250, 255]));
const { writeFileSync } = await import('node:fs');
writeFileSync(join(FIX, 'logo.svg'), `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 60"><rect width="200" height="60" rx="12" fill="#1C1C1C"/><text x="16" y="40" font-size="28" fill="#fff">BRAND</text></svg>`);

// A call that produced no JSON is a failed assertion, not a crash: aborting here
// would silently remove every check after it from the report.
const J = (r) => {
  if (r.json === null || r.json === undefined) {
    check(`the CLI answered with JSON (${r.method})`, false, { out: (r.out || r.raw || '').slice(0, 300), err: (r.err || '').slice(0, 300), code: r.code });
    return {};
  }
  return r.json;
};

// ------------------------------------------------------------------ A
section('A — material to finished poster, the human path');
const created = J(cli('project.create', { name: 'autumn', w: 1080, h: 1440 }, { root }));
const P = created.p;
const B = created.boards[0].b;
check('A1 a 1080x1440 project exists', !!P && created.boards[0].w === 1080, created);

const imported = J(cli('asset.import', { p: P, paths: [join(FIX, 'product.png'), join(FIX, 'logo.svg')] }, { root }));
const [prod, logo] = imported.assets || [];
check('A2 a PNG and an SVG both imported', !!prod?.id && !!logo?.id, imported);
eq('A2 the PNG carries real size', [prod.w, prod.h], [640, 860]);
eq('A2 the SVG is recognised as vector', logo.kind, 'vector');

const t1 = J(cli('canvas.add', { p: P, b: B, t: 'text', text: '少即是多', size: 92, name: 'title', intent: { anchor: 'top-center', margin: 110 } }, { root }));
const TID = (t1.created || [])[0];
const t2 = J(cli('canvas.add', { p: P, b: B, t: 'text', text: '为 AI 代理准备的画布与排版引擎，一次描述即可成片。', size: 26, name: 'body', w: 700, intent: { below: TID, gap: 26, align_x: 'center' } }, { root }));
const BID = (t2.created || [])[0];
const im = J(cli('canvas.add', { p: P, b: B, t: 'image', asset: prod.id, name: 'product', w: 720, intent: { below: BID, gap: 70, align_x: 'center', keep_aspect_ratio: true } }, { root }));
const IID = (im.created || [])[0];
const lg = J(cli('canvas.add', { p: P, b: B, t: 'svg', asset: logo.id, name: 'logo', w: 180, intent: { anchor: 'bottom-left', margin: 72 } }, { root }));
const LID = (lg.created || [])[0];
check('A3 four layers were placed with intents', !!TID && !!BID && !!IID && !!LID, { TID, BID, IID, LID });

const solved = J(cli('layout.resolve', { p: P, b: B }, { root }));
check('A4 the layout solves without conflict', (solved.conflicts || []).length === 0, solved.conflicts);
const doc = J(cli('project.get', { p: P, depth: 1 }, { root }));
const box = (id) => (doc.nodes || []).find((n) => n.id === id)?.box || {};
check('A4 the title honours its top margin', Math.abs(box(TID).y - 110) < 2, box(TID));
check('A4 the logo sits in the bottom-left safe corner', box(LID).y + box(LID).h <= 1440 - 60 && box(LID).x >= 60, box(LID));
check('A4 nothing drifted off the board', (doc.nodes || []).every((n) => n.box.x >= -1 && n.box.y >= -1), (doc.nodes || []).map((n) => [n.id, n.box]));

const val = J(cli('inspect.validate', { p: P }, { root }));
check('A4 the finished board has no error-level issue', val.errors === 0, (val.issues || []).filter((i) => i.s === 'error'));

// manual edit, then undo, then save
const before = JSON.stringify(box(IID));
const moved = J(cli('canvas.transform', { p: P, id: IID, x: box(IID).x + 60, y: box(IID).y + 40, w: box(IID).w, h: box(IID).h }, { root }));
check('A5 a manual drag is accepted', moved.ok === true, moved);
const afterMove = JSON.stringify(J(cli('project.get', { p: P, depth: 1 }, { root })).nodes.find((n) => n.id === IID).box);
check('A5 the drag really moved the node', afterMove !== before, { before, afterMove });
const edited = J(cli('canvas.text', { p: P, id: TID, content: '少，但是更好' }, { root }));
check('A5 copy can be edited', edited.ok === true, edited);
const undone = J(cli('history.undo', { p: P }, { root }));
check('A5 undo reports success', undone.ok !== false, undone);
const afterUndo = J(cli('project.get', { p: P, depth: 1 }, { root }));
eq('A5 the text edit was undone', afterUndo.nodes.find((n) => n.id === TID).text.content, '少即是多');
const saved = J(cli('project.save', { p: P }, { root }));
check('A5 the project saves to a real path', nonEmpty(join(root, saved.path || '')) || existsSync(saved.path), saved);

const closed = J(cli('project.close', { p: P }, { root }));
check('A6 it can be closed', closed.ok === true, closed);
const reopened = J(cli('project.open', { p: P }, { root }));
check('A6 reopening works', reopened.p === P, reopened);
const doc2 = J(cli('project.get', { p: P, depth: 1 }, { root }));
check('A6 the full document answers with the same short keys as the summary', typeof doc2.rev === 'number' && doc2.p === P && doc2.revision === undefined, Object.keys(doc2));
eq('A6 the document remembers which door made it', doc2.meta?.origin, 'cli');
const trimmed = J(cli('project.get', { p: P, fields: 'rev,name' }, { root }));
eq('A6 fields=rev,name trims a document too', Object.keys(trimmed).sort(), ['name', 'rev']);
eq('A6 the revision survived', doc2.rev, afterUndo.rev);
eq('A6 every layer survived', doc2.nodes.length, afterUndo.nodes.length);
eq('A6 the asset reference survived', doc2.nodes.find((n) => n.id === IID).image.asset, prod.id);
eq('A6 the font size survived', doc2.nodes.find((n) => n.id === TID).text.size, 92);

const exports1 = J(cli('render.export', { p: P, f: 'png', sc: 2, o: 'final' }, { root }));
const pngRec = (exports1.files || [])[0];
const pngAbs = join(root, pngRec?.path || '');
check('A7 PNG exported', nonEmpty(pngAbs), exports1);
eq('A7 the 2x scale is honoured', [imageDims(pngAbs)?.w, imageDims(pngAbs)?.h], [2160, 2880]);
eq('A7 the sha in the reply is the file sha', sha256(pngAbs).slice(0, 16), (pngRec?.sha || '').slice(0, 16));
for (const f of ['svg', 'pdf', 'jpg', 'webp']) {
  const r = J(cli('render.export', { p: P, f, sc: 1, o: `final-${f}` }, { root }));
  const rec = (r.files || [])[0];
  const abs = join(root, rec?.path || '');
  check(`A7 ${f.toUpperCase()} exported and openable`, nonEmpty(abs) && (imageDims(abs)?.fmt || 'x') !== 'unknown', r);
  if (f === 'pdf') check('A7 the PDF has a real xref', readFileSync(abs, 'latin1').includes('startxref'));
  if (f === 'svg') check('A7 the SVG contains the headline text', readFileSync(abs, 'utf8').includes('少即是多'));
}

// ------------------------------------------------------------------ C
section('C — three-poster series from one layout');
const series = J(cli('project.create', { name: 'series', preset: 'poster' }, { root }));
const SP = series.p;
const SB = series.boards[0].b;
const sAssets = J(cli('asset.import', { p: SP, paths: [join(FIX, 'product.png'), join(FIX, 'second.png'), join(FIX, 'product.png')] }, { root }));
check('C1 three product shots imported', (sAssets.assets || []).length >= 2, sAssets); // identical file dedupes by sha
check('C1 dedupe is reported rather than silently duplicating', (sAssets.assets || []).some((a) => a.dup), sAssets.assets);

const base = J(cli('poster.nodes', {
  p: SP, b: SB,
  nodes: [
    { name: 'title', t: 'text', text: '新品首发', size: 96, intent: { anchor: 'top-center', margin: 120 } },
    { name: 'shot', t: 'image', asset: sAssets.assets[0].id, w: 700, intent: { center_in: SB, keep_aspect_ratio: true } },
    { name: 'brand', t: 'text', text: 'BRAND', size: 24, intent: { anchor: 'bottom-left', margin: 72 } },
  ],
}, { root }));
check('C2 a shared template layout was built', base.ok === true, base);
const byName = (nm) => (J(cli('project.get', { p: SP, depth: 1 }, { root })).nodes || []).find((n) => n.name === nm);
const tplBoard = byName('title');
check('C2 the template board solved', !!tplBoard && tplBoard.box.y > 0, tplBoard?.box);

const variants = J(cli('template.variants', {
  p: SP, from: 0, count: 3,
  colors: ['#D95748', '#2E7D57', '#3B4BFB'],
  copy: { title: ['新品首发', '限时特惠', '冬季限定'] },
  assets: (sAssets.assets || []).map((a) => a.id),
}, { root }));
const vBoards = variants.boards || variants.ids || [];
check('C3 three variant boards exist', vBoards.length >= 3, variants);
const allBoards = J(cli('artboard.list', { p: SP }, { root }));
check('C3 every variant keeps the source size', allBoards.every((a) => a.w === allBoards[0].w && a.h === allBoards[0].h), allBoards);

const struct = allBoards.map((a) => {
  const d = J(cli('project.get', { p: SP, depth: 1 }, { root }));
  return d.nodes.filter((n) => n.artboard === a.b || (d.artboards.find((x) => x.id === a.b)?.layers || []).includes(n.id)).map((n) => [n.name, Math.round(n.box.x), Math.round(n.box.y), Math.round(n.box.w)]);
});
check('C4 the composition is the same across the series', struct.length >= 2 && JSON.stringify(struct[0].map((r) => r[0])) === JSON.stringify(struct[1].map((r) => r[0])), struct.slice(0, 2));
check('C4 and so are the positions (that is the point of a series)', JSON.stringify(struct[0]) === JSON.stringify(struct[struct.length - 1]), [struct[0], struct[struct.length - 1]]);

const batchExp = J(cli('render.export_batch', { p: SP, f: 'png', sc: 1, o: 'series', specs: [{ f: 'png', sc: 1, suffix: '1x' }, { f: 'png', sc: 2, suffix: '2x' }] }, { root }));
check('C4 batch export produced files for every board', (batchExp.files || []).length >= 6, batchExp);
const allValid = (batchExp.files || []).every((f) => {
  const abs = join(root, f.path);
  return nonEmpty(abs) && sha256(abs).slice(0, 12) === (f.sha || '').slice(0, 12);
});
check('C4 every exported file matches its reported sha', allValid, batchExp.files?.slice(0, 2));

// ------------------------------------------------------------------ D
section('D — conflicts, refusals and recovery');
const lockDoc = J(cli('project.get', { p: SP, depth: 1 }, { root }));
const brandNode = lockDoc.nodes.find((n) => n.name === 'brand');
J(cli('canvas.update', { p: SP, id: brandNode.id, s: { locked: true } }, { root }));
const tryMove = J(cli('canvas.transform', { p: SP, id: brandNode.id, x: 900, y: 900, w: 100, h: 40 }, { root }));
eq('D1 moving a locked layer is refused with a code', tryMove.code, 'LOCKED');
const stillThere = J(cli('project.get', { p: SP, depth: 1 }, { root })).nodes.find((n) => n.id === brandNode.id);
eq('D1 the locked layer did not move', stillThere.box.x, brandNode.box.x);
const forced = J(cli('canvas.transform', { p: SP, id: brandNode.id, x: 900, y: 900, w: 100, h: 40, force: true }, { root }));
check('D1 an explicit force override still works', forced.ok === true, forced);

const conf = J(cli('canvas.add_many', {
  p: SP, b: SB,
  nodes: [
    { name: 'cyc_a', t: 'rect', w: 100, h: 100, intent: { below: 'cyc_b' } },
    { name: 'cyc_b', t: 'rect', w: 100, h: 100, intent: { above: 'cyc_a' } },
  ],
}, { root }));
check('C2/D2 a cyclic constraint set is reported, not hung', conf.ok === true || conf.code, conf);
const confSolve = J(cli('layout.resolve', { p: SP, b: SB }, { root }));
check('D2 the solver names the cycle', (confSolve.conflicts || []).some((c) => String(c.why).includes('cycle')), confSolve);
check('D2 and it says which intent collided', (confSolve.conflicts || []).every((c) => c.n && c.i), confSolve.conflicts);
const ghost = J(cli('canvas.add', { p: SP, b: SB, t: 'rect', w: 40, h: 40, intent: { below: 'te_does_not_exist' } }, { root }));
check('D2 a relation pointing at nothing is reported, not silently ignored', String(ghost.warnings || []).includes('no node or board'), ghost);
const ghostSolve = J(cli('layout.resolve', { p: SP, b: SB }, { root }));
check('D2 and the solve names it too', (ghostSolve.conflicts || []).some((c) => String(c.with) === 'te_does_not_exist'), ghostSolve.conflicts);
const confDoc = J(cli('project.get', { p: SP, depth: 1 }, { root }));
check('D2 no NaN geometry escaped into the document', confDoc.nodes.every((n) => Number.isFinite(n.box.x) && Number.isFinite(n.box.y) && Number.isFinite(n.box.w)), confDoc.nodes.filter((n) => !Number.isFinite(n.box.x)).map((n) => n.id));

const revNow = confDoc.rev;
// An unlocked node: a locked one fails the lock check first, which is a different
// guarantee and belongs to D1.
const plain = byName('title');
const stale = J(cli('canvas.text', { p: SP, id: plain.id, content: 'x', rev: revNow - 3 }, { root }));
eq('D3 a stale revision is rejected', stale.code, 'REVISION_CONFLICT');
check('D3 the response carries the current revision', typeof stale.details?.rev === 'number', stale.details);
const retry = J(cli('canvas.text', { p: SP, id: plain.id, content: '限时首发', rev: stale.details?.rev }, { root }));
check('D3 the caller can retry safely with the fresh revision', retry.ok === true, retry);

const missingAsset = J(cli('canvas.add', { p: SP, b: SB, t: 'image', asset: 'nope' }, { root }));
eq('D4 an absent asset is a locatable error', missingAsset.code, 'ASSET_NOT_FOUND');
const missingFile = J(cli('image.probe', { path: join(FIX, 'gone.png') }, { root }));
eq('D4 an absent file is a locatable error', missingFile.code, 'NOT_FOUND');
const badColor = J(cli('canvas.add', { p: SP, b: SB, t: 'rect', style: { fill: { t: 'solid', color: 'not-a-colour' } } }, { root }));
check('D4 a bad colour is caught at the schema boundary', badColor.code === 'INVALID_VALUE' || badColor.code === 'INVALID_SCHEMA', badColor);
const badRatio = J(cli('artboard.create', { p: SP, ratio: 'abc' }, { root }));
eq('D4 a bad ratio is caught', badRatio.code, 'INVALID_SCHEMA');
check('D4 and it explains the accepted form', String(badRatio.hint).includes('16:9'), badRatio);
const noProvider = J(cli('image.op', { in: join(FIX, 'product.png'), out: 'ai.png', op: 'remove_bg', args: { mode: 'ai' } }, { root }));
eq('D4 AI work without a provider fails honestly', noProvider.code, 'PROVIDER_NOT_CONFIGURED');

const revBeforeTx = J(cli('project.get', { p: SP, depth: 0 }, { root })).rev;
const tx = J(cli('canvas.batch', {
  p: SP,
  cmds: [
    { t: 'Text', id: brandNode.id, content: 'should roll back' },
    { t: 'NodeSet', id: 'no_such_node', patch: { opacity: 0.5 } },
  ],
}, { root }));
check('D5 a failing batch reports the failure', tx.ok === false || tx.code, tx);
const afterTx = J(cli('project.get', { p: SP, depth: 1 }, { root }));
eq('D5 the first command of the failed transaction did not stick', afterTx.nodes.find((n) => n.id === brandNode.id).text.content, 'BRAND');
eq('D5 the revision did not move either', afterTx.rev, revBeforeTx);

const huge = J(cli('image.op', { in: join(FIX, 'product.png'), out: 'x.png', op: 'resize', args: { w: 40000, h: 40000 } }, { root }));
check('D6 absurd output sizes are refused rather than OOMing', huge.code === 'TOO_LARGE' || huge.code === 'INVALID_VALUE', huge);

section('caps honesty');
const caps = J(cli('caps', {}, { root }));
check('caps advertises the methods that actually exist', (caps.methods || []).length > 60, caps.methods?.length);
const declared = new Set((caps.methods || []).map((s) => String(s).split(/\s+/)[0]));
for (const probe of ['poster', 'canvas.add', 'layout.explain', 'inspect.validate', 'render.export', 'image.chain', 'text.measure']) {
  check(`caps declares ${probe}`, declared.has(probe), [...declared].slice(0, 8));
}
check('caps names the real limits', typeof caps.limits?.max_px === 'number', caps.limits);
check('caps is compact enough to be the first read', JSON.stringify(caps).length < 14_000, JSON.stringify(caps).length);

/* The self-description is the first thing an agent reads, so `full` has to be the
   expensive option rather than the default. */
const toolsShort = J(cli('system.tools', {}, { root }));
const toolsFull = J(cli('system.tools', { full: 1 }, { root }));
const nOf = (v) => (Array.isArray(v) ? v : []).map((t) => t.n ?? t.name).filter(Boolean);
check('system.tools lists every tool without full=1', nOf(toolsShort).length > 30 && nOf(toolsShort).length === nOf(toolsFull).length, {
  short: nOf(toolsShort).length,
  full: nOf(toolsFull).length,
});
check(
  'system.tools without full=1 costs less than half',
  JSON.stringify(toolsShort).length * 2 < JSON.stringify(toolsFull).length,
  { short: JSON.stringify(toolsShort).length, full: JSON.stringify(toolsFull).length },
);

/* `batch` is the round-trip saver, so the documented `jobs=[…]` argv spelling must
   reach the same handler as `agentcanvas batch '[…]'`. */
const b1 = J(cli('batch', { jobs: [{ m: 'version', p: {} }, { m: 'caps', p: { fields: 'version' } }] }, { root }));
check('batch runs both jobs through k=v argv', Array.isArray(b1.r) && b1.r.length === 2 && b1.r[0].ok !== false, b1.r);
const b2 = cli('batch', { 'jobs:': '[{"m":"version","p":{}}]' }, { root });
check('batch accepts the k:=json argv form too', Array.isArray(b2.json?.r) && b2.json.r.length === 1, { out: b2.out.slice(0, 160) });
const b3 = J(cli('batch', { jobs: [{ m: 'version', p: {} }, { m: 'nope.method', p: {} }] }, { root }));
check('a bad job is reported, not fatal to the batch', (b3.r || []).length === 2 && (b3.r || []).some((x) => x.code) && (b3.r || []).some((x) => x.ok !== false), b3.r);

/* Serialization economy: a whole f64 used to leave as `1440.0`, which is two tokens
   per number in a reply that carries hundreds of them. */
const rawDoc = cli('project.get', { p: P, depth: 1 }, { root });
check('a whole number is written without a decimal point', !/\d+\.0(?=[,}\]])/.test(rawDoc.raw), rawDoc.raw.slice(0, 200));
check('the trimmed document still answers with its real numbers', /"rev":\d+/.test(rawDoc.raw), rawDoc.raw.slice(0, 120));
const health = J(cli('system.health', {}, { root }));
check(
  'system.health names the root it actually resolved',
  typeof health.root === 'string' && /^[A-Za-z]:[\\/]/.test(health.root),
  health.root,
);

const ok = summary('acceptance');
console.log(ok ? '\nSCENARIOS A/C/D: PASS' : '\nSCENARIOS A/C/D: FAIL');
process.exit(ok ? 0 : 1);
