// A suite that crashes halfway reports fewer checks, not a failure — so a helper
// that was never imported silently removes every assertion after it.
// `node tools/check-imports.mjs` fails on that class of bug.
import { readFileSync, readdirSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const lib = readFileSync(join(ROOT, 'tests', 'lib.mjs'), 'utf8');
const exports = [...lib.matchAll(/^export (?:async )?(?:function|const|class) (\w+)/gm)].map((m) => m[1]);

const files = readdirSync(join(ROOT, 'tests'))
  .filter((f) => f.endsWith('.mjs') && f !== 'lib.mjs');

let bad = 0;
for (const f of files) {
  const src = readFileSync(join(ROOT, 'tests', f), 'utf8');
  const importLine = (src.match(/import \{([^}]*)\} from '\.\/lib\.mjs';/) || [, ''])[1];
  const imported = new Set(importLine.split(',').map((s) => s.trim().split(' as ').pop()).filter(Boolean));
  for (const name of exports) {
    const used = new RegExp(`(?<![\\w.$])${name}\\(`).test(src);
    if (used && !imported.has(name)) {
      console.log(`FAIL ${f}: uses ${name}() without importing it`);
      bad++;
    }
  }
}
console.log(bad ? `\n${bad} missing import(s)` : `imports ok (${exports.length} helpers, ${files.length} suites)`);
process.exit(bad ? 1 : 0);
