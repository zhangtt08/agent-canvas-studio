/** Left dock of the studio: the icon rail plus the four panels it switches
 *  between (画板 / 图层 / 素材 / 模板). */

import { useMemo, useRef, useState } from 'react';
import { absUrl, useRpc } from '../api';
import { useApp, type LeftTab } from '../state/app';
import type { Artboard, Asset, AssetRow, BoardInfo, Document, Mut, NodeType, Template, V } from '../types';
import { Icon, type IconName } from './Icons';
import { Badge, Btn, Chip, Empty, Field, IconBtn, NumVal, Select, TextInput } from './Primitives';
import { LayerTree } from './LayerTree';
import { findBoard } from '../lib/doc';
import { fmtBytes, ratioLabel } from '../lib/format';

const TABS: { v: LeftTab; label: string; icon: IconName }[] = [
  { v: 'boards', label: '画板', icon: 'boards' },
  { v: 'layers', label: '图层', icon: 'layers' },
  { v: 'assets', label: '素材', icon: 'assets' },
  { v: 'templates', label: '模板', icon: 'templates' },
];

export function Rail() {
  const { state, dispatch, openSheet, go } = useApp();
  const counts: Partial<Record<LeftTab, number>> = {
    boards: state.doc?.artboards.length,
    layers: state.doc?.nodes.length,
    assets: state.doc?.assets.length,
  };
  return (
    <nav className="rail" data-tauri-drag-region={undefined}>
      {TABS.map((t) => (
        <button
          key={t.v}
          type="button"
          className={`rail-i ${state.leftTab === t.v ? 'is-on' : ''}`}
          data-tip={t.label}
          aria-label={t.label}
          onClick={() => dispatch({ t: 'left-tab', tab: t.v })}
        >
          <Icon name={t.icon} size={18} />
          <em>{t.label}</em>
          {counts[t.v] ? (
            <span
              className="badge"
              style={{ position: 'absolute', top: 2, right: 2, height: 17, padding: '0 5px', fontSize: 12 }}
            >
              {counts[t.v]}
            </span>
          ) : null}
        </button>
      ))}
      <div className="rail-sp" />
      <button type="button" className="rail-i" data-tip="图片处理工作台" aria-label="图片处理" onClick={() => go('lab')}>
        <Icon name="crop" size={18} />
        <em>处理</em>
      </button>
      <button type="button" className="rail-i" data-tip="快捷键与说明" aria-label="帮助" onClick={() => openSheet('help', { topic: 'keys' })}>
        <Icon name="question" size={18} />
        <em>帮助</em>
      </button>
      <button type="button" className="rail-i" data-tip="设置" aria-label="设置" onClick={() => openSheet('settings')}>
        <Icon name="settings" size={18} />
        <em>设置</em>
      </button>
    </nav>
  );
}

/* ------------------------------------------------------------------ boards */

function BoardsPanel({ doc, board }: { doc: Document; board: Artboard | null }) {
  const { mutate, dispatch, openSheet } = useApp();
  /* `artboard.list` reports the layer count as `n`, not `layers`. */
  const list = useRpc<BoardInfo[]>('artboard.list', { p: doc.project_id });
  const [draft, setDraft] = useState<V>({});

  const set = (k: string, v: number | string) => setDraft((d) => ({ ...d, [k]: v }));
  const val = (k: string, d: number) => (typeof draft[k] === 'number' ? (draft[k] as number) : d);

  return (
    <div className="stack" style={{ gap: 8 }}>
      <div className="row-b">
        <span className="grp-h" style={{ margin: 0 }}>
          画板
        </span>
        <IconBtn
          icon="plus"
          size="sm"
          tip="按当前尺寸新建画板"
          label="新建画板"
          onClick={() =>
            /* `artboard.create` names the board with `board_name`, and takes
               `background` / `safe` — `name` is silently ignored by the engine. */
            void mutate<Mut>('artboard.create', {
              w: board?.width ?? 1080,
              h: board?.height ?? 1440,
              board_name: `画板 ${(list.data?.length ?? doc.artboards.length) + 1}`,
              background: board?.background?.t === 'solid' ? board.background.color : '#FFFFFF',
              safe: board?.safe_margin ?? 72,
            })
              .then((r) => {
                const id = r?.created?.[0];
                if (id) dispatch({ t: 'board', board: id });
              })
              .catch(() => undefined)
          }
        />
      </div>

      {(list.data ?? doc.artboards.map((a) => ({ b: a.id, name: a.name, w: a.width, h: a.height, n: a.layers.length }))).map((a, i) => {
        const on = board?.id === a.b;
        const nlayers = a.n ?? 0;
        return (
          <button
            key={a.b}
            type="button"
            className={`li ${on ? 'is-on' : ''}`}
            onClick={() => dispatch({ t: 'board', board: a.b })}
          >
            <span className="badge" style={{ minWidth: 24, justifyContent: 'center' }}>
              {String(i + 1).padStart(2, '0')}
            </span>
            <span className="li-t">
              <b>{a.name}</b>
              <span>
                {Math.round(a.w)}×{Math.round(a.h)} · {ratioLabel(a.w, a.h)} · {nlayers} 层
              </span>
            </span>
            <span className="li-r">
              <span
                className="li-x"
                role="button"
                tabIndex={0}
                aria-label="复制成其他比例"
                data-tip="按比例复制画板（重排布局意图）"
                onClick={(e) => {
                  e.stopPropagation();
                  openSheet('template', { mode: 'ratios', b: a.b });
                }}
              >
                <Icon name="copy" size={14} />
              </span>
              <span
                className="li-x danger"
                role="button"
                tabIndex={0}
                aria-label="删除画板"
                data-tip="删除画板"
                onClick={(e) => {
                  e.stopPropagation();
                  openSheet('confirm', {
                    title: `删除「${a.name}」`,
                    body: `画板上的 ${nlayers} 个图层会一起删除。`,
                    danger: true,
                    action: { m: 'artboard.delete', p: { b: a.b } },
                    done: '画板已删除',
                  });
                }}
              >
                <Icon name="trash" size={14} />
              </span>
            </span>
          </button>
        );
      })}

      {board ? (
        <div className="grp" style={{ borderTop: '1px solid var(--line)' }}>
          <div className="grp-h">尺寸与安全边距</div>
          <div className="box4">
            <NumVal tag="W" value={val('w', board.width)} min={1} max={20000} dp={0} onChange={(v) => set('w', v)} />
            <NumVal tag="H" value={val('h', board.height)} min={1} max={20000} dp={0} onChange={(v) => set('h', v)} />
            <NumVal tag="安全" value={val('safe', board.safe_margin ?? 0)} min={0} max={2000} dp={0} onChange={(v) => set('safe', v)} />
            <span className="badge" style={{ justifyContent: 'center' }}>{ratioLabel(board.width, board.height)}</span>
          </div>
          <div className="row" style={{ marginTop: 8, gap: 6 }}>
            <Btn
              size="sm"
              variant="primary"
              icon="check"
              tip="artboard.resize — 改画板尺寸，内容按约束重排"
              onClick={() =>
                /* `artboard.resize` always reflows (it maps to Cmd::Reflow); the
                   `mode` knob is scale | reflow | adaptive, `reflow:1` is not a key. */
                void mutate('artboard.resize', { b: board.id, w: val('w', board.width), h: val('h', board.height), mode: 'reflow' })
                  .then(() => mutate('artboard.set', { b: board.id, s: { safe_margin: val('safe', board.safe_margin ?? 0) } }))
                  .catch(() => undefined)
              }
            >
              应用
            </Btn>
            <Btn
              size="sm"
              icon="wand"
              tip="缩放内容并重新求解布局意图"
              onClick={() =>
                void mutate('layout.reflow', { b: board.id, w: val('w', board.width), h: val('h', board.height), mode: 'reflow' }).catch(() => undefined)
              }
            >
              重排
            </Btn>
          </div>
          <div style={{ marginTop: 8 }}>
            <Field label="底色">
              <TextInput
                value={typeof draft.background === 'string' ? draft.background : board.background?.t === 'solid' ? board.background.color : 'none'}
                onChange={(v) => {
                  set('background', v);
                  void mutate('artboard.set', { b: board.id, s: { background: v === 'none' ? { t: 'none' } : { t: 'solid', color: v } } }).catch(() => undefined);
                }}
                tip="#RRGGBB 或 none"
                mono
              />
            </Field>
          </div>
        </div>
      ) : (
        <Empty title="还没有画板" action={<Btn variant="primary" icon="plus" onClick={() => void mutate('artboard.create', { w: 1080, h: 1440 }).catch(() => undefined)}>新建画板</Btn>} />
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ layers */

const ADDS: { t: NodeType; icon: IconName; label: string }[] = [
  { t: 'text', icon: 'text', label: '文字' },
  { t: 'rect', icon: 'square', label: '矩形' },
  { t: 'ellipse', icon: 'circle', label: '椭圆' },
  { t: 'line', icon: 'line', label: '直线' },
  { t: 'polygon', icon: 'square', label: '多边形' },
  { t: 'frame', icon: 'boards', label: '容器' },
];

function LayersPanel({ doc, board }: { doc: Document; board: Artboard | null }) {
  const { mutate, select, state } = useApp();
  if (!board) return <Empty title="先新建一个画板" action={<Btn variant="primary" icon="plus" onClick={() => void mutate('artboard.create', { w: 1080, h: 1440 }).catch(() => undefined)}>新建画板</Btn>} />;
  return (
    <div className="stack" style={{ gap: 8 }}>
      <div className="row" style={{ gap: 4, flexWrap: 'wrap' }}>
        {ADDS.map((a) => (
          <button
            key={a.t}
            type="button"
            className="btn btn-sm"
            data-tip={`新建${a.label}图层`}
            onClick={() =>
              void mutate('canvas.add', {
                b: board.id,
                t: a.t,
                name: a.label,
                x: Math.round(board.width * 0.2),
                y: Math.round(board.height * 0.2),
                w: Math.round(a.t === 'line' ? board.width * 0.6 : board.width * 0.36),
                h: Math.round(a.t === 'line' ? 0 : board.height * 0.18),
                size: 48,
                text: a.t === 'text' ? '新文字图层' : undefined,
                sides: a.t === 'polygon' ? 3 : undefined,
                style:
                  a.t === 'frame'
                    ? { fill: { t: 'none' }, stroke: { color: '#171D3F33', width: 2 }, radius: { all: 16 } }
                    : a.t === 'line'
                      ? { stroke: { color: '#171D3F', width: 3 } }
                      : { fill: { t: 'solid', color: a.t === 'ellipse' ? '#8B5CF6' : '#4B6BFB' }, radius: { all: a.t === 'rect' ? 16 : 0 } },
                l: { mode: 'manual' },
                solve: 0,
              })
                .then((r) => r?.id && select([r.id]))
                .catch(() => undefined)
            }
          >
            <Icon name={a.icon} size={14} />
          </button>
        ))}
        <span style={{ marginRight: 'auto' }} />
        <IconBtn
          icon="group"
          size="sm"
          tip={state.sel.length > 1 ? '把选中图层成组' : '选中 2 个以上图层可成组'}
          label="成组"
          disabled={state.sel.length < 2}
          onClick={() => void mutate('canvas.group', { ids: state.sel }).catch(() => undefined)}
        />
      </div>
      <div className="row-b">
        <span className="grp-h" style={{ margin: 0 }}>
          图层 · {board.name}
        </span>
        <Badge tone="blue">{board.layers.length}</Badge>
      </div>
      <LayerTree doc={doc} board={board} />
    </div>
  );
}

/* ------------------------------------------------------------------ assets */

function AssetsPanel({ doc }: { doc: Document }) {
  const { mutate, select, state, openSheet, toast } = useApp();
  const file = useRef<HTMLInputElement | null>(null);
  const [pathDraft, setPathDraft] = useState('');
  const list = useRpc<AssetRow[]>('asset.list', { p: doc.project_id });
  /* `asset.list` is the compact projection `{id,name,w,h,kind,bytes}` — it has no
     `uri` and no palette. Join it onto the document's full records so the grid can
     actually load the pixels, and keep the live row when the doc has not caught up. */
  const assets: Asset[] = useMemo(() => {
    const byId = new Map(doc.assets.map((a) => [a.id, a]));
    const rows = list.data;
    if (!rows?.length) return doc.assets;
    return rows.map((r) => {
      const full = byId.get(r.id);
      if (full) return full;
      const minimal: Asset = { id: r.id, name: r.name, uri: '', width: r.w, height: r.h, bytes: r.bytes ?? 0 };
      return minimal;
    });
  }, [list.data, doc.assets]);

  const importPaths = (paths: string[]) => {
    const clean = paths.map((s) => s.trim()).filter(Boolean);
    if (!clean.length) return;
    void mutate<Mut>('asset.import', { paths: clean, mode: 'copy', dedupe: 1 })
      .then((r) => {
        if (!r) return;
        list.reload();
        toast('ok', `已导入 ${clean.length} 张图`);
      })
      .catch(() => undefined);
    setPathDraft('');
  };

  const place = (a: Asset) => {
    const board = findBoard(doc, state.board);
    if (!board) return;
    if (state.sel.length === 1) {
      const target = doc.nodes.find((n) => n.id === state.sel[0]);
      if (target?.type === 'image') {
        void mutate('canvas.image', { id: target.id, asset: a.id, fit: 'cover' }).catch(() => undefined);
        toast('ok', `已替换「${target.name}」的图`);
        return;
      }
    }
    void mutate('canvas.add', {
      b: board.id,
      t: 'image',
      name: a.name,
      asset: a.id,
      x: Math.round(board.width * 0.2),
      y: Math.round(board.height * 0.2),
      w: Math.round(board.width * 0.6),
      h: Math.round(board.width * 0.6 * (a.height / Math.max(1, a.width))),
      fit: 'cover',
      style: { radius: { all: 24 } },
      solve: 0,
    })
      .then((r) => r?.id && select([r.id]))
      .catch(() => undefined);
  };

  return (
    <div className="stack" style={{ gap: 8 }}>
      <div className="row-b">
        <span className="grp-h" style={{ margin: 0 }}>
          素材
        </span>
        <div className="row" style={{ gap: 4 }}>
          <input
            ref={file}
            type="file"
            accept="image/*"
            multiple
            style={{ display: 'none' }}
            onChange={(e) => {
              /* A webview only ever hands us file *names*; the engine resolves those
                 against the workspace root, so pass them straight through and let its
                 NOT_FOUND hint say what happened rather than inventing a path. */
              const names = Array.from(e.target.files ?? []).map((f) => f.name);
              if (names.length) importPaths(names);
              e.target.value = '';
            }}
          />
          <Btn size="sm" icon="plus" onClick={() => file.current?.click()} tip="从工作区目录导入（浏览器只能给出文件名）">
            选文件
          </Btn>
        </div>
      </div>

      <div className="row" style={{ gap: 6 }}>
        <div style={{ flex: 1, minWidth: 0 }}>
          <TextInput
            value={pathDraft}
            onChange={setPathDraft}
            placeholder="素材路径 · 相对工作区或绝对路径"
            mono
            onEnter={(v) => importPaths(v.split(/[,\n]/))}
            tip="例如 product.png 或 C:/pics/a.png — 交给 asset.import"
            ariaLabel="素材路径"
          />
        </div>
        <Btn size="sm" variant="primary" icon="assets" disabled={!pathDraft.trim()} onClick={() => importPaths(pathDraft.split(/[,\n]/))}>
          导入
        </Btn>
      </div>

      {assets.length ? (
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 8 }}>
          {assets.map((a) => (
            <div
              key={a.id}
              className="glass glass-inset lift"
              style={{ padding: 6, borderRadius: 14, cursor: 'pointer' }}
              data-sheen
              onClick={() => place(a)}
              onContextMenu={(e) => {
                e.preventDefault();
                openSheet('confirm', {
                  title: `移除素材「${a.name}」`,
                  body: '引用它的图层会一起删除。',
                  danger: true,
                  action: { m: 'asset.remove', p: { id: a.id, force: true } },
                  done: '素材已移除',
                });
              }}
            >
              <div className="thumb thumb-sq" style={{ borderRadius: 10, background: '#fff' }}>
                <img src={absUrl(a.thumb ?? a.uri)} alt={a.name} loading="lazy" />
              </div>
              <div style={{ padding: '6px 2px 2px' }}>
                <div className="trunc" style={{ fontSize: 13, fontWeight: 700 }}>{a.name}</div>
                <div className="t3" style={{ fontSize: 13 }}>
                  {Math.round(a.width)}×{Math.round(a.height)} · {fmtBytes(a.bytes)}
                </div>
              </div>
            </div>
          ))}
        </div>
      ) : (
        <Empty
          title="还没有素材"
          note="在上方填一个工作区内的图片路径，或把图片直接拖到画布上"
          action={
            <Btn variant="primary" icon="plus" onClick={() => file.current?.click()}>
              选文件
            </Btn>
          }
        />
      )}

      <div className="grp">
        <div className="grp-h">调色板 · 从素材提取</div>
        <div className="row" style={{ gap: 6, flexWrap: 'wrap' }}>
          {assets.flatMap((a) => a.palette ?? []).slice(0, 10).map((c, i) => (
            <button
              key={i}
              type="button"
              className="cbtn"
              style={{ background: c }}
              data-tip={`${c} · 点击作为选中图层填充色`}
              aria-label={c}
              onClick={() => {
                if (!state.sel.length) {
                  toast('info', '先选中一个图层');
                  return;
                }
                void mutate('canvas.batch', {
                  cmds: state.sel.map((id) => ({ t: 'Style', id, style: { fill: { t: 'solid', color: c } } })),
                }).catch(() => undefined);
              }}
            />
          ))}
          {!assets.some((a) => a.palette?.length) ? (
            <IconBtn
              icon="info"
              size="sm"
              tip="导入带调色板的素材后，这里会出现可点击的主色"
              label="调色板为空"
              disabled
            />
          ) : null}
        </div>
      </div>
    </div>
  );
}

/* --------------------------------------------------------------- templates */

function TemplatesPanel({ doc }: { doc: Document }) {
  const { mutate, openSheet, state } = useApp();
  const list = useRpc<Template[]>('template.list', {});
  /* `template.list` rows are `{name, desc, builtin:true}` and the last row is a
     summary carrying every built-in as `builtin:[[name,desc],…]` — drop it. */
  const rows = (list.data ?? []).filter((t) => t.builtin !== true);
  const items = rows.filter((t) => !t.builtin);
  const builtins = rows.filter((t) => t.builtin === true);
  const board = findBoard(doc, state.board);

  return (
    <div className="stack" style={{ gap: 10 }}>
      <div className="row-b">
        <span className="grp-h" style={{ margin: 0 }}>
          内置版式
        </span>
        <IconBtn icon="refresh" size="sm" tip="重新读取模板" label="刷新" onClick={() => list.reload()} />
      </div>
      {builtins.map((t) => (
        <button
          key={t.name}
          type="button"
          className="li"
          onClick={() => board && void mutate('template.use', { b: board.id, tpl: t.name, vars: { title: '少即是多' } }).catch(() => undefined)}
        >
          <span className="dot" style={{ background: 'var(--grad)' }} />
          <span className="li-t">
            <b>{t.name}</b>
            <span>{t.desc}</span>
          </span>
          <span className="li-r">
            <Icon name="plus" size={14} />
          </span>
        </button>
      ))}

      <div className="grp">
        <div className="row-b">
          <span className="grp-h" style={{ margin: 0 }}>
            我的模板
          </span>
          <Btn size="sm" icon="save" disabled={!board} tip={board ? '把当前画板存成模板' : '先选中画板'} onClick={() => openSheet('template', { mode: 'save' })}>
            存为模板
          </Btn>
        </div>
        {items.length ? (
          items.map((t) => (
            <button key={t.name} type="button" className="li" onClick={() => board && void mutate('template.use', { b: board.id, tpl: t.name }).catch(() => undefined)}>
              <span className="dot" style={{ background: 'var(--coral)' }} />
              <span className="li-t">
                <b>{t.name}</b>
                <span>{t.desc}</span>
              </span>
            </button>
          ))
        ) : (
          <Empty title="还没有自定义模板" action={<Btn variant="primary" icon="save" onClick={() => openSheet('template', { mode: 'save' })}>存为模板</Btn>} />
        )}
      </div>

      <div className="grp">
        <div className="grp-h">批量出系列</div>
        <div className="grid-chips">
          <Chip onClick={() => openSheet('template', { mode: 'variants' })} tip="同版式换配色 / 文案，生成多个画板">
            生成变体
          </Chip>
          <Chip onClick={() => openSheet('template', { mode: 'ratios' })} tip="把当前画板复制成别的比例">
            复制比例
          </Chip>
        </div>
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------ shell */

export function SidePanel({ doc }: { doc: Document }) {
  const { state } = useApp();
  const board = findBoard(doc, state.board);
  const tab = TABS.find((t) => t.v === state.leftTab)!;
  return (
    <section className="side-panel">
      <div className="side-panel-h">
        <span className="h3">{tab.label}</span>
        <Badge tone="blue">{board ? board.name : '未选画板'}</Badge>
      </div>
      <div className="side-panel-b">
        {state.leftTab === 'boards' ? <BoardsPanel doc={doc} board={board} /> : null}
        {state.leftTab === 'layers' ? <LayersPanel doc={doc} board={board} /> : null}
        {state.leftTab === 'assets' ? <AssetsPanel doc={doc} /> : null}
        {state.leftTab === 'templates' ? <TemplatesPanel doc={doc} /> : null}
      </div>
    </section>
  );
}

export function BoardPicker() {
  const { state, dispatch } = useApp();
  const doc = state.doc;
  if (!doc) return null;
  return (
    <Select
      value={state.board ?? ''}
      onChange={(v) => dispatch({ t: 'board', board: v })}
      options={doc.artboards.map((a) => ({ v: a.id, label: a.name }))}
      tip="切换当前画板"
      ariaLabel="画板"
    />
  );
}
