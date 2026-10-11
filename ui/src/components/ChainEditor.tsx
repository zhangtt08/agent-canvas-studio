/** The chain editor of 图片处理: draggable glass step chips, one argument row per
 *  op, and an add-menu fed by `caps.image_ops`. */

import { useState } from 'react';
import type { V } from '../types';
import { OP_ICON, OP_LABEL, argsOf, chainString, defaultsOf } from '../lib/chain';
import { Badge, Btn, ColorBtn, GlassMenu, IconBtn, NumVal, Select, TextInput, type MenuSpec } from './Primitives';
import { Icon } from './Icons';

export interface Step {
  op: string;
  args: V;
}

export function ChainEditor({
  chain,
  ops,
  active,
  onChange,
  onPick,
  onRun,
  busy,
  presets,
  onPreset,
}: {
  chain: Step[];
  ops: string[];
  active: number | null;
  onChange: (c: Step[]) => void;
  onPick: (i: number | null) => void;
  onRun: () => void;
  busy: boolean;
  presets: { name: string; d: string }[];
  onPreset: (name: string) => void;
}) {
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const [drag, setDrag] = useState<number | null>(null);

  const add = (op: string) => {
    const step: Step = { op, args: defaultsOf(op) };
    const at = active === null ? chain.length : active + 1;
    onChange([...chain.slice(0, at), step, ...chain.slice(at)]);
    onPick(at);
  };

  const drop = (i: number) => {
    if (drag === null || drag === i) return;
    const out = [...chain];
    const [moved] = out.splice(drag, 1);
    out.splice(i, 0, moved);
    onChange(out);
    setDrag(null);
  };

  const items: MenuSpec[] = ops.map((op) => ({
    label: `${OP_LABEL[op] ?? op} · ${op}`,
    icon: OP_ICON[op] ?? 'plus',
    onClick: () => add(op),
  }));

  const sel = active === null ? null : chain[active];
  const args = sel ? argsOf(sel.op) : [];

  return (
    <div className="stack" style={{ gap: 12 }}>
      <div className="chain glass glass-inset" data-sheen>
        {chain.length ? (
          chain.map((s, i) => (
            <span key={`${s.op}-${i}`} style={{ display: 'contents' }}>
              {i > 0 ? (
                <span className="arrow">
                  <Icon name="chevron" size={15} />
                </span>
              ) : null}
              <div
                className={`step glass glass-2 ${active === i ? 'is-on' : ''}`}
                draggable
                onDragStart={() => setDrag(i)}
                onDragOver={(e) => e.preventDefault()}
                onDrop={() => drop(i)}
                onClick={() => onPick(active === i ? null : i)}
                data-tip="拖动可换顺序 · 点击展开参数"
                data-sheen
              >
                <span className="step-i">
                  <Icon name={OP_ICON[s.op] ?? 'sliders'} size={14} sw={1.9} />
                </span>
                <span style={{ display: 'grid', gap: 1 }}>
                  <span style={{ fontSize: 13, fontWeight: 700, letterSpacing: '-0.01em' }}>{OP_LABEL[s.op] ?? s.op}</span>
                  <span className="step-n t3">{s.op}</span>
                </span>
                <button
                  type="button"
                  className="chip-x"
                  aria-label="移除这一步"
                  onClick={(e) => {
                    e.stopPropagation();
                    onChange(chain.filter((_, k) => k !== i));
                    onPick(null);
                  }}
                >
                  <Icon name="close" size={11} sw={2} />
                </button>
              </div>
            </span>
          ))
        ) : (
          <div className="row" style={{ gap: 10, color: 'var(--t2)' }}>
            <Icon name="crop" size={18} />
            <span style={{ fontSize: 13 }}>链条是空的</span>
            <Btn
              size="sm"
              icon="plus"
              onClick={() => add('crop')}
            >
              加第一步
            </Btn>
          </div>
        )}
        <span style={{ marginLeft: 'auto' }} />
        <button
          type="button"
          className="btn btn-sm"
          data-tip="从引擎支持的算子里添加"
          onClick={(e) => {
            const r = e.currentTarget.getBoundingClientRect();
            setMenu({ x: r.left, y: r.bottom + 6 });
          }}
        >
          <Icon name="plus" size={14} />
          添加步骤
        </button>
      </div>

      <div className="mono" style={{ fontSize: 12.5, color: 'var(--t2)', padding: '0 2px' }}>
        {chainString(chain) || '—'}
      </div>

      {sel ? (
        <div className="stack-6" style={{ padding: '10px 12px', borderRadius: 14, background: 'rgba(255,255,255,.4)', border: '1px solid rgba(255,255,255,.6)' }}>
          <div className="row" style={{ gap: 8 }}>
            <Badge tone="blue">{sel.op}</Badge>
            <span className="h3">{OP_LABEL[sel.op] ?? sel.op}</span>
            <span className="spacer" />
            <IconBtn icon="up" size="sm" tip="往前挪" label="前移" disabled={active === 0} onClick={() => active !== null && drop2(chain, active, active - 1, onChange)} />
            <IconBtn icon="down" size="sm" tip="往后挪" label="后移" disabled={active === chain.length - 1} onClick={() => active !== null && drop2(chain, active, active + 1, onChange)} />
          </div>
          {args.length ? (
            <div className="box4" style={{ gridTemplateColumns: 'repeat(auto-fit, minmax(120px, 1fr))' }}>
              {args.map((a) => {
                const v = sel.args[a.k];
                const set = (next: string | number) =>
                  onChange(chain.map((s, i) => (i === active ? { ...s, args: { ...s.args, [a.k]: next } } : s)));
                if (a.kind === 'num')
                  return (
                    <div key={a.k} className="field">
                      <label>{a.label}</label>
                      <NumVal tag="#" value={typeof v === 'number' ? v : Number(a.def)} min={a.min} max={a.max} step={a.step ?? 1} dp={a.dp ?? 0} onChange={set} />
                    </div>
                  );
                if (a.kind === 'color')
                  return (
                    <div key={a.k} className="field">
                      <label>{a.label}</label>
                      <div className="row" style={{ gap: 6 }}>
                        <ColorBtn color={String(v ?? a.def)} onChange={(c) => set(c)} />
                        <TextInput value={String(v ?? a.def)} onChange={(x) => set(x)} mono />
                      </div>
                    </div>
                  );
                if (a.kind === 'select')
                  return (
                    <div key={a.k} className="field">
                      <label>{a.label}</label>
                      <Select value={String(v ?? a.def)} options={a.options ?? []} onChange={(x) => set(x)} ariaLabel={a.label} />
                    </div>
                  );
                return (
                  <div key={a.k} className="field">
                    <label>{a.label}</label>
                    <TextInput value={String(v ?? a.def)} onChange={(x) => set(x)} tip={a.label} />
                  </div>
                );
              })}
            </div>
          ) : (
            <div className="row" style={{ gap: 8 }}>
              <span style={{ fontSize: 13, color: 'var(--t2)' }}>这一步不需要参数</span>
              <Btn size="sm" variant="quiet" onClick={() => onPick(null)}>
                收起
              </Btn>
            </div>
          )}
        </div>
      ) : null}

      <div className="row" style={{ gap: 8, flexWrap: 'wrap' }}>
        <Btn variant="primary" icon="bolt" disabled={busy || !chain.length} onClick={onRun}>
          {busy ? '处理中' : '运行链条'}
        </Btn>
        <Btn variant="quiet" icon="trash" disabled={!chain.length} onClick={() => { onChange([]); onPick(null); }}>
          清空
        </Btn>
        <span className="spacer" />
        <span style={{ fontSize: 13, color: 'var(--t2)' }}>预设</span>
        <div className="row" style={{ gap: 6, flexWrap: 'wrap' }}>
          {presets.map((p) => (
            <Btn key={p.name} size="sm" onClick={() => onPreset(p.name)} tip={p.d}>
              {p.name}
            </Btn>
          ))}
        </div>
      </div>

      {menu ? <GlassMenu x={menu.x} y={menu.y} items={items} onClose={() => setMenu(null)} /> : null}
    </div>
  );
}

function drop2(chain: Step[], from: number, to: number, onChange: (c: Step[]) => void) {
  if (to < 0 || to >= chain.length) return;
  const out = [...chain];
  const [m] = out.splice(from, 1);
  out.splice(to, 0, m);
  onChange(out);
}
