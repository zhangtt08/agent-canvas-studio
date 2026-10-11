// Builds every example in examples/ from the real engine: real projects, real
// exported images. Run after scripts/build.mjs:  node examples/run-all.mjs
// Each example is also a regression fixture — the acceptance suites assert on the
// same shapes, so an example that stops producing files fails the gate.
import { mkdirSync, existsSync, readFileSync, writeFileSync, readdirSync, rmSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { ROOT, sandbox, cli, rpc, check, summary, section, imageDims, writePng, sha256 } from '../tests/lib.mjs';

/** The example workspace *is* `examples/`: the engine refuses to write outside its
 *  root, and the projects it creates are part of the deliverable (示例工程), so the
 *  root, the fixtures, the project state and the exported files all live together. */
const root = process.env.AGENTCANVAS_HOME || join(ROOT, 'examples');
const OUT = join(ROOT, 'examples');
const FIX = join(OUT, 'fixtures');
mkdirSync(FIX, { recursive: true });

/** Deterministic synthetic "product photography" so examples need no licensed assets. */
function makeShot(name, w, h, tint) {
  const p = join(FIX, name);
  if (!existsSync(p)) {
    writePng(p, w, h, (x, y) => {
      const u = (x - w / 2) / w;
      const v = (y - h * 0.46) / h;
      const r = Math.hypot(u, v * 0.85);
      if (r < 0.3) {
        const sh = Math.max(0, 1 - r / 0.3);
        return [tint[0] * sh + 30, tint[1] * sh + 40, tint[2] * sh + 60, 255];
      }
      if (r < 0.312) return [18, 18, 24, 255];
      const g = 246 - Math.round(v * 26);
      return [g, g - 2, g - 6, 255];
    });
  }
  return p;
}

const shots = [
  makeShot('product-a.png', 720, 960, [92, 122, 235]),
  makeShot('product-b.png', 720, 960, [226, 122, 86]),
  makeShot('product-c.png', 720, 960, [52, 168, 128]),
];
writeFileSync(join(FIX, 'logo.svg'), '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 220 64"><rect width="220" height="64" rx="14" fill="#1C1C1C"/><text x="18" y="43" font-family="Arial" font-size="30" letter-spacing="4" fill="#ffffff">BRAND</text></svg>');

/** Every example owns a project with a fixed id, so a re-run replaces its sibling
 *  instead of piling up `proj_*` directories nobody asked for. */
const EXAMPLES = ['product-poster', 'three-poster-campaign', 'editorial-cover', 'batch-resize', 'headless-mcp', 'image-pipeline'];

// An example is regenerated, not accumulated: render.export never overwrites, so
// without this a second run would leave *-2.png files beside the real ones. The
// engine state (projects, log, recents) is wiped for the same reason: the second
// run must start from the same world as the first, or the golden comparison below
// is comparing two different documents.
for (const d of EXAMPLES) rmSync(join(OUT, d, 'out'), { recursive: true, force: true });
rmSync(join(OUT, 'projects'), { recursive: true, force: true });
rmSync(join(OUT, 'log'), { recursive: true, force: true });
rmSync(join(OUT, 'recents.json'), { force: true });

/** The previous golden record, if any, for the determinism check at the end. */
const hadGolden = existsSync(join(OUT, 'MANIFEST.json'));
const golden = hadGolden ? JSON.parse(readFileSync(join(OUT, 'MANIFEST.json'), 'utf8')).files || [] : [];

section('examples');

// ---------------------------------------------------------------- 1. single poster
const one = cli('project.create', { id: 'product-poster', name: 'product-poster', preset: 'poster' }, { root });
const P1 = one.json?.p;
const B1 = one.json?.boards?.[0]?.b;
const a1 = cli('asset.import', { p: P1, paths: [shots[0], join(FIX, 'logo.svg')] }, { root });
cli('canvas.add_many', {
  p: P1, b: B1,
  nodes: [
    { name: 'title', t: 'text', text: '少即是多', size: 96, weight: 800, color: '#1C1C1C', intent: { anchor: 'top-center', margin: 120 } },
    { name: 'subtitle', t: 'text', text: 'LESS, BUT BETTER.', size: 28, ls: 4, color: '#6B7280', intent: { below: 'title', gap: 18, align_x: 'center' } },
    { name: 'product', t: 'image', asset: a1.json?.assets?.[0]?.id, w: 720, intent: { below: 'subtitle', gap: 70, align_x: 'center', keep_aspect_ratio: true } },
    { name: 'logo', t: 'svg', asset: a1.json?.assets?.[1]?.id, w: 180, intent: { anchor: 'bottom-left', margin: 72 } },
    { name: 'date', t: 'text', text: '2026 秋季系列', size: 22, color: '#6B7280', intent: { anchor: 'bottom-right', margin: 72 } },
  ],
}, { root });
cli('layout.resolve', { p: P1, b: B1 }, { root });
const v1 = cli('inspect.validate', { p: P1 }, { root });
const e1 = cli('render.export', { p: P1, f: 'png', sc: 1, o: join(OUT, 'product-poster', 'out') }, { root });
check('product-poster exported', (e1.json?.files || []).length >= 1, e1.json);
check('product-poster validates without errors', v1.json?.errors === 0, (v1.json?.issues || []).filter((i) => i.s === 'error').slice(0, 2));
cli('project.save', { p: P1 }, { root });

// ---------------------------------------------------------------- 2. series of three
const ser = cli('project.create', { id: 'three-poster-campaign', name: 'three-poster-campaign', preset: 'poster' }, { root });
const P2 = ser.json?.p;
const B2 = ser.json?.boards?.[0]?.b;
const a2 = cli('asset.import', { p: P2, paths: shots }, { root });
cli('canvas.add_many', {
  p: P2, b: B2,
  nodes: [
    { name: 'title', t: 'text', text: '新品首发', size: 92, intent: { anchor: 'top-center', margin: 120 } },
    { name: 'product', t: 'image', asset: a2.json?.assets?.[0]?.id, w: 700, intent: { center_in: B2, keep_aspect_ratio: true } },
    { name: 'price', t: 'rect', w: 280, h: 104, style: { fill: { t: 'solid', color: '#D95748' }, radius: { all: 26 } }, intent: { anchor: 'bottom', margin: 120, align_x: 'center' } },
    { name: 'price_label', t: 'text', text: '¥ 299', size: 46, color: '#FFFFFF', intent: { center_in: 'price' } },
  ],
}, { root });
cli('layout.resolve', { p: P2, b: B2 }, { root });
const varr = cli('template.variants', {
  p: P2, from: 0, count: 3,
  colors: ['#D95748', '#2E7D57', '#3B4BFB'],
  copy: { title: ['新品首发', '限时特惠', '冬季限定'] },
  assets: (a2.json?.assets || []).map((x) => x.id),
}, { root });
check('three-poster-campaign built 3 boards', ((varr.json?.boards) || []).length >= 3 || ((varr.json?.ids) || []).length >= 3, varr.json);
const e2 = cli('render.export', { p: P2, f: 'png', sc: 1, o: join(OUT, 'three-poster-campaign', 'out') }, { root });
check('all three posters exported', (e2.json?.files || []).length >= 3, e2.json);

// ---------------------------------------------------------------- 3. editorial cover
const ed = cli('project.create', { id: 'editorial-cover', name: 'editorial-cover', w: 1200, h: 1600, background: '#F7F5F1' }, { root });
const P3 = ed.json?.p;
const B3 = ed.json?.boards?.[0]?.b;
const a3 = cli('asset.import', { p: P3, path: shots[1] }, { root });
cli('canvas.add_many', {
  p: P3, b: B3,
  nodes: [
    { name: 'masthead', t: 'text', text: 'REVIEW', size: 190, weight: 900, color: '#1C1C1C', intent: { anchor: 'top-center', margin: 60 } },
    { name: 'subject', t: 'image', asset: a3.json?.assets?.[0]?.id, w: 860, intent: { center_in: B3, keep_aspect_ratio: true } },
    { name: 'coverline', t: 'text', text: '设计的秩序感', size: 74, intent: { below: 'subject', gap: 40, align_x: 'center' } },
    { name: 'deck', t: 'text', text: '十二位设计师谈留白、层级与节奏', size: 26, color: '#6B7280', w: 900, intent: { below: 'coverline', gap: 16, align_x: 'center' } },
    { name: 'rule', t: 'line', w: 1000, style: { stroke: { color: '#1C1C1C', width: 2 } }, intent: { above: 'masthead', gap: 18, align_x: 'center' } },
  ],
}, { root });
cli('layout.resolve', { p: P3, b: B3 }, { root });
const e3 = cli('render.export', { p: P3, f: 'pdf', o: join(OUT, 'editorial-cover', 'out') }, { root });
check('editorial-cover exported a PDF', (e3.json?.files || []).length >= 1, e3.json);

// ---------------------------------------------------------------- 4. batch resize
const br = cli('project.create', { id: 'batch-resize', name: 'batch-resize', preset: 'square' }, { root });
const P4 = br.json?.p;
const B4 = br.json?.boards?.[0]?.b;
const a4 = cli('asset.import', { p: P4, path: shots[0] }, { root });
cli('canvas.add', { p: P4, b: B4, t: 'image', asset: a4.json?.assets?.[0]?.id, name: 'art', intent: { fit: 'cover' } }, { root });
cli('artboard.duplicate', { p: P4, b: B4, to: ['1:1', '4:5', '9:16', '16:9'] }, { root });
cli('layout.resolve', { p: P4 }, { root });
const e4 = cli('render.export_batch', { p: P4, o: join(OUT, 'batch-resize', 'out'), specs: [{ f: 'png', sc: 1, suffix: '1x' }, { f: 'jpg', q: 88, sc: 1, suffix: 'web' }] }, { root });
check('batch-resize produced one file per board per spec', (e4.json?.files || []).length >= 8, e4.json?.files?.length);
const dims = (e4.json?.files || []).map((f) => [f.w, f.h]);
check('the ratios really differ', new Set(dims.map((d) => (d[0] / d[1]).toFixed(2))).size >= 4, dims);

// ---------------------------------------------------------------- 5. headless MCP
const p5 = join(OUT, 'headless-mcp');
mkdirSync(p5, { recursive: true });
writeFileSync(join(p5, 'README.md'), '# 无界面调用\n\n见 `tests/mcp-e2e.mjs`：它不启动任何窗口，只用 MCP stdio 建工程、排版、检查、导出。\n\n```powershell\nagentcanvas mcp    # stdio 服务\nnode tests/mcp-e2e.mjs\n```\n');
const poster = cli('poster', { id: 'headless-mcp', name: 'headless-mcp', title: '无界面也能出片', subtitle: 'HEADLESS', image: shots[2], accent: '#3B4BFB', preset: 'poster', f: 'png', out: join(OUT, 'headless-mcp', 'out') }, { root });
check('poster() one-call path produced a file', (poster.json?.files || []).length >= 1, poster.json?.e || poster.json?.code);

// ---------------------------------------------------------------- 6. image pipeline
const ip = join(OUT, 'image-pipeline', 'out');
mkdirSync(ip, { recursive: true });
const chain = cli('image.chain', { in: shots[0], out: join(ip, 'story.webp'), chain: 'crop 9:16 > scale 0.6 > sharpen 1.1 > border 8 #FFFFFF > watermark AgentCanvas', q: 86 }, { root });
check('image.chain ran the string form', chain.json?.out, chain.json);
const batch = cli('image.batch', { paths: shots, out: ip, fmt: 'jpg', q: 82, chain: [{ op: 'resize', w: 800, h: 800, fit: 'contain' }, { op: 'adjust', brightness: 1.04, saturation: 1.1 }] }, { root });
check('image.batch processed every file', (batch.json?.r || []).every((x) => x.ok) && (batch.json?.r || []).length === 3, batch.json);
const opt = cli('image.op', { in: shots[0], out: join(ip, 'small.jpg'), op: 'optimize', args: { kb: 60 } }, { root });
check('optimize hit the byte target', (opt.json?.bytes || 1e9) <= 60 * 1024, opt.json);
const pal = cli('image.palette', { path: shots[1], n: 5 }, { root });
check('palette returned real hexes', (pal.json?.colors || pal.json?.palette || []).length >= 3, pal.json);

// ---------------------------------------------------------------- manifest
const files = [];
const collect = (dir) => {
  if (!existsSync(dir)) return;
  for (const e of readdirSync(dir)) {
    const p = join(dir, e);
    if (statSync(p).isDirectory() && e !== 'assets' && e !== 'versions') collect(p);
    else if (/\.(png|jpg|webp|svg|pdf|json)$/i.test(e) && !p.includes('fixtures')) {
      const rel = p.replace(ROOT + '\\', '').replace(/\\/g, '/');
      // Only pixels are hashable across runs: the project JSON carries ids and clocks.
      const img = /\.(png|jpe?g|webp)$/i.test(e);
      files.push({ path: rel, bytes: statSync(p).size, dims: imageDims(p) || undefined, sha: img ? sha256(p) : undefined });
    }
  }
};
collect(OUT);
// Determinism: the same spec through the same engine must produce the same pixels.
// Node ids and timestamps live in the project JSON, which is deliberately excluded.
const hashed = golden.filter((g) => g.sha);
const produced = files.filter((f) => f.sha);
const drift = hashed
  .map((g) => {
    const now = files.find((f) => f.path === g.path);
    return now && now.sha === g.sha ? null : { path: g.path, was: g.sha, now: now ? now.sha : 'missing' };
  })
  .filter(Boolean);
// A pixel file this run made but the golden record never saw is drift of another
// kind: the artifact was renamed or invented, so nothing vouches for it.
const unseen = produced.filter((f) => !hashed.some((g) => g.path === f.path)).map((f) => f.path);
// The very first run has no record to compare against. It is reported as the
// baseline, and the next run is graded against it.
const baseline = hashed.length === 0 && !hadGolden;
check(
  'regenerated images match the previous golden record',
  baseline || (hashed.length > 0 && produced.length > 0 && drift.length === 0 && unseen.length === 0),
  baseline
    ? { compared: 0, note: 'no previous golden record; this run is the baseline' }
    : { compared: hashed.length, produced: produced.length, drift: drift.slice(0, 3), new_files: unseen.slice(0, 3) },
);
writeFileSync(join(OUT, 'MANIFEST.json'), JSON.stringify({ generated_at: new Date().toISOString(), engine: cli('version', {}, { root }).json, files }, null, 2));
console.log(`  ${files.length} example artifacts recorded in examples/MANIFEST.json`);

const ok = summary('examples');
process.exit(ok ? 0 : 1);
