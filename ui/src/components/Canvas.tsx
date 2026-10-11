/** The canvas viewport. It displays the engine's own SVG (`render.svg` with
 *  `preview:1`), never a DOM re-implementation, so what you see is the exact
 *  byte stream the rasteriser consumes. The interaction layer sits on top in
 *  `Overlay.tsx`. */

import { useCallback, useEffect, useRef, useState } from 'react';
import { useRpc } from '../api';
import { useApp } from '../state/app';
import type { Artboard, Document } from '../types';
import { Overlay } from './Overlay';
import { Icon } from './Icons';
import { Badge } from './Primitives';
import { clamp, round } from '../lib/format';
import { nodeMap } from '../lib/doc';

const ZOOM_MIN = 0.05;
const ZOOM_MAX = 4;

function Board({ ab, doc, active }: { ab: Artboard; doc: Document; active: boolean }) {
  const { state, dispatch } = useApp();
  const hidden = doc.nodes.filter((n) => !n.visible).map((n) => n.id);
  const svg = useRpc<string>('render.svg', { p: doc.project_id, b: ab.id, preview: 1, omit: hidden });
  const [editing, setEditing] = useState(false);

  return (
    <div
      className={`board ${active ? 'is-act' : ''}`}
      style={{ width: ab.width, height: ab.height }}
      onPointerDownCapture={() => {
        if (!active) dispatch({ t: 'board', board: ab.id });
      }}
    >
      <div className="board-svg">
        {svg.data ? (
          <div style={{ width: '100%', height: '100%' }} dangerouslySetInnerHTML={{ __html: svg.data }} />
        ) : (
          <div className="skl" style={{ position: 'absolute', inset: 0 }} />
        )}
      </div>
      <div className="board-tag" data-tauri-drag-region={undefined}>
        <span>{ab.name}</span>
        <span className="num">
          {Math.round(ab.width)}×{Math.round(ab.height)}
        </span>
        {active ? <Badge tone="grad">{`${doc.artboards.indexOf(ab) + 1}/${doc.artboards.length}`}</Badge> : null}
      </div>
      {active ? (
        <Overlay
          doc={doc}
          ab={ab}
          sel={state.sel}
          hover={state.hover}
          zoom={state.view.zoom}
          editing={editing}
          setEditing={setEditing}
        />
      ) : (
        <div
          className="ov-hit"
          style={{ position: 'absolute', inset: 0, cursor: 'pointer' }}
          onPointerDown={() => dispatch({ t: 'board', board: ab.id })}
        />
      )}
      {!ab.layers.length && active ? (
        <div className="board-empty">空画板 · 用左侧「图层」或右下角 Agent 面板添加内容</div>
      ) : null}
    </div>
  );
}

export function Canvas() {
  const { state, dispatch, setView } = useApp();
  const doc = state.doc;
  const stage = useRef<HTMLDivElement | null>(null);
  const [space, setSpace] = useState(false);
  const [panning, setPanning] = useState(false);
  const panRef = useRef<{ x: number; y: number; px: number; py: number } | null>(null);
  const fitDone = useRef(false);

  const zoomTo = useCallback(
    (next: number, cx?: number, cy?: number) => {
      const el = stage.current;
      const z = clamp(round(next, 4), ZOOM_MIN, ZOOM_MAX);
      if (!el) {
        setView({ zoom: z });
        return;
      }
      const r = el.getBoundingClientRect();
      const ax = (cx ?? r.width / 2) - r.left;
      const ay = (cy ?? r.height / 2) - r.top;
      const { zoom, pan } = state.view;
      const k = z / zoom;
      setView({ zoom: z, pan: { x: ax - k * (ax - pan.x), y: ay - k * (ay - pan.y) } });
    },
    [setView, state.view],
  );

  /* fit the whole row of boards into the stage */
  const fit = useCallback(() => {
    const el = stage.current;
    if (!el || !doc) return;
    const boards = doc.artboards.filter((a) => a.visible);
    if (!boards.length) return;
    const totalW = boards.reduce((s, a) => s + a.width, 0) + 64 * (boards.length - 1) + 128;
    const totalH = Math.max(...boards.map((a) => a.height)) + 128;
    const r = el.getBoundingClientRect();
    const z = clamp(round(Math.min(r.width / totalW, r.height / totalH), 3), ZOOM_MIN, ZOOM_MAX);
    setView({ zoom: z, pan: { x: (r.width - totalW * z) / 2, y: (r.height - totalH * z) / 2 } });
  }, [doc, setView]);

  useEffect(() => {
    if (fitDone.current || !doc) return;
    fitDone.current = true;
    fit();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [doc?.project_id, fit]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return;
      if (e.code === 'Space') setSpace(true);
    };
    const onUp = (e: KeyboardEvent) => {
      if (e.code === 'Space') setSpace(false);
    };
    window.addEventListener('keydown', onKey);
    window.addEventListener('keyup', onUp);
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('keyup', onUp);
    };
  }, []);

  /* wheel zooms at the cursor; shift+wheel pans horizontally, as in design tools */
  useEffect(() => {
    const el = stage.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      if (e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
        setView({ pan: { x: state.view.pan.x - e.deltaX, y: state.view.pan.y - e.deltaY } });
        return;
      }
      const f = Math.exp(-e.deltaY * 0.0016);
      zoomTo(state.view.zoom * f, e.clientX, e.clientY);
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
  }, [setView, state.view, zoomTo]);

  if (!doc) return null;
  const boards = doc.artboards.filter((a) => a.visible);
  const byId = nodeMap(doc);

  const startPan = (e: React.PointerEvent) => {
    if (!(space || e.button === 1)) return;
    e.preventDefault();
    panRef.current = { x: e.clientX, y: e.clientY, px: state.view.pan.x, py: state.view.pan.y };
    setPanning(true);
    const move = (ev: PointerEvent) => {
      const p = panRef.current;
      if (!p) return;
      setView({ pan: { x: p.px + (ev.clientX - p.x), y: p.py + (ev.clientY - p.y) } });
    };
    const up = () => {
      panRef.current = null;
      setPanning(false);
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };

  return (
    <>
      <div
        ref={stage}
        className={`stage ${space ? 'is-pan' : ''} ${panning ? 'is-panning' : ''}`}
        onPointerDown={startPan}
        onDoubleClick={(e) => {
          if (e.target === e.currentTarget) dispatch({ t: 'sel', sel: [] });
        }}
      >
        <div
          className="world"
          style={{ transform: `translate3d(${state.view.pan.x}px, ${state.view.pan.y}px, 0) scale(${state.view.zoom})` }}
        >
          {boards.map((ab) => (
            <Board key={ab.id} ab={ab} doc={doc} active={ab.id === state.board} />
          ))}
        </div>

        {/* viewport chrome, in screen space */}
        <div className="row" style={{ position: 'absolute', left: 12, bottom: 12, gap: 6, zIndex: 20 }}>
          <div className="glass glass-1" style={{ display: 'flex', alignItems: 'center', gap: 2, padding: 4, borderRadius: 999 }} data-sheen>
            <button type="button" className="li-x" data-tip="缩小" aria-label="缩小" onClick={() => zoomTo(state.view.zoom / 1.25)}>
              <Icon name="zoomOut" size={15} />
            </button>
            <button
              type="button"
              className="li-x"
              data-tip="缩放比例 · 点击回到 100%"
              aria-label="缩放比例"
              style={{ width: 58, fontFamily: 'var(--mono)', fontSize: 12 }}
              onClick={() => zoomTo(1)}
            >
              {Math.round(state.view.zoom * 100)}%
            </button>
            <button type="button" className="li-x" data-tip="放大" aria-label="放大" onClick={() => zoomTo(state.view.zoom * 1.25)}>
              <Icon name="zoomIn" size={15} />
            </button>
            <button type="button" className="li-x" data-tip="适配全部画板" aria-label="适配" onClick={fit}>
              <Icon name="fit" size={15} />
            </button>
          </div>
          {state.sel.length > 1 ? (
            <div className="glass glass-1" style={{ display: 'flex', alignItems: 'center', gap: 6, padding: '4px 10px', borderRadius: 999 }} data-sheen>
              <span style={{ fontSize: 13, fontWeight: 700 }}>{state.sel.length} 个图层</span>
              <span className="num" style={{ fontSize: 12, color: 'var(--t3)' }}>
                {(() => {
                  const ns = state.sel.map((i) => byId.get(i)).filter(Boolean);
                  const x = Math.min(...ns.map((n) => n!.box.x));
                  const y = Math.min(...ns.map((n) => n!.box.y));
                  return `X ${Math.round(x)} · Y ${Math.round(y)}`;
                })()}
              </span>
            </div>
          ) : null}
        </div>
      </div>

      <div className="studio-bar" style={{ borderTop: '1px solid var(--line)', borderBottom: 'none' }}>
        <span className="t3" style={{ fontSize: 13, marginRight: 'auto' }}>
          空格 / 中键拖拽平移 · 滚轮缩放 · 双击文字直接改
        </span>
        <Badge tone={doc.meta?.dirty ? 'warn' : 'ok'}>{doc.meta?.dirty ? '有未保存改动' : '与磁盘一致'}</Badge>
        {doc.meta?.origin ? <Badge>{`最后修改 ${doc.meta.origin}`}</Badge> : null}
      </div>
    </>
  );
}
