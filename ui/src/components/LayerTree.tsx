/** Layer tree: drag to reorder or re-parent, eye + lock, inline rename, and a
 *  glass context menu. All of it dispatches `canvas.*`. */

import { useMemo, useRef, useState } from 'react';
import { useApp } from '../state/app';
import type { Artboard, Document, Node } from '../types';
import { TYPE_COLOR, TYPE_LABEL, layerRows } from '../lib/doc';
import { Icon, type IconName } from './Icons';
import { GlassMenu, type MenuSpec } from './Primitives';
function TypeGlyph({ node }: { node: Node }) {
  const map: Record<string, IconName> = {
    text: 'text',
    image: 'image',
    rect: 'square',
    ellipse: 'circle',
    line: 'line',
    group: 'group',
    frame: 'boards',
    polygon: 'square',
    path: 'wand',
    svg: 'code',
  };
  return <Icon name={map[node.type] ?? 'square'} size={14} />;
}

export function LayerTree({ doc, board }: { doc: Document; board: Artboard }) {
  const { mutate, select, dispatch, toast, state } = useApp();
  const rows = layerRows(doc, board);
  const [menu, setMenu] = useState<{ x: number; y: number; node: Node } | null>(null);
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [renaming, setRenaming] = useState<string | null>(null);
  const dragId = useRef<string | null>(null);
  const [overRow, setOverRow] = useState<{ id: string; zone: 'above' | 'below' | 'into' } | null>(null);

  const visible = useMemo(() => {
    const out = new Set<string>();
    for (const { node } of rows) {
      let p = node.parent;
      let open = true;
      while (p) {
        if (collapsed[p]) open = false;
        p = doc.nodes.find((x) => x.id === p)?.parent ?? undefined;
      }
      if (open) out.add(node.id);
    }
    return out;
  }, [rows, collapsed, doc.nodes]);

  const byIndex = (id: string) => rows.findIndex((r) => r.node.id === id);
  const toLayersIndex = (targetId: string, zone: 'above' | 'below' | 'into') => {
    const ab = board;
    const i = byIndex(targetId);
    if (zone === 'into') {
      const kids = (doc.nodes.find((n) => n.id === targetId)?.children ?? []).length;
      return kids;
    }
    /* rows are painted top-first, `ab.layers` is bottom-first */
    const base = ab.layers.length - 1 - i;
    return zone === 'above' ? base + 1 : base;
  };

  const move = (id: string, targetId: string, zone: 'above' | 'below' | 'into') => {
    if (id === targetId) return;
    const subtree = new Set([id, ...(doc.nodes.find((n) => n.id === id)?.children ?? [])]);
    if (subtree.has(targetId)) {
      toast('info', '不能把图层放进它自己里面');
      return;
    }
    const to = toLayersIndex(targetId, zone);
    const parent = zone === 'into' ? targetId : doc.nodes.find((n) => n.id === targetId)?.parent;
    void mutate('canvas.reorder', { id, to, b: board.id, ...(parent ? { parent } : {}) }).catch(() => undefined);
  };

  const act = (n: Node, kbd: string) => {
    switch (kbd) {
      case 'toggle':
        void mutate('canvas.set', { ids: [n.id], patch: { visible: !n.visible } }).catch(() => undefined);
        break;
      case 'lock':
        void mutate('canvas.set', { ids: [n.id], patch: { locked: !n.locked } }).catch(() => undefined);
        break;
      case 'rename':
        setRenaming(n.id);
        break;
      case 'dup':
        void mutate('canvas.duplicate', { ids: [n.id] })
          .then((r) => r?.ids && select(r.ids))
          .catch(() => undefined);
        break;
      case 'del':
        dispatch({
          t: 'sheet',
          sheet: {
            name: 'confirm',
            props: {
              title: `删除「${n.name}」`,
              body: '会从画板中移除该图层（含其子图层），可用撤销恢复。',
              danger: true,
              action: { m: 'canvas.delete', p: { ids: [n.id] } },
              done: '已删除',
            },
          },
        });
        break;
      case 'front':
        void mutate('canvas.reorder', { id: n.id, to: board.layers.length, b: board.id }).catch(() => undefined);
        break;
      case 'back':
        void mutate('canvas.reorder', { id: n.id, to: 0, b: board.id }).catch(() => undefined);
        break;
      case 'group':
        void mutate('canvas.group', { ids: [n.id, ...(n.children ?? [])] }).catch(() => undefined);
        break;
      case 'ungroup':
        void mutate('canvas.ungroup', { id: n.id }).catch(() => undefined);
        break;
      default:
        break;
    }
  };

  const items = (n: Node): MenuSpec[] => [
    { label: n.visible ? '隐藏图层' : '显示图层', icon: n.visible ? 'eyeOff' : 'eye', onClick: () => act(n, 'toggle') },
    { label: n.locked ? '解锁' : '锁定', icon: n.locked ? 'unlock' : 'lock', onClick: () => act(n, 'lock') },
    { label: '重命名', icon: 'text', onClick: () => act(n, 'rename') },
    { label: '复制图层', icon: 'copy', kbd: '⌘D', onClick: () => act(n, 'dup') },
    { label: '置于顶层', icon: 'up', onClick: () => act(n, 'front') },
    { label: '置于底层', icon: 'down', onClick: () => act(n, 'back') },
    n.children?.length
      ? { label: '取消群组', icon: 'group', onClick: () => act(n, 'ungroup') }
      : { label: '与子图层成组', icon: 'group', disabled: !n.children?.length && n.type !== 'group', onClick: () => act(n, 'group') },
    { label: '删除', icon: 'trash', danger: true, kbd: 'Del', onClick: () => act(n, 'del') },
  ];

  if (!rows.length) {
    return (
      <div className="empty" style={{ padding: '26px 10px' }}>
        <svg className="empty-ill" viewBox="0 0 92 72" fill="none" aria-hidden="true">
          <rect x="14" y="12" width="64" height="48" rx="12" fill="rgba(255,255,255,.7)" stroke="rgba(23,29,63,.14)" />
          <path d="M30 44l14-16 12 12 6-6 6 10H28z" fill="rgba(75,107,251,.28)" />
          <circle cx="36" cy="26" r="5" fill="rgba(139,92,246,.5)" />
        </svg>
        <h4>这个画板还是空的</h4>
        <div className="row" style={{ gap: 8 }}>
          <button
            type="button"
            className="btn btn-sm btn-primary"
            onClick={() =>
              void mutate('canvas.add', {
                b: board.id,
                t: 'text',
                name: '标题',
                x: Math.round(board.width * 0.08),
                y: Math.round(board.height * 0.36),
                w: Math.round(board.width * 0.84),
                h: Math.round(board.height * 0.12),
                text: '在这里输入标题',
                size: Math.round(board.width * 0.09),
                weight: 800,
                color: '#171D3F',
                intent: { anchor: 'top-left', margin_top: Math.round(board.height * 0.36), margin_left: Math.round(board.width * 0.08), size: { w_ref: 0.84 } },
                solve: 1,
              }).catch(() => undefined)
            }
          >
            <Icon name="text" size={14} />
            加一个标题
          </button>
          <button
            type="button"
            className="btn btn-sm"
            onClick={() =>
              void mutate('canvas.add', {
                b: board.id,
                t: 'rect',
                name: '底色块',
                x: 0,
                y: 0,
                w: board.width,
                h: board.height,
                at: 0,
                style: { fill: { t: 'solid', color: '#F6F7FB' }, radius: { all: 0 } },
                intent: { size: { mode: 'fill' } },
                solve: 1,
              }).catch(() => undefined)
            }
          >
            <Icon name="square" size={14} />
            加底色块
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="tree">
      {rows.map(({ node: n, depth }) => {
        if (!visible.has(n.id)) return null;
        const on = state.sel.includes(n.id);
        return (
          <div
            key={n.id}
            className={[
              'tnode',
              on ? 'is-on' : '',
              !n.visible ? 'dim' : '',
              dragId.current === n.id ? 'is-drag' : '',
              overRow?.id === n.id && overRow.zone !== 'into' ? 'is-drop' : '',
              overRow?.id === n.id && overRow.zone === 'into' ? 'is-on' : '',
            ]
              .filter(Boolean)
              .join(' ')}
            style={{ paddingLeft: 4 + depth * 14 }}
            draggable
            onDragStart={(e) => {
              dragId.current = n.id;
              e.dataTransfer.effectAllowed = 'move';
              e.dataTransfer.setData('text/plain', n.id);
            }}
            onDragEnd={() => {
              dragId.current = null;
              setOverRow(null);
            }}
            onDragOver={(e) => {
              e.preventDefault();
              const r = e.currentTarget.getBoundingClientRect();
              const rel = (e.clientY - r.top) / r.height;
              const isBox = n.type === 'group' || n.type === 'frame';
              setOverRow({ id: n.id, zone: isBox && rel > 0.3 && rel < 0.7 ? 'into' : rel < 0.5 ? 'above' : 'below' });
            }}
            onDrop={(e) => {
              e.preventDefault();
              const src = e.dataTransfer.getData('text/plain') || dragId.current;
              if (src && overRow) move(src, n.id, overRow.zone);
              setOverRow(null);
              dragId.current = null;
            }}
            onPointerDown={() => select([n.id])}
            onDoubleClick={() => {
              if (n.type === 'text') {
                select([n.id]);
                dispatch({ t: 'board', board: board.id });
              }
            }}
            onContextMenu={(e) => {
              e.preventDefault();
              select([n.id]);
              setMenu({ x: e.clientX, y: e.clientY, node: n });
            }}
          >
            {n.children?.length ? (
              <button
                type="button"
                className="tnode-c"
                aria-label="展开或折叠"
                onClick={(e) => {
                  e.stopPropagation();
                  setCollapsed((c) => ({ ...c, [n.id]: !c[n.id] }));
                }}
              >
                <Icon name={collapsed[n.id] ? 'chevron' : 'chevronDown'} size={13} />
              </button>
            ) : (
              <span className="tnode-c" />
            )}
            <span style={{ color: TYPE_COLOR[n.type], display: 'grid', placeItems: 'center', width: 16, flex: '0 0 auto' }}>
              <TypeGlyph node={n} />
            </span>
            <span
              className="tnode-n"
              contentEditable={renaming === n.id}
              suppressContentEditableWarning
              onBlur={(e) => {
                const v = e.currentTarget.textContent?.trim() ?? '';
                if (renaming === n.id && v && v !== n.name) {
                  void mutate('canvas.set', { ids: [n.id], patch: { name: v } }).catch(() => undefined);
                }
                setRenaming(null);
              }}
              onKeyDown={(e) => {
                if (e.key === 'Enter') (e.target as HTMLElement).blur();
                if (e.key === 'Escape') setRenaming(null);
              }}
              data-tip={`${TYPE_LABEL[n.type]} · ${Math.round(n.box.w)}×${Math.round(n.box.h)}`}
            >
              {n.name}
            </span>
            {n.layout.mode === 'constraint' ? (
              <span className="tnode-t" data-tip="由布局意图求解得到位置">
                auto
              </span>
            ) : null}
            {n.locked ? <span className="dot" style={{ background: 'var(--warn)' }} /> : null}
            <span className="row" style={{ gap: 0, marginLeft: 'auto' }}>
              <button
                type="button"
                className={`li-x ${!n.visible ? 'is-on' : ''}`}
                aria-label="显示或隐藏"
                data-tip={n.visible ? '隐藏图层' : '显示图层'}
                onClick={(e) => {
                  e.stopPropagation();
                  act(n, 'toggle');
                }}
              >
                <Icon name={n.visible ? 'eye' : 'eyeOff'} size={14} />
              </button>
              <button
                type="button"
                className={`li-x ${n.locked ? 'is-on' : ''}`}
                aria-label="锁定或解锁"
                data-tip={n.locked ? '解锁图层' : '锁定图层'}
                onClick={(e) => {
                  e.stopPropagation();
                  act(n, 'lock');
                }}
              >
                <Icon name={n.locked ? 'lock' : 'unlock'} size={14} />
              </button>
            </span>
          </div>
        );
      })}

      {menu ? <GlassMenu x={menu.x} y={menu.y} items={items(menu.node)} onClose={() => setMenu(null)} /> : null}
    </div>
  );
}
