// The shipped folder must not be an old snapshot of the tree. `tests/run-all.mjs`
// grades the build output; this grades `dist/`, which is what the user actually runs.
// `node tools/check-dist.mjs`
import { readdirSync, statSync, existsSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const norm = (p) => p.split('\\').join('/');

function walk(dir) {
  if (!existsSync(dir)) return [];
  const out = [];
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) {
      if (['node_modules', 'target', 'dist', '.git', '.scratch'].includes(e.name)) continue;
      out.push(...walk(p));
    } else out.push(p);
  }
  return out;
}

const mtime = (p) => (existsSync(p) ? statSync(p).mtimeMs : 0);
const newest = (dir, re) => walk(dir).filter((f) => re.test(norm(f))).reduce((m, f) => Math.max(m, mtime(f)), 0);

const engine = Math.max(newest(join(ROOT, 'core'), /\.rs$/), newest(join(ROOT, 'cli'), /\.rs$/), mtime(join(ROOT, 'Cargo.toml')));
const uiSrc = Math.max(newest(join(ROOT, 'ui', 'src'), /\.(ts|tsx|css|html)$/), mtime(join(ROOT, 'ui', 'index.html')));

const rows = [];
const need = (label, path, floor, floorLabel) => {
  const t = mtime(path);
  rows.push({
    ok: t > 0 && t >= floor,
    label,
    detail: t === 0 ? `missing: ${path}` : `built ${new Date(t).toISOString().slice(11, 19)}, ${floorLabel} last changed ${new Date(floor).toISOString().slice(11, 19)}`,
  });
};
/** A vendored runtime DLL keeps its own timestamp, so only its presence is a guarantee. */
const present = (label, path) => {
  const ok = existsSync(path);
  rows.push({ ok, label, detail: ok ? '' : `missing: ${path}` });
};

need('dist/bin/agentcanvas.exe', join(ROOT, 'dist', 'bin', 'agentcanvas.exe'), engine, 'engine+cli sources');
need('dist/AgentCanvas.exe', join(ROOT, 'dist', 'AgentCanvas.exe'), Math.max(engine, newest(join(ROOT, 'app'), /\.rs$/), uiSrc), 'engine, app and UI sources');
present('WebView2Loader.dll beside the app', join(ROOT, 'dist', 'WebView2Loader.dll'));
need('ui/dist bundle', join(ROOT, 'ui', 'dist', 'assets'), uiSrc, 'ui/src');

let bad = 0;
for (const r of rows) {
  console.log(`  ${r.ok ? 'ok  ' : 'FAIL'} ${r.label}  ${r.ok ? '' : r.detail}`);
  if (!r.ok) bad++;
}
console.log(bad ? `\n${bad} artifact(s) stale or missing — run node scripts/build.mjs` : `\ndist is current (${rows.length} checks)`);
process.exit(bad ? 1 : 0);
