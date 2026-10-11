// The single gate: `node tests/run-all.mjs`. Every suite must be green before
// delivery, and a suite that cannot run counts as a failure — not as skipped.
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { ROOT, BIN } from './lib.mjs';

const only = process.argv.slice(2);
// The JS suites all drive the *release* binary. If it is stale every one of them
// fails against code that no longer exists, so build it as step 0 and put mingw on
// PATH the way scripts/build.mjs does.
const PATH_EXTRA = ['C:\\Users\\EDY\\tools\\mingw64\\bin', join(process.env.HOME || process.env.USERPROFILE || '', '.cargo', 'bin')];
const env = { ...process.env, PATH: [process.env.PATH, ...PATH_EXTRA].join(';') };
const suites = [
  { name: 'suite imports', cmd: process.execPath, args: ['tools/check-imports.mjs'], needsBin: false },
  { name: 'release build (engine + cli)', cmd: 'cargo', args: ['build', '--release', '-p', 'agentcanvas-cli'], needsBin: false },
  { name: 'cargo test', cmd: 'cargo', args: ['test', '--workspace'], needsBin: false },
  { name: 'scenario B (MCP headless)', cmd: process.execPath, args: ['tests/mcp-e2e.mjs'], needsBin: true },
  { name: 'scenario E (HTTP + CLI)', cmd: process.execPath, args: ['tests/http-e2e.mjs'], needsBin: true },
  { name: 'scenarios A/C/D (acceptance)', cmd: process.execPath, args: ['tests/acceptance.mjs'], needsBin: true },
  // Spawned through node rather than `npm run`: on Windows `spawnSync('npm.cmd')`
  // fails with EINVAL unless the whole call goes through a shell.
  { name: 'ui typecheck (tsc)', cmd: process.execPath, args: ['node_modules/typescript/bin/tsc', '--noEmit'], cwd: join(ROOT, 'ui'), needsBin: false },
  { name: 'ui build (vite)', cmd: process.execPath, args: ['node_modules/vite/bin/vite.js', 'build'], cwd: join(ROOT, 'ui'), needsBin: false },
  { name: 'desktop smoke (frameless window)', cmd: process.execPath, args: ['tests/ui.mjs'], needsBin: true },
  { name: 'shipped copy is current', cmd: process.execPath, args: ['tools/check-dist.mjs'], needsBin: false },
].filter((s) => !only.length || only.some((o) => s.name.includes(o)));

const results = [];
for (const s of suites) {
  if (s.needsBin && !existsSync(BIN)) {
    results.push({ name: s.name, ok: false, note: `binary missing: ${BIN} — run scripts/build.mjs first` });
    console.log(`\n#### ${s.name}\n  SKIP->FAIL: binary missing (${BIN})`);
    continue;
  }
  console.log(`\n#### ${s.name}`);
  const r = spawnSync(s.cmd, s.args, { cwd: s.cwd || ROOT, stdio: 'inherit', shell: false, env });
  results.push({ name: s.name, ok: r.status === 0, note: r.error ? String(r.error.message || r.error) : `exit ${r.status}` });
}

console.log('\n================ GATE ================');
for (const r of results) console.log(`${r.ok ? 'PASS' : 'FAIL'}  ${r.name}   ${r.ok ? '' : r.note}`);
const bad = results.filter((r) => !r.ok).length;
console.log(`\n${results.length - bad}/${results.length} suites green`);
process.exit(bad ? 1 : 0);
