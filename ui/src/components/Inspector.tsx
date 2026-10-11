/** Right dock: 样式 / 布局 / 检查. Every control here dispatches the same
 *  `canvas.*` / `layout.*` / `inspect.*` methods an agent calls. */

import { useCallback, useMemo, useState, type ReactNode } from 'react';
import { useApp } from '../state/app';
import type { Effects, Intent, Issue, Node, Paint, Radius, Shadow, Stop, V } from '../types';
import { findBoard, isUniform, nodeMap } from '../lib/doc';
import { ADDABLE_INTENTS, LAYOUT_MODES, intentRows, rowPatch, type IRow } from '../lib/intent';
import { BLENDS, BOX_MODES, OVERFLOWS, TYPE_LABEL, WEIGHTS, WEIGHT_LABEL } from '../lib/doc';
import { round } from '../lib/format';
import { Badge, Btn, Chip, ColorBtn, Empty, Field, GlassMenu, IconBtn, NumVal, Range, Seg, Select, Switch, TextInput, type MenuSpec } from './Primitives';
import { Icon } from './Icons';

const ALIGN_X = [
  { v: 'left', label: '左', icon: 'alignL' as const },
  { v: 'center', label: '中', icon: 'alignC' as const },
  { v: 'right', label: '右', icon: 'alignR' as const },
];
const ALIGN_Y = [
  { v: 'top', label: '上', icon: 'alignT' as const },
  { v: 'middle', label: '中', icon: 'alignM' as const },
  { v: 'bottom', label: '下', icon: 'alignB' as const },
];

function Group({ title, right, children }: { title: string; right?: ReactNode; children: ReactNode }) {
  return (
    <div className="grp">
      <div className="grp-h">
        <span>{title}</span>
        {right}
      </div>
      {children}
    </div>
  );
}

/* --------------------------------------------------------- gradient editor */

function GradientEditor({ paint, onChange }: { paint: Paint; onChange: (p: Paint) => void }) {
  const [sel, setSel] = useState(0);
  const [bar, setBar] = useState<HTMLDivElement | null>(null);
  if (paint.t !== 'linear' && paint.t !== 'radial') return null;
  const stops = paint.stops;
  const isRadial = paint.t === 'radial';

  const commitStops = (list: Stop[]) => {
    const sorted = [...list].sort((a, b) => a.at - b.at);
    onChange(isRadial ? { t: 'radial', cx: paint.cx, cy: paint.cy, r: paint.r, stops: sorted } : { t: 'linear', angle: paint.angle, stops: sorted });
  };

  const atOf = (clientX: number) => {
    if (!bar) return 0;
    const r = bar.getBoundingClientRect();
    return round(Math.max(0, Math.min(1, (clientX - r.left) / Math.max(1, r.width))), 3);
  };

  const dragStop = (i: number, e: React.PointerEvent) => {
    e.stopPropagation();
    const move = (ev: PointerEvent) => commitStops(stops.map((s, k) => (k === i ? { ...s, at: atOf(ev.clientX) } : s)));
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };

  const first = stops[0]?.at ?? 0;
  const span = (stops[stops.length - 1]?.at ?? 1) - first || 1;
  const css = `linear-gradient(90deg, ${stops.map((s) => `${s.color} ${(((s.at - first) / span) * 100).toFixed(1)}%`).join(', ')})`;

  return (
    <div className="stack-6">
      <div
        ref={setBar}
        className="gbar"
        style={{ backgroundImage: css }}
        data-tip="点击色条加一个色标 · 拖动色标改位置"
        onPointerDown={(e) => {
          commitStops([...stops, { at: atOf(e.clientX), color: '#4B6BFB' }]);
          setSel(stops.length);
        }}
      >
        {stops.map((s, i) => (
          <button
            key={i}
            type="button"
            className={`stop ${i === sel ? 'is-on' : ''}`}
            style={{ left: `${((s.at - first) / span) * 100}%`, background: s.color }}
            aria-label={`色标 ${i + 1}`}
            data-tip="拖动改变位置"
            onPointerDown={(e) => {
              setSel(i);
              dragStop(i, e);
            }}
          />
        ))}
      </div>
      <div className="row" style={{ gap: 8 }}>
        <ColorBtn color={stops[sel]?.color ?? '#000000'} onChange={(c) => commitStops(stops.map((s, i) => (i === sel ? { ...s, color: c } : s)))} />
        <TextInput value={stops[sel]?.color ?? '#000000'} onChange={(v) => commitStops(stops.map((s, i) => (i === sel ? { ...s, color: v } : s)))} mono tip="色标颜色" />
        <IconBtn
          icon="trash"
          size="sm"
          danger
          disabled={stops.length < 3}
          tip={stops.length < 3 ? '至少保留两个色标' : '删除这个色标'}
          label="删除色标"
          onClick={() => {
            commitStops(stops.filter((_, i) => i !== sel));
            setSel(Math.max(0, sel - 1));
          }}
        />
      </div>
      {isRadial ? (
        <div className="box4">
          <NumVal tag="CX" value={paint.cx} min={0} max={1} step={0.02} dp={2} onChange={(v) => onChange({ ...paint, cx: v })} />
          <NumVal tag="CY" value={paint.cy} min={0} max={1} step={0.02} dp={2} onChange={(v) => onChange({ ...paint, cy: v })} />
          <NumVal tag="R" value={paint.r} min={0.05} max={2} step={0.02} dp={2} onChange={(v) => onChange({ ...paint, r: v })} />
        </div>
      ) : (
        <div className="row" style={{ gap: 8 }}>
          <NumVal tag="∠" value={paint.angle} min={0} max={360} step={5} dp={0} suffix="度" onChange={(v) => onChange({ ...paint, angle: v })} />
          <div style={{ flex: 1 }}>
            <Range value={paint.angle} min={0} max={360} step={5} onChange={(v) => onChange({ ...paint, angle: v })} tip="渐变方向" />
          </div>
        </div>
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ effects */

function EffectsEditor({ fx, onFx }: { fx: Effects; onFx: (fx: Effects) => void }) {
  const shadows = fx.shadows ?? [];
  const upd = (list: Shadow[]) => onFx({ ...fx, shadows: list });
  return (
    <div className="stack-6">
      {shadows.map((s, i) => (
        <div key={i} className="glass glass-inset" style={{ padding: 8, borderRadius: 12 }}>
          <div className="row" style={{ gap: 6, marginBottom: 6 }}>
            <span className="badge" style={{ marginRight: 'auto' }}>
              投影 {i + 1}
            </span>
            <ColorBtn
              color={s.color.slice(0, 7)}
              onChange={(c) => upd(shadows.map((x, k) => (k === i ? { ...x, color: `${c}${s.color.length > 7 ? s.color.slice(7, 9) : '4d'}` } : x)))}
            />
            <button type="button" className="li-x danger" aria-label="删除这个投影" data-tip="删除" onClick={() => upd(shadows.filter((_, k) => k !== i))}>
              <Icon name="trash" size={13} />
            </button>
          </div>
          <div className="box4">
            <NumVal tag="X" value={s.x} min={-200} max={200} dp={0} onChange={(v) => upd(shadows.map((x, k) => (k === i ? { ...x, x: v } : x)))} />
            <NumVal tag="Y" value={s.y} min={-200} max={200} dp={0} onChange={(v) => upd(shadows.map((x, k) => (k === i ? { ...x, y: v } : x)))} />
            <NumVal tag="虚化" value={s.blur} min={0} max={200} dp={0} onChange={(v) => upd(shadows.map((x, k) => (k === i ? { ...x, blur: v } : x)))} />
            <NumVal tag="扩散" value={s.spread ?? 0} min={-100} max={100} dp={0} onChange={(v) => upd(shadows.map((x, k) => (k === i ? { ...x, spread: v } : x)))} />
          </div>
        </div>
      ))}
      <div className="row" style={{ gap: 6 }}>
        <Btn size="sm" icon="plus" onClick={() => upd([...shadows, { x: 0, y: 18, blur: 32, color: '#171d3f40' }])}>
          加投影
        </Btn>
        {shadows.length ? (
          <Btn size="sm" variant="quiet" onClick={() => onFx({ ...fx, shadows: [] })}>
            清空
          </Btn>
        ) : null}
      </div>
      <div className="box4" style={{ marginTop: 4 }}>
        <NumVal tag="模糊" value={fx.blur ?? 0} min={0} max={120} step={0.5} dp={1} onChange={(v) => onFx({ ...fx, blur: v })} />
        <NumVal tag="底纹" value={fx.backdrop_blur ?? 0} min={0} max={80} step={0.5} dp={1} onChange={(v) => onFx({ ...fx, backdrop_blur: v })} />
        <NumVal tag="饱和" value={fx.saturate ?? 1} min={0} max={3} step={0.05} dp={2} onChange={(v) => onFx({ ...fx, saturate: v })} />
        <NumVal tag="颗粒" value={fx.noise ?? 0} min={0} max={1} step={0.02} dp={2} onChange={(v) => onFx({ ...fx, noise: v })} />
      </div>
    </div>
  );
}

/* -------------------------------------------------------------------- style */

function StyleTab({ node }: { node: Node }) {
  const { state, mutate, toast } = useApp();
  const doc = state.doc!;
  const many = state.sel.length > 1;

  /* Style fields ride `canvas.style`; multi-select has to use `canvas.batch`, whose
     command tags are the PascalCase `commands::Cmd` variants and whose `Style`
     command takes a whole `style` object rather than spread fields. */
  const stylePatch = useCallback(
    (patch: V) => {
      if (many) {
        const style = patch.fill || patch.stroke || patch.radius || patch.color
          ? { fill: patch.fill, stroke: patch.stroke, radius: patch.radius, color: patch.color }
          : null;
        const rest = { ...patch };
        delete rest.fill;
        delete rest.stroke;
        delete rest.radius;
        delete rest.color;
        delete rest.fx;
        void mutate('canvas.batch', {
          cmds: state.sel.flatMap((id) => {
            const out: V[] = [];
            if (style) out.push({ t: 'Style', id, style });
            if (Object.keys(rest).length) out.push({ t: 'NodeSet', id, patch: rest });
            return out;
          }),
        }).catch(() => undefined);
        return;
      }
      void mutate('canvas.style', { id: node.id, ...patch }).catch(() => undefined);
    },
    [many, mutate, node.id, state.sel],
  );

  /* visibility and lock are node flags, not style — `canvas.set` is the documented
     door for them (`canvas.style` forwards them anyway, but `set` is unambiguous). */
  const nodePatch = useCallback(
    (patch: V) => {
      void mutate('canvas.set', { ids: state.sel.length ? state.sel : [node.id], patch }).catch(() => undefined);
    },
    [mutate, node.id, state.sel],
  );

  const patchNode = useCallback(
    (patch: V) => {
      void mutate('canvas.set', { ids: state.sel, patch }).catch(() => undefined);
    },
    [mutate, state.sel],
  );

  const setText = (p: V) => patchNode({ text: { ...(node.text ?? { content: '' }), ...p } });
  const setImage = (p: V) => patchNode({ image: { ...(node.image ?? { asset: '', fit: 'cover' }), ...p } });

  const setBox = (k: 'x' | 'y' | 'w' | 'h', v: number) => {
    const box = { ...node.box, [k]: v };
    const cmds = state.sel.map((id) => ({ t: 'Transform', id, x: k === 'x' ? v : node.box.x, y: k === 'y' ? v : node.box.y, w: k === 'w' ? v : node.box.w, h: k === 'h' ? v : node.box.h }));
    void (many ? mutate('canvas.batch', { cmds }) : mutate('canvas.transform', { id: node.id, x: box.x, y: box.y, w: box.w, h: box.h })).catch(() => undefined);
  };

  const fill: Paint = node.style?.fill ?? { t: 'none' };
  const radius: Radius = node.style?.radius ?? { all: 0 };
  const stroke = node.style?.stroke;
  const text = node.text;
  const image = node.image;
  const board = findBoard(doc, node.artboard ?? state.board);

  return (
    <>
      <Group
        title={many ? `${state.sel.length} 个图层` : TYPE_LABEL[node.type]}
        right={
          <span className="mono" style={{ fontSize: 12, color: 'var(--t3)' }}>
            {node.id}
          </span>
        }
      >
        <div className="box4">
          <NumVal tag="X" value={round(node.box.x, 0)} dp={0} onChange={(v) => setBox('x', v)} />
          <NumVal tag="Y" value={round(node.box.y, 0)} dp={0} onChange={(v) => setBox('y', v)} />
          <NumVal tag="W" value={round(node.box.w, 0)} min={1} dp={0} onChange={(v) => setBox('w', v)} />
          <NumVal tag="H" value={round(node.box.h, 0)} min={0} dp={0} onChange={(v) => setBox('h', v)} />
          <NumVal tag="∠" value={node.rotation} min={-360} max={360} step={1} dp={1} suffix="度" onChange={(v) => void mutate('canvas.transform', { id: node.id, rot: v }).catch(() => undefined)} />
          <NumVal tag="A" value={round(node.opacity * 100, 0)} min={0} max={100} dp={0} suffix="%" onChange={(v) => stylePatch({ opacity: round(v / 100, 3) })} />
        </div>
        <div className="row" style={{ marginTop: 8, gap: 6 }}>
          <IconBtn
            icon="swap"
            size="sm"
            tip="水平翻转"
            label="水平翻转"
            active={node.flip_x}
            onClick={() => void mutate('canvas.transform', { id: node.id, x: node.box.x, y: node.box.y, w: node.box.w, h: node.box.h, flip_x: !node.flip_x }).catch(() => undefined)}
          />
          <IconBtn
            icon="swap"
            size="sm"
            tip="垂直翻转"
            label="垂直翻转"
            active={node.flip_y}
            onClick={() => void mutate('canvas.transform', { id: node.id, x: node.box.x, y: node.box.y, w: node.box.w, h: node.box.h, flip_y: !node.flip_y }).catch(() => undefined)}
          />
          <IconBtn icon={node.visible ? 'eye' : 'eyeOff'} size="sm" tip={node.visible ? '隐藏图层' : '显示图层'} label="显示" onClick={() => nodePatch({ visible: !node.visible })} />
          <IconBtn icon={node.locked ? 'lock' : 'unlock'} size="sm" tip={node.locked ? '解锁图层' : '锁定图层'} label="锁定" onClick={() => nodePatch({ locked: !node.locked })} />
          <div style={{ flex: '1 1 auto', minWidth: 96 }}>
            <Select
              value={node.blend ?? 'normal'}
              options={BLENDS.map((b) => ({ v: b, label: b }))}
              onChange={(v) => stylePatch({ blend: v === 'normal' ? null : v })}
              tip="混合模式"
              ariaLabel="混合模式"
            />
          </div>
        </div>
      </Group>

      <Group title="对齐与分布">
        <div className="row" style={{ gap: 4, flexWrap: 'wrap' }}>
          {ALIGN_X.map((a) => (
            <IconBtn key={a.v} icon={a.icon} size="sm" tip={`水平${a.label}对齐`} label={a.v} onClick={() => void mutate('canvas.align', { ids: state.sel, axis: 'x', how: a.v }).catch(() => undefined)} />
          ))}
          <span style={{ width: 6 }} />
          {ALIGN_Y.map((a) => (
            <IconBtn key={a.v} icon={a.icon} size="sm" tip={`垂直${a.label}对齐`} label={a.v} onClick={() => void mutate('canvas.align', { ids: state.sel, axis: 'y', how: a.v }).catch(() => undefined)} />
          ))}
          <span style={{ width: 6 }} />
          <IconBtn icon="sliders" size="sm" tip="水平等距分布（需 3 个以上）" label="水平分布" disabled={state.sel.length < 3} onClick={() => void mutate('canvas.distribute', { ids: state.sel, axis: 'x' }).catch(() => undefined)} />
          <IconBtn icon="sliders" size="sm" tip="垂直等距分布（需 3 个以上）" label="垂直分布" disabled={state.sel.length < 3} onClick={() => void mutate('canvas.distribute', { ids: state.sel, axis: 'y' }).catch(() => undefined)} />
        </div>
      </Group>

      <Group
        title="填充"
        right={
          <Seg
            tight
            value={fill.t === 'solid' ? 'solid' : fill.t === 'linear' || fill.t === 'radial' ? 'grad' : fill.t === 'image' ? 'image' : 'none'}
            onChange={(v) => {
              if (v === 'none') stylePatch({ fill: { t: 'none' } });
              else if (v === 'solid') stylePatch({ fill: { t: 'solid', color: '#4B6BFB' } });
              else if (v === 'grad') stylePatch({ fill: { t: 'linear', angle: 135, stops: [{ at: 0, color: '#4B6BFB' }, { at: 1, color: '#8B5CF6' }] } });
              else {
                const a = doc.assets[0];
                if (!a) {
                  toast('info', '先在素材面板导入一张图');
                  return;
                }
                stylePatch({ fill: { t: 'image', asset: a.id, fit: 'cover', opacity: 1 } });
              }
            }}
            options={[
              { v: 'none', label: '无' },
              { v: 'solid', label: '纯色' },
              { v: 'grad', label: '渐变' },
              { v: 'image', label: '图' },
            ]}
          />
        }
      >
        {fill.t === 'solid' ? (
          <div className="row" style={{ gap: 8 }}>
            <ColorBtn color={fill.color} onChange={(c) => stylePatch({ fill: { t: 'solid', color: c } })} />
            <TextInput value={fill.color} onChange={(v) => stylePatch({ fill: { t: 'solid', color: v } })} mono tip="#RRGGBB 或 #RRGGBBAA" />
          </div>
        ) : fill.t === 'linear' || fill.t === 'radial' ? (
          <>
            <div className="row" style={{ marginBottom: 8, gap: 6 }}>
              <Chip dim active={fill.t === 'linear'} onClick={() => stylePatch({ fill: { t: 'linear', angle: fill.t === 'linear' ? fill.angle : 90, stops: fill.stops } })}>
                线性
              </Chip>
              <Chip dim active={fill.t === 'radial'} onClick={() => stylePatch({ fill: fill.t === 'radial' ? fill : { t: 'radial', cx: 0.5, cy: 0.5, r: 0.7, stops: fill.stops } })}>
                径向
              </Chip>
            </div>
            <GradientEditor paint={fill} onChange={(p) => stylePatch({ fill: p })} />
          </>
        ) : fill.t === 'image' ? (
          <div className="stack-6">
            <Select
              value={fill.asset}
              options={doc.assets.map((a) => ({ v: a.id, label: a.name }))}
              onChange={(v) => stylePatch({ fill: { ...fill, asset: v } })}
              tip="填充用的素材"
              ariaLabel="素材"
            />
            <Seg
              tight
              value={fill.fit}
              onChange={(v) => stylePatch({ fill: { ...fill, fit: v } })}
              options={[
                { v: 'contain', label: '完整' },
                { v: 'cover', label: '铺满' },
                { v: 'fill', label: '拉伸' },
                { v: 'actual', label: '原始' },
              ]}
            />
            <div className="row" style={{ gap: 8 }}>
              <span className="t3" style={{ fontSize: 13 }}>图片不透明度</span>
              <NumVal tag="A" value={fill.opacity} min={0} max={1} step={0.05} dp={2} onChange={(v) => stylePatch({ fill: { ...fill, opacity: v } })} />
            </div>
          </div>
        ) : (
          <div className="row" style={{ gap: 8 }}>
            <span className="t3" style={{ fontSize: 13, flex: 1 }}>无填充</span>
            <Btn size="sm" icon="plus" onClick={() => stylePatch({ fill: { t: 'solid', color: '#F6F7FB' } })}>
              加一层底色
            </Btn>
          </div>
        )}
      </Group>

      <Group title="描边">
        <div className="row" style={{ gap: 8 }}>
          <ColorBtn
            color={stroke?.color ?? '#171D3F'}
            onChange={(c) => stylePatch({ stroke: { color: c, width: stroke?.width ?? 2, ...(stroke?.align ? { align: stroke.align } : {}) } })}
          />
          <NumVal
            tag="粗"
            value={stroke?.width ?? 0}
            min={0}
            max={60}
            step={0.5}
            dp={1}
            onChange={(v) => stylePatch({ stroke: v === 0 ? null : { color: stroke?.color ?? '#171D3F', width: v } })}
          />
          <div style={{ flex: 1 }}>
            <Select
              value={stroke?.align ?? 'center'}
              options={[
                { v: 'inside', label: '内侧' },
                { v: 'center', label: '居中' },
                { v: 'outside', label: '外侧' },
              ]}
              onChange={(v) => stylePatch({ stroke: { color: stroke?.color ?? '#171D3F', width: stroke?.width ?? 2, align: v } })}
              tip="描边对齐方式"
              ariaLabel="描边对齐"
            />
          </div>
        </div>
        <div className="row" style={{ gap: 6, marginTop: 8 }}>
          {[
            { l: '实线', d: [] },
            { l: '虚线', d: [6, 4] },
            { l: '点线', d: [1, 4] },
            { l: '长短', d: [10, 4, 2, 4] },
          ].map((o) => (
            <Chip
              key={o.l}
              dim
              active={(stroke?.dash ?? []).join(',') === o.d.join(',')}
              onClick={() => stylePatch({ stroke: { color: stroke?.color ?? '#171D3F', width: stroke?.width ?? 2, dash: o.d } })}
            >
              {o.l}
            </Chip>
          ))}
        </div>
      </Group>

      {node.type === 'rect' || node.type === 'ellipse' || node.type === 'image' || node.type === 'frame' ? (
        <Group
          title="圆角"
          right={
            <IconBtn
              icon="settings"
              size="sm"
              tip={isUniform(radius) ? '四个角分别设置' : '合并四个角'}
              label="四角独立"
              active={!isUniform(radius)}
              onClick={() => stylePatch({ radius: isUniform(radius) ? { all: radius.all, tl: radius.all, tr: radius.all, br: radius.all, bl: radius.all } : { all: radius.all } })}
            />
          }
        >
          {isUniform(radius) ? (
            <div className="row" style={{ gap: 8 }}>
              <NumVal tag="R" value={radius.all} min={0} max={4000} dp={0} onChange={(v) => stylePatch({ radius: { all: v } })} />
              <div style={{ flex: 1 }}>
                <Range
                  value={radius.all}
                  min={0}
                  max={Math.max(100, Math.round(Math.min(node.box.w, node.box.h) / 2))}
                  onChange={(v) => stylePatch({ radius: { all: v } })}
                  tip="圆角半径"
                />
              </div>
            </div>
          ) : (
            <div className="box4">
              {(['tl', 'tr', 'br', 'bl'] as const).map((k) => (
                <NumVal key={k} tag={k.toUpperCase()} value={radius[k] ?? radius.all} min={0} max={4000} dp={0} onChange={(v) => stylePatch({ radius: { ...radius, [k]: v } })} />
              ))}
            </div>
          )}
        </Group>
      ) : null}

      {text ? (
        <Group title="文字">
          <div className="stack-6">
            <Field label="字体">
              <Select
                value={text.family ?? 'PingFang SC'}
                options={(state.fonts.length ? state.fonts : [text.family ?? 'PingFang SC']).map((f) => ({ v: f, label: f }))}
                onChange={(v) => setText({ family: v })}
                tip="清单来自 text.fonts · 预览与导出用同一份字体文件"
                ariaLabel="字体"
              />
            </Field>
            <div className="box4">
              <NumVal tag="字号" value={text.size ?? 16} min={1} max={900} dp={0} onChange={(v) => setText({ size: v })} />
              <NumVal tag="字距" value={text.letter_spacing ?? 0} min={-40} max={100} step={0.1} dp={1} onChange={(v) => setText({ letter_spacing: v })} />
              <NumVal tag="行高" value={text.line_ratio ?? 1.35} min={0.5} max={4} step={0.05} dp={2} onChange={(v) => setText({ line_ratio: v })} />
              <NumVal tag="内缩" value={text.padding ?? 0} min={0} max={400} dp={0} onChange={(v) => setText({ padding: v })} />
            </div>
            <Field label="字重">
              <Select
                value={String(text.weight ?? 400)}
                options={WEIGHTS.map((w) => ({ v: String(w), label: `${w} · ${WEIGHT_LABEL[w]}` }))}
                onChange={(v) => setText({ weight: Number(v) })}
                tip="实际可用字重取决于字体文件"
                ariaLabel="字重"
              />
            </Field>
            <div className="row" style={{ gap: 6 }}>
              <Seg
                tight
                value={text.align ?? 'left'}
                onChange={(v) => setText({ align: v })}
                options={[
                  { v: 'left', icon: 'alignL', tip: '左对齐' },
                  { v: 'center', icon: 'alignC', tip: '居中' },
                  { v: 'right', icon: 'alignR', tip: '右对齐' },
                ]}
              />
              <Seg
                tight
                value={text.valign ?? 'top'}
                onChange={(v) => setText({ valign: v })}
                options={[
                  { v: 'top', label: '上', tip: '顶部对齐' },
                  { v: 'middle', label: '中', tip: '垂直居中' },
                  { v: 'bottom', label: '下', tip: '底部对齐' },
                ]}
              />
            </div>
            <div className="box4">
              <Select
                value={text.overflow ?? 'clip'}
                options={OVERFLOWS.map((o) => ({ v: o.v, label: o.label }))}
                onChange={(v) => setText({ overflow: v })}
                tip="文字超出文本框时怎么处理"
                ariaLabel="溢出"
              />
              <Select
                value={text.box_mode ?? 'auto-height'}
                options={BOX_MODES.map((o) => ({ v: o.v, label: o.label }))}
                onChange={(v) => setText({ box_mode: v })}
                tip="文本框尺寸模式"
                ariaLabel="框模式"
              />
            </div>
            <div className="row" style={{ gap: 8 }}>
              <ColorBtn color={text.color ?? '#171D3F'} onChange={(c) => setText({ color: c })} tip="文字颜色" />
              <TextInput value={text.content} onChange={(v) => void mutate('canvas.text', { id: node.id, content: v }).catch(() => undefined)} tip="内容 · 画布上双击可直接编辑" />
            </div>
            <div className="sw-row">
              <span>全部大写</span>
              <Switch on={Boolean(text.uppercase)} onChange={(v) => setText({ uppercase: v })} />
            </div>
            <div className="sw-row">
              <span>斜体</span>
              <Switch on={Boolean(text.style_italic)} onChange={(v) => setText({ style_italic: v })} />
            </div>
          </div>
        </Group>
      ) : null}

      {image ? (
        <Group title="图片">
          <div className="stack-6">
            <Seg
              tight
              value={image.fit}
              onChange={(v) => void mutate('canvas.image', { id: node.id, asset: image.asset, fit: v }).catch(() => undefined)}
              options={[
                { v: 'contain', label: '完整' },
                { v: 'cover', label: '铺满' },
                { v: 'fill', label: '拉伸' },
                { v: 'actual', label: '原始' },
              ]}
            />
            <Field label="裁切（按素材像素）">
              <div className="box4">
                {(['x', 'y', 'w', 'h'] as const).map((k) => (
                  <NumVal
                    key={k}
                    tag={k.toUpperCase()}
                    value={image.crop?.[k] ?? 0}
                    min={0}
                    dp={0}
                    onChange={(v) => setImage({ crop: { ...(image.crop ?? { x: 0, y: 0, w: 0, h: 0 }), [k]: v } })}
                  />
                ))}
              </div>
            </Field>
            <div className="box4">
              <NumVal tag="亮度" value={image.brightness ?? 0} min={-1} max={1} step={0.02} dp={2} onChange={(v) => setImage({ brightness: v })} />
              <NumVal tag="对比" value={image.contrast ?? 1} min={0} max={3} step={0.02} dp={2} onChange={(v) => setImage({ contrast: v })} />
              <NumVal tag="饱和" value={image.saturate ?? 1} min={0} max={3} step={0.02} dp={2} onChange={(v) => setImage({ saturate: v })} />
              <NumVal tag="虚化" value={image.blur ?? 0} min={0} max={60} step={0.5} dp={1} onChange={(v) => setImage({ blur: v })} />
            </div>
            <div className="row" style={{ gap: 8 }}>
              <ColorBtn color={image.tint ?? '#4B6BFB'} onChange={(c) => setImage({ tint: c })} tip="染色" />
              <Select
                value={image.asset}
                options={doc.assets.map((a) => ({ v: a.id, label: a.name }))}
                onChange={(v) => void mutate('canvas.image', { id: node.id, asset: v, fit: image.fit }).catch(() => undefined)}
                tip="换一张图"
                ariaLabel="素材"
              />
            </div>
            <div className="sw-row">
              <span>去色</span>
              <Switch on={Boolean(image.grayscale)} onChange={(v) => setImage({ grayscale: v })} />
            </div>
            <div className="row" style={{ gap: 6 }}>
              <Btn size="sm" icon="crop" onClick={() => setImage({ crop: { x: 0, y: 0, w: 0, h: 0 } })} tip="清掉裁切框">
                清除裁切
              </Btn>
              <Btn size="sm" variant="quiet" onClick={() => patchNode({ image: { ...image, brightness: null, contrast: null, saturate: null, blur: null, tint: null, grayscale: null } })}>
                还原调整
              </Btn>
            </div>
          </div>
        </Group>
      ) : null}

      <Group title="效果">
        <EffectsEditor fx={node.fx ?? {}} onFx={(fx) => stylePatch({ fx })} />
      </Group>

      <Group title="语义角色">
        <Select
          value={node.z_role ?? 'normal'}
          options={[
            { v: 'normal', label: '普通' },
            { v: 'background', label: '背景' },
            { v: 'decor', label: '装饰' },
            { v: 'content', label: '内容' },
            { v: 'label', label: '标签' },
          ]}
          onChange={(v) => stylePatch({ z_role: v === 'normal' ? null : v })}
          tip="检查面板按角色判断重叠是否算问题"
          ariaLabel="角色"
        />
        {board ? (
          <div className="row" style={{ gap: 6, marginTop: 8 }}>
            <Btn size="sm" variant="quiet" icon="front" onClick={() => void mutate('canvas.reorder', { id: node.id, to: board.layers.length, b: board.id }).catch(() => undefined)}>
              置顶
            </Btn>
            <Btn size="sm" variant="quiet" icon="back" onClick={() => void mutate('canvas.reorder', { id: node.id, to: 0, b: board.id }).catch(() => undefined)}>
              置底
            </Btn>
            <Btn size="sm" variant="quiet" icon="copy" onClick={() => void mutate('canvas.duplicate', { ids: [node.id] }).catch(() => undefined)}>
              复制
            </Btn>
          </div>
        ) : null}
      </Group>
    </>
  );
}

/* ------------------------------------------------------------------- layout */

function LayoutTab({ node }: { node: Node }) {
  const { state, mutate, openSheet, toast } = useApp();
  const doc = state.doc!;
  const [addMenu, setAddMenu] = useState<{ x: number; y: number } | null>(null);
  const [conflicts, setConflicts] = useState<{ n: string; i: string; why: string }[]>([]);
  const intent = node.layout.intent ?? {};
  const rows = useMemo(() => intentRows(intent, doc.nodes), [intent, doc.nodes]);

  const applyIntent = (next: Intent, mode: string = node.layout.mode === 'manual' ? 'constraint' : node.layout.mode) =>
    void mutate('layout.apply', { id: node.id, intent: next, mode }).catch(() => undefined);

  const editRow = (row: IRow, patch: { ref?: string; num?: number; num2?: number; text?: string; on?: boolean }) => {
    const p = rowPatch(row, patch);
    applyIntent({ ...intent, [p.key]: p.value } as Intent);
  };

  const dropRow = (row: IRow) => {
    const next = { ...intent } as V;
    delete next[row.key as string];
    applyIntent(next as Intent);
  };

  const addItems: MenuSpec[] = ADDABLE_INTENTS.filter((a) => !(intent as V)[a.key as string]).map((a) => ({
    label: a.label,
    icon: 'plus' as const,
    onClick: () => applyIntent({ ...intent, [a.key]: a.seed } as Intent),
  }));

  const nodeOptions = doc.nodes.filter((n) => n.id !== node.id).map((n) => ({ v: n.id, label: n.name }));
  const board = findBoard(doc, node.artboard ?? state.board);

  return (
    <>
      <Group title="布局模式">
        <Seg
          grow
          value={node.layout.mode}
          options={LAYOUT_MODES.map((m) => ({
            v: m.v,
            label: m.label,
            tip: m.v === 'manual' ? '坐标写死，重排不会动它' : m.v === 'constraint' ? '位置由语义意图求解' : m.v === 'flex' ? '作为弹性容器排列子图层' : '作为网格容器排列子图层',
          }))}
          onChange={(v) => void mutate('layout.apply', { id: node.id, intent, mode: v }).catch(() => undefined)}
        />
        <div className="sw-row" style={{ marginTop: 10 }}>
          <span>钉住位置（不参与求解）</span>
          <Switch on={Boolean(node.layout.pinned)} onChange={(v) => void mutate('layout.lock', { id: node.id, pinned: v }).catch(() => undefined)} />
        </div>
      </Group>

      <Group
        title="语义意图"
        right={
          <span className="row" style={{ gap: 4 }}>
            <IconBtn icon="question" size="sm" tip="为什么在这里 · 意图生效顺序" label="解释" onClick={() => openSheet('explain', { id: node.id })} />
            <button
              type="button"
              className="btn btn-sm btn-icon"
              data-tip="添加一条意图"
              aria-label="添加意图"
              onClick={(e) => {
                const r = e.currentTarget.getBoundingClientRect();
                setAddMenu({ x: r.left - 120, y: r.bottom + 6 });
              }}
            >
              <Icon name="plus" size={14} />
            </button>
          </span>
        }
      >
        {rows.length ? (
          <div className="stack-6">
            {rows.map((r) => (
              <div key={String(r.key)} className="glass glass-inset" style={{ borderRadius: 12, padding: '8px 9px' }}>
                <div className="row" style={{ gap: 6 }}>
                  <span className="badge badge-blue mono" style={{ fontSize: 12 }}>
                    {String(r.key)}
                  </span>
                  <span className="trunc" style={{ flex: '1 1 auto', fontSize: 13, fontWeight: 600 }}>
                    {r.value}
                  </span>
                  <button type="button" className="li-x danger" aria-label="移除这条意图" data-tip="移除" onClick={() => dropRow(r)}>
                    <Icon name="close" size={12} sw={2} />
                  </button>
                </div>
                <div className="row" style={{ gap: 6, marginTop: 7 }}>
                  {r.kind === 'ref' ? (
                    <div style={{ flex: '1 1 auto', minWidth: 0 }}>
                      <Select value={r.ref ?? ''} options={[{ v: '', label: '选择参照图层' }, ...nodeOptions]} onChange={(v) => editRow(r, { ref: v })} tip="参照图层" ariaLabel="参照图层" />
                    </div>
                  ) : null}
                  {r.kind === 'enum' && r.options ? (
                    <div style={{ flex: '1 1 auto', minWidth: 0 }}>
                      <Select value={r.text ?? ''} options={r.options.map((o) => ({ v: o.v, label: o.label }))} onChange={(v) => editRow(r, { text: v })} tip={r.label} ariaLabel={r.label} />
                    </div>
                  ) : null}
                  {r.kind === 'num' || r.kind === 'size' ? (
                    <>
                      <NumVal tag={r.key === 'size' ? 'W' : '值'} value={r.num} dp={0} onChange={(v) => editRow(r, { num: v })} />
                      {r.kind === 'size' ? <NumVal tag="H" value={r.num2} dp={0} onChange={(v) => editRow(r, { num2: v })} /> : null}
                    </>
                  ) : null}
                  {r.kind === 'bool' ? <Switch on={Boolean(r.num)} onChange={(v) => editRow(r, { on: v, num: v ? 1 : 0 })} tip={r.label} /> : null}
                  {r.kind === 'text' ? (
                    <div style={{ flex: '1 1 auto' }}>
                      <TextInput value={r.text ?? ''} onChange={(v) => editRow(r, { text: v })} tip={r.label} />
                    </div>
                  ) : null}
                </div>
              </div>
            ))}
          </div>
        ) : (
          <Empty
            title="这个图层还没有意图"
            action={
              <Btn
                variant="primary"
                icon="plus"
                onClick={() =>
                  applyIntent({
                    anchor: 'top-left',
                    margin_top: Math.round(node.box.y),
                    margin_left: Math.round(node.box.x),
                    size: { w_ref: round(node.box.w / Math.max(1, board?.width ?? node.box.w), 3) },
                  })
                }
              >
                按现在的位置生成
              </Btn>
            }
          />
        )}
        {conflicts.length ? (
          <div className="stack-6" style={{ marginTop: 10 }}>
            {conflicts.map((c, i) => (
              <div key={i} className="issue sev-err">
                <i className="issue-b" />
                <div className="issue-t">
                  <b>{c.i}</b>
                  <span>{c.why}</span>
                </div>
              </div>
            ))}
          </div>
        ) : null}
      </Group>

      <Group title="整板求解">
        <div className="row" style={{ gap: 6, flexWrap: 'wrap' }}>
          <Btn
            size="sm"
            icon="wand"
            onClick={() =>
              void mutate<{ conflicts?: { n: string; i: string; why: string }[] }>('layout.resolve', { b: state.board })
                .then((r) => {
                  const cs = r?.conflicts ?? [];
                  setConflicts(cs);
                  toast(cs.length ? 'info' : 'ok', cs.length ? `${cs.length} 处意图冲突` : '本板已按意图重排');
                })
                .catch(() => undefined)
            }
          >
            求解本板
          </Btn>
          <Btn size="sm" variant="quiet" icon="refresh" onClick={() => void mutate('layout.resolve', {}).catch(() => undefined)}>
            求解全部
          </Btn>
          <Btn
            size="sm"
            variant="quiet"
            icon="sliders"
            disabled={state.sel.length < 3}
            tip="选中 3 个以上图层可用"
            onClick={() =>
              void mutate('canvas.batch', {
                cmds: [{ t: 'LayoutIntent', id: state.sel[0], intent: { distribute: state.sel, equal_gap: { axis: 'y', gap: 40 } }, mode: 'constraint' }],
              }).catch(() => undefined)
            }
          >
            等距分布
          </Btn>
        </div>
      </Group>

      {addMenu ? <GlassMenu x={addMenu.x} y={addMenu.y} items={addItems.length ? addItems : [{ label: '所有意图都已挂上', icon: 'check', onClick: () => undefined }]} onClose={() => setAddMenu(null)} /> : null}
    </>
  );
}

/* ------------------------------------------------------------------ inspect */

function InspectTab() {
  const { state, mutate, read, select, dispatch, toast } = useApp();
  const doc = state.doc!;
  const [busy, setBusy] = useState(false);
  const issues = state.issues;

  const run = useCallback(
    (board: string | null) => {
      setBusy(true);
      /* `inspect.validate` is a read: it takes `board` (not `b`) to scope itself
         and must not ride `mutate()`, which would stamp a rev it never returns. */
      void read<{ issues?: Issue[]; errors?: number; warns?: number }>(
        'inspect.validate',
        { ...(board ? { board } : {}), contrast: 1, lowres: 1 },
      )
        .then((r) => {
          const list = r?.issues ?? [];
          dispatch({ t: 'issues', issues: list, board });
          const errs = r?.errors ?? 0;
          toast(errs ? 'err' : list.length ? 'info' : 'ok', errs ? `${errs} 个错误 · ${list.length} 条` : list.length ? `发现 ${list.length} 个问题` : '没有发现问题');
        })
        .finally(() => setBusy(false));
    },
    [dispatch, read, toast],
  );

  return (
    <>
      <Group title="质量检查">
        <div className="row" style={{ gap: 6, marginBottom: 10, flexWrap: 'wrap' }}>
          <Btn size="sm" variant="primary" icon="check" disabled={busy} onClick={() => run(state.board)}>
            {busy ? '检查中' : '检查本板'}
          </Btn>
          <Btn size="sm" icon="check" disabled={busy} onClick={() => run(null)}>
            检查全部
          </Btn>
          <Btn
            size="sm"
            variant="quiet"
            icon="wand"
            disabled={!issues.some((i) => i.f)}
            tip="按引擎建议修复全部可自动处理的问题"
            onClick={() =>
              void mutate<{ cmds?: unknown[] }>('inspect.fix', { auto: 1 })
                .then((r) => {
                  toast('ok', `已应用 ${r?.cmds?.length ?? 0} 条修复`);
                  run(state.board);
                })
                .catch(() => undefined)
            }
          >
            自动修复
          </Btn>
          <Btn size="sm" variant="quiet" icon="eyeOff" onClick={() => run(null)} disabled={!issues.length}>
            重新检查
          </Btn>
        </div>

        {issues.length ? (
          <div className="stack-6">
            {issues.map((it, i) => (
              <div
                key={i}
                role="button"
                tabIndex={0}
                className={`issue sev-${it.s === 'error' ? 'err' : it.s === 'warn' ? 'warn' : 'info'}`}
                onClick={() => {
                  if (it.b && it.b !== state.board) dispatch({ t: 'board', board: it.b });
                  if (it.n) select([it.n]);
                }}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && it.n) select([it.n]);
                }}
                data-tip={it.n ? '点击定位到该图层' : '整板问题'}
              >
                <i className="issue-b" />
                <div className="issue-t">
                  <b>{it.m}</b>
                  <span className="mono" style={{ fontSize: 12 }}>
                    {it.c}
                    {it.e && Object.keys(it.e).length ? ` · ${JSON.stringify(it.e)}` : ''}
                  </span>
                </div>
                {it.f ? (
                  <button
                    type="button"
                    className="li-x"
                    aria-label="应用这条修复"
                    data-tip="应用这条修复"
                    onClick={(e) => {
                      e.stopPropagation();
                      void mutate('canvas.batch', { cmds: [it.f] }).catch(() => undefined);
                    }}
                  >
                    <Icon name="wand" size={14} />
                  </button>
                ) : null}
              </div>
            ))}
          </div>
        ) : (
          <Empty
            title={busy ? '正在检查' : '还没有检查过'}
            action={
              <Btn variant="primary" icon="check" onClick={() => run(state.board)}>
                开始检查
              </Btn>
            }
          />
        )}
      </Group>

      <Group title="文档">
        <div className="stack-6">
          <div className="kv">
            <span>修订</span>
            <b>{doc.revision}</b>
          </div>
          <div className="kv">
            <span>图层</span>
            <b>{doc.nodes.length}</b>
          </div>
          <div className="kv">
            <span>画板</span>
            <b>{doc.artboards.length}</b>
          </div>
          <div className="kv">
            <span>素材</span>
            <b>{doc.assets.length}</b>
          </div>
          <div className="kv">
            <span>最后修改来自</span>
            <b>{doc.meta?.origin ?? '—'}</b>
          </div>
        </div>
      </Group>
    </>
  );
}

/* -------------------------------------------------------------------- shell */

export function Inspector() {
  const { state, dispatch, select } = useApp();
  const doc = state.doc;
  const nodes = doc ? state.sel.map((i) => nodeMap(doc).get(i)).filter((n): n is Node => !!n) : [];
  const node = nodes.length === 1 ? nodes[0] : null;
  const tab = state.rightTab;
  const board = doc ? findBoard(doc, state.board) : null;

  return (
    <aside className="insp glass glass-2" data-sheen>
      <div style={{ padding: '12px 14px 0' }}>
        <div className="row-b" style={{ marginBottom: 10 }}>
          <span className="h3 trunc">{node ? node.name : nodes.length > 1 ? `${nodes.length} 个图层` : '属性'}</span>
          {node ? <Badge tone="blue">{TYPE_LABEL[node.type]}</Badge> : board ? <Badge tone="blue">{board.name}</Badge> : null}
        </div>
        <Seg
          grow
          value={tab}
          options={[
            { v: 'style', label: '样式', icon: 'style' },
            { v: 'layout', label: '布局', icon: 'layout' },
            { v: 'inspect', label: '检查', icon: 'check' },
          ]}
          onChange={(v) => dispatch({ t: 'right-tab', tab: v })}
        />
      </div>
      <div className="insp-b">
        {!doc ? (
          <Empty title="还没有打开画布" />
        ) : !nodes.length ? (
          <Empty
            title="没有选中图层"
            action={
              <Btn
                variant="primary"
                icon="layers"
                onClick={() => {
                  const top = board?.layers[board.layers.length - 1];
                  if (top) select([top]);
                }}
              >
                选中顶层图层
              </Btn>
            }
          />
        ) : tab === 'style' ? (
          <StyleTab node={nodes[0]} />
        ) : tab === 'layout' && node ? (
          <LayoutTab node={node} />
        ) : tab === 'layout' ? (
          <Empty title="多选图层没有单独布局" action={<Btn variant="primary" icon="layers" onClick={() => select([nodes[0].id])}>只选第一个</Btn>} />
        ) : (
          <InspectTab />
        )}
      </div>
    </aside>
  );
}
