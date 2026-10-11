// Regenerates the "真实返回示例" block in docs/api-reference.md from replies the
// shipping binary actually produced, so the doc can't drift from the engine.
//   node tools/api-capture.mjs
import { spawnSync } from 'node:child_process';
import { mkdirSync, rmSync, writeFileSync, readFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { ROOT } from '../tests/lib.mjs';

const EXE = join(ROOT, 'dist', 'bin', 'agentcanvas.exe');
if (!existsSync(EXE)) {
  console.error('no dist/bin/agentcanvas.exe — run node scripts/build.mjs first');
  process.exit(1);
}
const HOME = join(ROOT, '.scratch', 'api');
rmSync(HOME, { recursive: true, force: true });
mkdirSync(join(HOME, 'out'), { recursive: true });
const SHOT = join(ROOT, 'examples', 'fixtures', 'product-a.png');
if (!existsSync(SHOT)) {
  console.error('no examples/fixtures/product-a.png — run node examples/run-all.mjs first');
  process.exit(1);
}

const cases = [
  ['一次调用出图', ['poster', '--root', HOME, 'id=doc-poster', 'name=无界面出片', 'title=夏日轻乳', 'subtitle=生牛乳发酵',
    'body=冷链直送，只有三种配料。', `image=${SHOT}`, 'accent=#4B6BFB', 'preset=poster', 'f=png', `out=${join(HOME, 'out')}`]],
  ['工程摘要 + fields 裁剪', ['project.get', '--root', HOME, 'p=doc-poster', 'depth=0', 'fields=p,name,rev,boards']],
  ['排版后的第一个节点', ['project.get', '--root', HOME, 'p=doc-poster', 'depth=1', 'fields=p,nodes']],
  ['体检', ['inspect.validate', '--root', HOME, 'p=doc-poster']],
  ['文字度量', ['text.measure', '--root', HOME, 'text=夏日轻乳 生牛乳发酵', 'family=Inter', 'size=72', 'max_w=520']],
  ['位图链（字符串形式）', ['image.chain', '--root', HOME, `in=${SHOT}`, `out=${join(HOME, 'out', 'chain.webp')}`,
    'chain=crop 1:1 > scale 0.6 > sharpen 1 > watermark AgentCanvas', 'q=82']],
  ['一个来回跑多个调用', ['batch', '--root', HOME, 'jobs=[{"m":"version","p":{}},{"m":"system.health","p":{}}]']],
  ['错误：方法名写错', ['nope.method', '--root', HOME]],
  ['错误：写到 root 外面', ['render.export', '--root', HOME, 'p=doc-poster', 'o=C:/Users/you/Desktop/x.png']],
];

const runs = cases.map(([label, args]) => {
  const r = spawnSync(EXE, args, { encoding: 'utf8', windowsHide: true });
  const raw = ((r.stdout || '') + (r.stdout ? '' : r.stderr || '')).trim();
  let one = raw;
  try {
    one = JSON.stringify(JSON.parse(raw));
  } catch {
    /* keep the raw line */
  }
  // The command line is shown with placeholders: the reply below is the real stdout of
  // exactly this call, but a 90-character absolute --root makes every example unreadable.
  const shown = args
    .map((a) => a.replaceAll(HOME, '<root>').replaceAll(SHOT, '<img>').replaceAll(join(ROOT, 'examples', 'fixtures'), '<fixtures>').replace(/\\/g, '/'))
    .map((a) => (a.includes(' ') || a.includes('=') ? `"${a.replace(/"/g, '&quot;')}"` : a))
    .join(' ');
  return { label, cmd: `agentcanvas ${shown}`, bytes: one.length, body: one };
});

const ver = JSON.parse(spawnSync(EXE, ['version', '--root', HOME], { encoding: 'utf8' }).stdout.trim());
const lines = [];
lines.push('<!-- api:begin -->');
lines.push(`以下每一段都是 \`agentcanvas\` ${ver.ver}（schema ${ver.schema}）在 Windows 上的真实 stdout，`);
lines.push('由 `node tools/api-capture.mjs` 生成，不是手写示例。重跑即可刷新。');
lines.push('');
for (const r of runs) {
  lines.push(`**${r.label}** — 返回 ${r.bytes} 字节`);
  lines.push('');
  lines.push('```powershell');
  lines.push(r.cmd);
  lines.push('```');
  lines.push('');
  lines.push('```json');
  lines.push(r.body);
  lines.push('```');
  lines.push('');
}
lines.push('<!-- api:end -->');
const block = lines.join('\n');

const DOC = join(ROOT, 'docs', 'api-reference.md');
const cur = readFileSync(DOC, 'utf8');
const next = cur.includes('<!-- api:begin -->')
  ? cur.replace(/<!-- api:begin -->[\s\S]*<!-- api:end -->/, block)
  : `${cur.trimEnd()}\n\n## 真实返回示例\n\n${block}\n`;
writeFileSync(DOC, next);
console.log(`docs/api-reference.md updated: ${runs.length} captured replies, ${block.length} chars of block`);
