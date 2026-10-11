// One command that produces the shippable thing:
//   node scripts/build.mjs            release CLI + UI + desktop exe (bundled installer if possible)
//   node scripts/build.mjs --debug    faster, no optimisation
//   node scripts/build.mjs --bundle   also run `tauri build` for the NSIS/MSI installer
import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync, existsSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolveRoot();
const release = !process.argv.includes('--debug');
const wantBundle = process.argv.includes('--bundle');
const profile = release ? 'release' : 'debug';
const PATH_EXTRA = ['C:\\Users\\EDY\\tools\\mingw64\\bin', join(process.env.HOME || process.env.USERPROFILE || '', '.cargo', 'bin')];
const env = { ...process.env, PATH: [process.env.PATH, ...PATH_EXTRA].join(';') };

function resolveRoot() {
  return join(dirname(fileURLToPath(import.meta.url)), '..');
}

function run(cmd, args, opts = {}) {
  console.log(`\n$ ${cmd} ${args.join(' ')}`);
  // npm/npx are .cmd shims: spawning them without a shell throws EINVAL on Windows.
  const r = spawnSync(cmd, args, { cwd: opts.cwd || ROOT, stdio: 'inherit', env, shell: !!opts.shell });
  if (r.status !== 0) {
    console.error(`FAILED: ${cmd} ${args.join(' ')} (${r.error?.message || 'exit ' + r.status})`);
    process.exit(1);
  }
  return r;
}

function findBuilt(name) {
  const cands = [
    join(ROOT, 'target', profile, `${name}.exe`),
    join(ROOT, 'target', 'x86_64-pc-windows-gnu', profile, `${name}.exe`),
    join(ROOT, 'target', 'x86_64-pc-windows-msvc', profile, `${name}.exe`),
  ];
  return cands.find((p) => existsSync(p));
}

// 1. engine + headless binary
run('cargo', ['build', '-p', 'agentcanvas-cli', ...(release ? ['--release'] : [])]);

// 2. frontend
run('npm.cmd', ['install'], { cwd: join(ROOT, 'ui'), shell: true });
run('npm.cmd', ['run', 'build'], { cwd: join(ROOT, 'ui'), shell: true });

// 3. desktop binary (needs the built frontend because tauri embeds frontendDist)
run('cargo', ['build', '-p', 'agentcanvas-app', ...(release ? ['--release'] : [])]);

// 4. staged dist/
const bin = findBuilt('agentcanvas');
const app = findBuilt('agentcanvas-app');
const outBin = join(ROOT, 'dist', 'bin');
const outApp = join(ROOT, 'dist');
mkdirSync(outBin, { recursive: true });
if (bin) copyFileSync(bin, join(outBin, 'agentcanvas.exe'));
if (app) copyFileSync(app, join(outApp, 'AgentCanvas.exe'));
// A copied Tauri exe cannot find WebView2Loader.dll on its own; ship it next to it.
for (const dll of ['WebView2Loader.dll']) {
  const src = [join(ROOT, 'target', profile, dll), join(ROOT, 'target', 'x86_64-pc-windows-gnu', profile, dll)].find((p) => existsSync(p));
  if (src) {
    copyFileSync(src, join(outApp, dll));
    copyFileSync(src, join(outBin, dll));
  }
}
const manifest = {
  built_at: new Date().toISOString(),
  profile,
  files: [bin, app].filter(Boolean).map((p) => ({ name: p.split(/[\\/]/).pop(), bytes: statSync(p).size })),
};
writeFileSync(join(ROOT, 'dist', 'BUILD.json'), JSON.stringify(manifest, null, 2));

if (wantBundle) {
  run('npm.cmd', ['install'], { cwd: ROOT, shell: true });
  run('npx.cmd', ['tauri', 'build'], { cwd: join(ROOT, 'app'), shell: true });
}

console.log('\nbuilt:');
for (const p of [join(outBin, 'agentcanvas.exe'), join(outApp, 'AgentCanvas.exe')]) {
  console.log(`  ${existsSync(p) ? 'ok  ' : 'MISS'} ${p}`);
}
