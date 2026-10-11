// Generates docs/schema.md and docs/tool-reference.md from the running engine,
// so the published contract can never drift from what the binary actually accepts.
//   node tools/gen-docs.mjs [path-to-agentcanvas.exe]
import { spawnSync } from 'node:child_process';
import { writeFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';

const ROOT = process.cwd();
const BIN =
  process.argv[2] ||
  [
    join(ROOT, 'dist', 'bin', 'agentcanvas.exe'),
    join(ROOT, 'target', 'x86_64-pc-windows-gnu', 'release', 'agentcanvas.exe'),
    join(ROOT, 'target', 'x86_64-pc-windows-gnu', 'debug', 'agentcanvas.exe'),
  ].find((p) => existsSync(p));
if (!BIN) {
  console.error('no agentcanvas binary; build it first');
  process.exit(1);
}

const call = (m, p = {}) => {
  const r = spawnSync(BIN, ['rpc', JSON.stringify({ m, p })], { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
  try {
    return JSON.parse((r.stdout || '').trim());
  } catch {
    console.error(`caps/tools call failed: ${r.stdout?.slice(0, 200)} ${r.stderr?.slice(0, 200)}`);
    process.exit(1);
  }
};

const caps = call('system.caps');
const tools = call('system.tools');
const ver = call('system.version');
const skeleton = call('system.schema');

const lines = [];
const push = (s = '') => lines.push(s);

push('# 工具参考（自动生成）');
push('');
push('> 由 `node tools/gen-docs.mjs` 从当前构建的引擎生成，不要手改。');
push(`> 引擎 ${ver.ver} · schema ${ver.schema} · ${tools.length} 个 MCP 工具 · ${caps.methods.length} 个方法`);
push('');
push('MCP 客户端看到的是下面这些**聚合后的工具**；每个工具都转发到同一个 `rpc::call`，');
push('所以工具清单不需要为了覆盖率而膨胀。任何未列出的方法都可以通过 `agentcanvas`');
push('这个通用工具访问：`{"name":"agentcanvas","arguments":{"m":"layout.explain","p":{…}}}`。');
push('');
push('| 工具 | 必填参数 | 说明 |');
push('| --- | --- | --- |');
for (const t of tools) {
  const req = (t.inputSchema?.required || []).join(', ') || '—';
  const desc = (t.description || '').replace(/\|/g, '\\|');
  push(`| \`${t.name}\` | ${req} | ${desc} |`);
}
push('');
push('## 全部方法');
push('');
push('```');
for (const m of caps.methods) push(m);
push('```');
push('');
push('## 图片处理 op');
push('');
push('`image.op {op}` / `image.chain` 接受的名字：');
push('');
push('```');
push((caps.image_ops || []).join(' '));
push('```');
push('');
push('预设：`' + (caps.image_presets || []).join('`, `') + '`');
push('');
push('## 语义布局 intent');
push('');
push('```');
push((caps.intents || []).join(' '));
push('```');
push('');
push('## 尺寸预设');
push('');
push('| 名称 | 像素 | 说明 |');
push('| --- | --- | --- |');
for (const [k, w, h, d] of caps.presets || []) push(`| \`${k}\` | ${w}×${h} | ${d} |`);
push('');
push('## 内置模板');
push('');
push('```');
push((caps.templates || []).join(' '));
push('```');
push('');
push('## 短键');
push('');
push('| 键 | 含义 |');
push('| --- | --- |');
for (const [k, v] of Object.entries(caps.keys || {})) push(`| \`${k}\` | ${v} |`);
push('');
push('## 限额');
push('');
push('```json');
push(JSON.stringify(caps.limits, null, 2));
push('```');
writeFileSync(join(ROOT, 'docs', 'tool-reference.md'), lines.join('\n') + '\n');

// --- schema.md -------------------------------------------------------------
const s = [];
const w = (x = '') => s.push(x);
w('# 文档 Schema');
w('');
w('> 由 `node tools/gen-docs.mjs` 从引擎的 `doc.rs` 类型生成。');
w(`> schema 版本 **${caps.schema}**，引擎 ${caps.ver}。`);
w('');
w('一个工程就是一份 `Document`：');
w('');
w('```json');
w(JSON.stringify(skeleton.doc, null, 2));
w('```');
w('');
w('约定：');
w('');
w('- 坐标是 **CSS 像素**，原点在画板左上角，y 向下；`rotation` 是角度、顺时针、绕 `origin`。');
w('- `nodes[].box` 是**求解结果**，`nodes[].layout.intent` 是**作者意图**，两者分开存。');
w('- `layers` 与 `children` 的索引 0 是**最底层**。');
w('- 颜色支持 `#rgb` `#rgba` `#rrggbb` `#rrggbbaa` `rgb()` `rgba()` 和 `$token`。');
w('- `layout.mode` ∈ `manual` `constraint` `flex` `grid`；`manual` 或 `pinned:true` 的节点不参与自动排版。');
w('- 未知字段会被拒绝（不是忽略），这样拼写错误会立刻暴露而不是变成"设置没生效"。');
w('');
w('## 节点类型');
w('');
w('`' + (skeleton.node_types || caps.node_types || []).join('`, `') + '`');
w('');
w('## 命令');
w('');
w('每一次修改都是一个命令（`canvas.batch` 一次提交多条，一个事务）：');
w('');
w('```json');
w(JSON.stringify(skeleton.cmds, null, 1));
w('```');
w('');
w('## 错误码');
w('');
w('| code | 含义 |');
w('| --- | --- |');
for (const [c, d] of skeleton.codes || []) w(`| \`${c}\` | ${d} |`);
w('');
w('## 迁移');
w('');
w('`schema_version` 是语义化版本。读入时：');
w('');
w('- 同 minor 直接读；');
w('- 低版本走 `doc::migrate`（读旧写新，不丢字段）；');
w('- 高版本拒绝打开并返回 `INVALID_SCHEMA` + 引擎版本，避免旧引擎写坏新工程。');
writeFileSync(join(ROOT, 'docs', 'schema.md'), s.join('\n') + '\n');

console.log(`docs/tool-reference.md: ${tools.length} tools, ${caps.methods.length} methods`);
console.log(`docs/schema.md: schema ${caps.schema}`);
