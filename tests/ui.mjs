// Desktop smoke: the packaged app boots, serves the same engine over HTTP, and
// the shipped UI bundle really is frameless with its own window controls.
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, readFileSync, mkdirSync, statSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { ROOT, sandbox, check, section, summary, eq } from './lib.mjs';

const APP = process.env.AGENTCANVAS_APP || join(ROOT, 'dist', 'AgentCanvas.exe');
const shots = join(ROOT, 'shots');
mkdirSync(shots, { recursive: true });

section('desktop smoke — packaged app + shipped UI');
if (!existsSync(APP)) {
  console.log(`  FAIL ${APP} not built (node scripts/build.mjs)`);
  process.exit(1);
}

const root = sandbox('ui');
const proc = spawn(APP, [], { cwd: ROOT, env: { ...process.env, AGENTCANVAS_HOME: root }, stdio: 'ignore', detached: false });
let port = 0;
const t0 = Date.now();
while (Date.now() - t0 < 30_000 && !port) {
  for (const p of range(8899, 8909)) {
    try {
      const r = await fetch(`http://127.0.0.1:${p}/health`, { signal: AbortSignal.timeout(700) });
      const j = await r.json();
      // Only accept the app we just launched; a server from an earlier run would
      // otherwise answer for a different workspace root.
      if (j?.ok && j?.pid === proc.pid) {
        port = p;
        break;
      }
    } catch {}
  }
  if (!port) await sleep(400);
}
check('the desktop app brings up the same HTTP engine', port > 0, `waited ${((Date.now() - t0) / 1000).toFixed(1)}s`);

if (port) {
  const caps = await (await fetch(`http://127.0.0.1:${port}/rpc`, { method: 'POST', body: JSON.stringify({ m: 'system.caps' }) })).json();
  check('caps answers from inside the app', caps?.name === 'AgentCanvas', caps?.e || caps?.code);
  const made = await (await fetch(`http://127.0.0.1:${port}/rpc`, { method: 'POST', body: JSON.stringify({ m: 'project.create', p: { name: 'from-window', w: 1080, h: 1440 } }) })).json();
  check('a project can be created through the running app', !!made?.p, made);
  // The CLI is a native exe: spawn it directly. Handing it to `process.execPath`
  // makes node try to *interpret* the binary, which fails and looks like a missing
  // project — the state was fine all along.
  const seen = spawnSync(join(ROOT, 'dist', 'bin', 'agentcanvas.exe'), ['project.get', `p=${made.p}`, '--root', root], { encoding: 'utf8' });
  const headless = JSON.parse(seen.stdout || '{}');
  eq('the headless CLI sees the same project the app made', headless.p, made.p);
  const again = spawnSync(join(ROOT, 'dist', 'bin', 'agentcanvas.exe'), ['canvas.add', `p=${made.p}`, 'b=b1', 't=rect', 'w=40', 'h=40', '--root', root], { encoding: 'utf8' });
  const added = JSON.parse(again.stdout || '{}');
  check('and an edit made by the CLI is visible to the running app', added.ok === true, added);
  const live = await (await fetch(`http://127.0.0.1:${port}/rpc`, { method: 'POST', body: JSON.stringify({ m: 'project.get', p: { p: made.p, depth: 0 } }) })).json();
  eq('the app reports the revision the CLI wrote', live?.rev, added.rev);
}

// The shipped UI bundle must really be frameless: the drag region and the three
// window controls are compiled into the JS, not provided by the OS.
// Vite emits the styles as a separate .css asset, so that counts as the bundle too.
const dist = join(ROOT, 'ui', 'dist');
const assets = existsSync(dist) ? walk(dist).filter((f) => /\.(js|css|html)$/.test(f)) : [];
check('the frontend was built into ui/dist', assets.length > 0, dist);
const bundle = assets.map((f) => readFileSync(f, 'utf8')).join('\n');
check('the bundle declares a drag region', bundle.includes('data-tauri-drag-region'));
for (const label of ['最小化', '关闭']) {
  check(`the bundle draws its own "${label}" control`, bundle.includes(label));
}
check('the bundle talks to the window API', /toggleMaximize|internalToggleMaximize|isMaximized/.test(bundle));
check('the glass material is real CSS, not a flat fill', /backdrop-filter/.test(bundle) && /blur\(/.test(bundle));
check('the material saturates what it blurs', /saturate\(/.test(bundle));

// Render the shipped bundle the same way the window does, and look at it.
// `npx.cmd` cannot be spawned without a shell on Windows (EINVAL), so vite's own
// entry point is run through node — same server, no shim.
const preview = spawn(process.execPath, [join(ROOT, 'ui', 'node_modules', 'vite', 'bin', 'vite.js'), 'preview', '--port', '4173', '--strictPort'], { cwd: join(ROOT, 'ui'), stdio: 'ignore' });
await sleep(3500);
const shot = (name, hash) => {
  const out = join(shots, name);
  const r = spawnSync('C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe', [
    '--headless=new', '--disable-gpu', '--hide-scrollbars', '--window-size=1600,1000',
    '--virtual-time-budget=6000', `--screenshot=${out}`, `http://127.0.0.1:4173/${hash}`,
  ], { encoding: 'utf8' });
  const okShot = existsSync(out) && statSync(out).size > 20_000;
  check(`screenshot ${name}`, okShot, { size: existsSync(out) ? statSync(out).size : 0, err: r.stderr?.slice(0, 200) });
  return out;
};
shot('shipped-home.png', '');
shot('shipped-studio.png', '#/studio');
shot('shipped-imagelab.png', '#/image');
try {
  preview.kill();
} catch {}

// A GUI app ignores SIGTERM on Windows, so a plain `kill()` leaves it holding its
// HTTP port for whatever suite runs next.
killTree(proc.pid);
check('the app exits when the smoke test ends', !alive(proc.pid), `pid ${proc.pid} still running`);
const ok = summary('ui-smoke');
console.log(ok ? '\nDESKTOP SMOKE: PASS' : '\nDESKTOP SMOKE: FAIL');
process.exit(ok ? 0 : 1);

function range(a, b) {
  return Array.from({ length: b - a }, (_, i) => a + i);
}
function killTree(pid) {
  spawnSync('taskkill', ['/PID', String(pid), '/T', '/F'], { stdio: 'ignore' });
}
function alive(pid) {
  const r = spawnSync('tasklist', ['/FI', `PID eq ${pid}`, '/NH'], { encoding: 'utf8' });
  return String(r.stdout || '').includes(String(pid));
}
function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}
function walk(dir) {
  const out = [];
  for (const e of readdirSync(dir)) {
    const p = join(dir, e);
    if (statSync(p).isDirectory()) out.push(...walk(p));
    else out.push(p);
  }
  return out;
}
