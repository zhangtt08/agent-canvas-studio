/** The interaction layer over the engine SVG: selection frames, 8 resize handles,
 *  a rotation handle, alignment guides, safe-area frame and the marquee.
 *
 *  Every gesture ends in the same RPC an agent calls — there is no local-only
 *  edit path — so human and agent history entries are identical. */

import { useCallback, useMemo, useRef, useState } from 'react';
import { useApp } from '../state/app';
import type { Artboard, Document, Node, Rect } from '../types';
import { cx, cy, nodeMap, paintOrder } from '../lib/doc';
import { round } from '../lib/format';

type Handle = 'nw' | 'n' | 'ne' | 'e' | 'se' | 's' | 'sw' | 'w';

const CURSOR: Record<Handle, string> = {
  nw: 'nwse-resize',
  n: 'ns-resize',
  ne: 'nesw-resize',
  e: 'ew-resize',
  se: 'nwse-resize',
  s: 'ns-resize',
  sw: 'nesw-resize',
  w: 'ew-resize',
};

function handlePos(b: Rect, h: Handle): [number, number] {
  const cx = b.x + b.w / 2;
  const cy = b.y + b.h / 2;
  const map: Record<Handle, [number, number]> = {
    nw: [b.x, b.y],
    n: [cx, b.y],
    ne: [b.x + b.w, b.y],
    e: [b.x + b.w, cy],
    se: [b.x + b.w, b.y + b.h],
    s: [cx, b.y + b.h],
    sw: [b.x, b.y + b.h],
    w: [b.x, cy],
  };
  return map[h];
}

export function Overlay({
  doc,
  ab,
  sel,
  hover,
  zoom,
  editing,
  setEditing,
}: {
  doc: Document;
  ab: Artboard;
  sel: string[];
  hover: string | null;
  zoom: number;
  editing: boolean;
  setEditing: (v: boolean) => void;
}) {
  const { state: app, dispatch, mutate, select, toast } = useApp();
  const svgRef = useRef<SVGSVGElement | null>(null);
  const ghostRef = useRef<SVGGElement | null>(null);
  const frameRef = useRef<SVGGElement | null>(null);
  const guideV = useRef<SVGLineElement | null>(null);
  const guideH = useRef<SVGLineElement | null>(null);
  const labelRef = useRef<HTMLDivElement | null>(null);
  const [marquee, setMarquee] = useState<Rect | null>(null);

  const byId = useMemo(() => nodeMap(doc), [doc]);
  const order = useMemo(() => paintOrder(doc, ab), [doc, ab]);
  const others = useMemo(
    () => order.map((i) => byId.get(i)).filter((n): n is Node => !!n && !sel.includes(n.id)),
    [order, byId, sel],
  );
  const selNodes = useMemo(
    () => sel.map((i) => byId.get(i)).filter((n): n is Node => !!n && n.artboard === ab.id),
    [sel, byId, ab.id],
  );

  const box = useMemo<Rect | null>(() => {
    if (!selNodes.length) return null;
    const x = Math.min(...selNodes.map((n) => n.box.x));
    const y = Math.min(...selNodes.map((n) => n.box.y));
    const x2 = Math.max(...selNodes.map((n) => n.box.x + n.box.w));
    const y2 = Math.max(...selNodes.map((n) => n.box.y + n.box.h));
    return { x, y, w: x2 - x, h: y2 - y };
  }, [selNodes]);

  const single = selNodes.length === 1 ? selNodes[0] : null;
  const locked = selNodes.some((n) => n.locked);
  const hs = 9 / zoom;

  /* pointer -> document coordinates, corrected for the world transform */
  const toDoc = useCallback(
    (cx: number, cy: number) => {
      const el = svgRef.current;
      if (!el) return { x: 0, y: 0, k: 1 };
      const r = el.getBoundingClientRect();
      const k = r.width / ab.width || 1;
      return { x: (cx - r.left) / k, y: (cy - r.top) / k, k };
    },
    [ab.width],
  );

  const showLabel = (text: string, x: number, y: number) => {
    const el = labelRef.current;
    if (!el) return;
    el.textContent = text;
    el.style.left = `${x}px`;
    el.style.top = `${y}px`;
    el.style.opacity = '1';
  };
  const hideLabel = () => {
    if (labelRef.current) labelRef.current.style.opacity = '0';
  };

  const setGuide = (v: number | null, h: number | null) => {
    const gv = guideV.current;
    const gh = guideH.current;
    if (gv) {
      gv.style.display = v === null ? 'none' : '';
      if (v !== null) {
        gv.setAttribute('x1', String(v));
        gv.setAttribute('x2', String(v));
      }
    }
    if (gh) {
      gh.style.display = h === null ? 'none' : '';
      if (h !== null) {
        gh.setAttribute('y1', String(h));
        gh.setAttribute('y2', String(h));
      }
    }
  };

  /* ------------------------------------------------------------ snapping */
  const snapTargets = useMemo(() => {
    const xs = [0, ab.width / 2, ab.width];
    const ys = [0, ab.height / 2, ab.height];
    for (const n of others) {
      xs.push(n.box.x, n.box.x + n.box.w / 2, n.box.x + n.box.w);
      ys.push(n.box.y, n.box.y + n.box.h / 2, n.box.y + n.box.h);
    }
    return { xs, ys };
  }, [others, ab.width, ab.height]);

  const snapDelta = (base: Rect, dx: number, dy: number, tol: number) => {
    let bestX: { d: number; at: number } | null = null;
    let bestY: { d: number; at: number } | null = null;
    const x = base.x + dx;
    const y = base.y + dy;
    for (const t of snapTargets.xs) {
      for (const [edge, off] of [
        [x, 'x'],
        [x + base.w / 2, 'cx'],
        [x + base.w, 'x2'],
      ] as [number, string][]) {
        const d = t - edge;
        if (Math.abs(d) <= tol && (!bestX || Math.abs(d) < Math.abs(bestX.d))) bestX = { d, at: t };
        void off;
      }
    }
    for (const t of snapTargets.ys) {
      for (const edge of [y, y + base.h / 2, y + base.h]) {
        const d = t - edge;
        if (Math.abs(d) <= tol && (!bestY || Math.abs(d) < Math.abs(bestY.d))) bestY = { d, at: t };
      }
    }
    return { dx: dx + (bestX?.d ?? 0), dy: dy + (bestY?.d ?? 0), vx: bestX?.at ?? null, hy: bestY?.at ?? null };
  };

  /* -------------------------------------------------------------- commit */
  const commitBoxes = useCallback(
    (items: { id: string; box: Rect; rot?: number }[]) => {
      if (!items.length) return;
      if (items.length === 1) {
        const it = items[0];
        void mutate('canvas.transform', { id: it.id, x: it.box.x, y: it.box.y, w: it.box.w, h: it.box.h, ...(it.rot === undefined ? {} : { rot: it.rot }) }).catch(() => undefined);
        return;
      }
      void mutate('canvas.batch', {
        cmds: items.map((it) => ({ t: 'Transform', id: it.id, x: it.box.x, y: it.box.y, w: it.box.w, h: it.box.h })),
      }).catch(() => undefined);
    },
    [mutate],
  );

  /* ------------------------------------------------------------- gestures */
  const dragMove = (e: React.PointerEvent, mode: 'move' | 'resize' | 'rotate', handle?: Handle) => {
    e.stopPropagation();
    if (mode === 'move' && locked) {
      toast('info', '该图层已锁定，先解锁再拖动');
      return;
    }
    const start = toDoc(e.clientX, e.clientY);
    const startBoxes = selNodes.map((n) => ({ id: n.id, box: { ...n.box }, rot: n.rotation }));
    const base = box ?? startBoxes[0]?.box ?? { x: 0, y: 0, w: 0, h: 0 };
    /* 设置 · 界面吸附 off → tolerance 0, i.e. the pointer moves 1:1 */
    const tol = app.prefs.snapping ? 7 / start.k : 0;
    const g = ghostRef.current;
    if (g && mode === 'move') g.setAttribute('transform', 'translate(0 0)');
    if (mode !== 'move') setEditing(false);

    const move = (ev: PointerEvent) => {
      const p = toDoc(ev.clientX, ev.clientY);
      const dx = p.x - start.x;
      const dy = p.y - start.y;
      if (mode === 'move') {
        const s = snapDelta(base, dx, dy, tol);
        if (g) g.setAttribute('transform', `translate(${s.dx.toFixed(2)} ${s.dy.toFixed(2)})`);
        setGuide(s.vx, s.hy);
        showLabel(`X ${Math.round(base.x + s.dx)}  Y ${Math.round(base.y + s.dy)}`, cx(base) + s.dx, base.y + s.dy);
      } else if (mode === 'rotate' && single) {
        const cx = single.box.x + single.box.w / 2;
        const cy = single.box.y + single.box.h / 2;
        let ang = (Math.atan2(p.y - cy, p.x - cx) * 180) / Math.PI + 90;
        if (ev.shiftKey) ang = Math.round(ang / 15) * 15;
        const norm = round(((ang + 180) % 360 + 360) % 360 - 180, 1);
        if (frameRef.current) frameRef.current.setAttribute('transform', `rotate(${norm} ${cx} ${cy})`);
        showLabel(`${norm}°`, cx, cy);
      } else if (handle && single) {
        const b0 = single.box;
        let nx = b0.x;
        let ny = b0.y;
        let nw = b0.w;
        let nh = b0.h;
        if (handle.includes('e')) nw = Math.max(4, b0.w + dx);
        if (handle.includes('s')) nh = Math.max(4, b0.h + dy);
        if (handle.includes('w')) {
          nw = Math.max(4, b0.w - dx);
          nx = b0.x + b0.w - nw;
        }
        if (handle.includes('n')) {
          nh = Math.max(4, b0.h - dy);
          ny = b0.y + b0.h - nh;
        }
        if (ev.shiftKey && handle.length === 2) {
          const k = Math.max(nw / b0.w, nh / b0.h);
          nw = b0.w * k;
          nh = b0.h * k;
          if (handle.includes('w')) nx = b0.x + b0.w - nw;
          if (handle.includes('n')) ny = b0.y + b0.h - nh;
        }
        if (frameRef.current) frameRef.current.setAttribute('transform', `translate(0 0) scale(1)`);
        const rect = frameRef.current?.querySelector('rect');
        if (rect) {
          rect.setAttribute('x', String(nx));
          rect.setAttribute('y', String(ny));
          rect.setAttribute('width', String(nw));
          rect.setAttribute('height', String(nh));
        }
        showLabel(`${Math.round(nw)} × ${Math.round(nh)}`, nx + nw / 2, ny + nh);
      }
    };

    const up = (ev: PointerEvent) => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      setGuide(null, null);
      hideLabel();
      if (frameRef.current) frameRef.current.removeAttribute('transform');
      if (g) g.setAttribute('transform', 'translate(0 0)');
      const p = toDoc(ev.clientX, ev.clientY);
      const dx = p.x - start.x;
      const dy = p.y - start.y;
      if (mode === 'move') {
        const s = snapDelta(base, dx, dy, tol);
        if (Math.abs(s.dx) < 0.5 && Math.abs(s.dy) < 0.5) return;
        commitBoxes(startBoxes.map((n) => ({ id: n.id, box: { ...n.box, x: n.box.x + s.dx, y: n.box.y + s.dy } })));
      } else if (mode === 'rotate' && single) {
        const cx = single.box.x + single.box.w / 2;
        const cy = single.box.y + single.box.h / 2;
        let ang = (Math.atan2(p.y - cy, p.x - cx) * 180) / Math.PI + 90;
        if (ev.shiftKey) ang = Math.round(ang / 15) * 15;
        const norm = round(((ang + 180) % 360 + 360) % 360 - 180, 1);
        if (Math.abs(norm - single.rotation) < 0.2) return;
        commitBoxes([{ id: single.id, box: { ...single.box }, rot: norm }]);
      } else if (handle && single) {
        const b0 = single.box;
        const nx = handle.includes('w') ? Math.min(b0.x + b0.w - 4, b0.x + dx) : b0.x;
        const ny = handle.includes('n') ? Math.min(b0.y + b0.h - 4, b0.y + dy) : b0.y;
        const nw = Math.max(4, handle.includes('e') ? b0.w + dx : handle.includes('w') ? b0.x + b0.w - nx : b0.w);
        const nh = Math.max(4, handle.includes('s') ? b0.h + dy : handle.includes('n') ? b0.y + b0.h - ny : b0.h);
        if (Math.abs(nw - b0.w) < 0.5 && Math.abs(nh - b0.h) < 0.5 && Math.abs(nx - b0.x) < 0.5 && Math.abs(ny - b0.y) < 0.5) return;
        commitBoxes([{ id: single.id, box: { x: nx, y: ny, w: nw, h: nh } }]);
      }
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };

  const pick = (e: React.PointerEvent, n: Node) => {
    e.stopPropagation();
    const next = e.shiftKey
      ? sel.includes(n.id)
        ? sel.filter((x) => x !== n.id)
        : [...sel, n.id]
      : sel.includes(n.id) && sel.length === 1
        ? sel
        : [n.id];
    select(next);
    if (n.locked) return;
    dragMove(e, 'move');
  };

  /* ------------------------------------------------------------- marquee */
  const startMarquee = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    const origin = toDoc(e.clientX, e.clientY);
    select([]);
    setMarquee({ x: origin.x, y: origin.y, w: 0, h: 0 });
    const move = (ev: PointerEvent) => {
      const p = toDoc(ev.clientX, ev.clientY);
      setMarquee({
        x: Math.min(origin.x, p.x),
        y: Math.min(origin.y, p.y),
        w: Math.abs(p.x - origin.x),
        h: Math.abs(p.y - origin.y),
      });
    };
    const up = (ev: PointerEvent) => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      const p = toDoc(ev.clientX, ev.clientY);
      const r: Rect = {
        x: Math.min(origin.x, p.x),
        y: Math.min(origin.y, p.y),
        w: Math.abs(p.x - origin.x),
        h: Math.abs(p.y - origin.y),
      };
      setMarquee(null);
      if (r.w < 3 && r.h < 3) return;
      const hit = order
        .map((i) => byId.get(i))
        .filter((n): n is Node => {
          if (!n || !n.visible) return false;
          const b = n.box;
          return b.x < r.x + r.w && r.x < b.x + b.w && b.y < r.y + r.h && r.y < b.y + b.h;
        })
        .map((n) => n.id);
      if (hit.length) select(hit);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };

  /* --------------------------------------------------------- inline text */
  const startEdit = (n: Node) => {
    if (n.type !== 'text' || n.locked) return;
    select([n.id]);
    setEditing(true);
  };

  const tb = single?.type === 'text' && single.text ? single : null;
  const tsize = tb?.text?.size ?? 16;
  const tlineH = (tb?.text?.line_ratio ?? 1.35) * tsize;

  return (
    <>
      <svg ref={svgRef} className="overlay" viewBox={`0 0 ${ab.width} ${ab.height}`} width="100%" height="100%">
        {/* backdrop: marquee + deselect */}
        <rect x={0} y={0} width={ab.width} height={ab.height} fill="transparent" className="ov-hit" onPointerDown={startMarquee} />

        {/* per-node hit areas, in paint order so the topmost wins */}
        <g ref={ghostRef}>
          {order.map((i) => byId.get(i)).filter((n): n is Node => !!n && n.visible).map((n) => {
            const cx = n.box.x + n.box.w / 2;
            const cy = n.box.y + n.box.h / 2;
            const isSel = sel.includes(n.id);
            return (
              <g key={n.id} transform={n.rotation ? `rotate(${n.rotation} ${cx} ${cy})` : undefined}>
                <rect
                  className={`ov-node ${n.locked ? 'is-locked' : ''}`}
                  x={n.box.x}
                  y={n.box.y}
                  width={n.box.w}
                  height={Math.max(n.box.h, 2 / zoom)}
                  strokeWidth={1.2 / zoom}
                  stroke={hover === n.id && !isSel ? 'rgba(75,107,251,.65)' : 'transparent'}
                  onPointerDown={(e) => pick(e, n)}
                  onPointerEnter={() => dispatch({ t: 'hover', hover: n.id })}
                  onPointerLeave={() => dispatch({ t: 'hover', hover: null })}
                  onDoubleClick={() => startEdit(n)}
                />
              </g>
            );
          })}

          {/* live ghost while dragging */}
          {selNodes.map((n) => (
            <rect
              key={`gh-${n.id}`}
              className="ov-ghost"
              x={n.box.x}
              y={n.box.y}
              width={n.box.w}
              height={n.box.h}
              strokeWidth={1.5 / zoom}
              style={{ display: selNodes.length ? '' : 'none' }}
              transform={n.rotation ? `rotate(${n.rotation} ${n.box.x + n.box.w / 2} ${n.box.y + n.box.h / 2})` : undefined}
              pointerEvents="none"
            />
          ))}
        </g>

        {/* safe area + authored guides */}
        {ab.safe_margin && app.prefs.safeArea ? (
          <rect
            className="ov-safe"
            x={ab.safe_margin}
            y={ab.safe_margin}
            width={Math.max(0, ab.width - ab.safe_margin * 2)}
            height={Math.max(0, ab.height - ab.safe_margin * 2)}
            strokeWidth={1 / zoom}
            strokeDasharray={`${7 / zoom} ${5 / zoom}`}
          />
        ) : null}
        {(ab.guides ?? []).map((g, i) =>
          g.axis === 'v' ? (
            <line key={i} className="ov-dim" x1={g.pos} y1={0} x2={g.pos} y2={ab.height} strokeWidth={1 / zoom} strokeDasharray={`${5 / zoom} ${5 / zoom}`} />
          ) : (
            <line key={i} className="ov-dim" x1={0} y1={g.pos} x2={ab.width} y2={g.pos} strokeWidth={1 / zoom} strokeDasharray={`${5 / zoom} ${5 / zoom}`} />
          ),
        )}

        {/* selection frame + handles */}
        {box ? (
          <g ref={frameRef}>
            <rect
              className="ov-frame"
              x={box.x}
              y={box.y}
              width={box.w}
              height={box.h}
              strokeWidth={1.5 / zoom}
              style={{ pointerEvents: 'none' }}
            />
            {selNodes.length > 1
              ? selNodes.map((n) => (
                  <rect
                    key={n.id}
                    className="ov-multi"
                    x={n.box.x}
                    y={n.box.y}
                    width={n.box.w}
                    height={n.box.h}
                    strokeWidth={1 / zoom}
                    transform={n.rotation ? `rotate(${n.rotation} ${cx(n.box)} ${cy(n.box)})` : undefined}
                    style={{ pointerEvents: 'none' }}
                  />
                ))
              : null}
            {!locked && single
              ? (Object.keys(CURSOR) as Handle[]).map((h) => {
                  const [hx, hy] = handlePos(single.box, h);
                  return (
                    <rect
                      key={h}
                      className="ov-handle"
                      x={hx - hs / 2}
                      y={hy - hs / 2}
                      width={hs}
                      height={hs}
                      rx={hs * 0.22}
                      strokeWidth={1.5 / zoom}
                      style={{ cursor: CURSOR[h] }}
                      transform={single.rotation ? `rotate(${single.rotation} ${single.box.x + single.box.w / 2} ${single.box.y + single.box.h / 2})` : undefined}
                      onPointerDown={(e) => dragMove(e, 'resize', h)}
                    />
                  );
                })
              : null}
            {!locked && single ? (
              <>
                <line
                  className="ov-frame"
                  x1={cx(single.box)}
                  y1={single.box.y}
                  x2={cx(single.box)}
                  y2={single.box.y - 26 / zoom}
                  strokeWidth={1.2 / zoom}
                  style={{ pointerEvents: 'none' }}
                />
                <circle
                  className="ov-rot"
                  cx={cx(single.box)}
                  cy={single.box.y - 32 / zoom}
                  r={hs * 0.62}
                  strokeWidth={1.5 / zoom}
                  style={{ cursor: 'grab' }}
                  onPointerDown={(e) => dragMove(e, 'rotate')}
                />
              </>
            ) : null}
          </g>
        ) : null}

        {marquee ? <rect className="ov-marquee" x={marquee.x} y={marquee.y} width={marquee.w} height={marquee.h} strokeWidth={1 / zoom} /> : null}

        <line ref={guideV} className="ov-guide" x1={0} y1={0} x2={0} y2={ab.height} style={{ display: 'none' }} />
        <line ref={guideH} className="ov-guide" x1={0} y1={0} x2={ab.width} y2={0} style={{ display: 'none' }} />
      </svg>

      <div ref={labelRef} className="ov-label" style={{ opacity: 0, transform: `translate(-50%,-140%) scale(${1 / zoom})`, transformOrigin: 'left top' }} />

      {tb && editing ? (
        <textarea
          className="text-edit"
          autoFocus
          defaultValue={tb.text?.content ?? ''}
          style={{
            left: tb.box.x,
            top: tb.box.y,
            width: tb.box.w,
            height: Math.max(tb.box.h, tlineH),
            fontSize: tsize,
            lineHeight: tlineH,
            letterSpacing: tb.text?.letter_spacing ?? 0,
            fontFamily: tb.text?.family ?? 'inherit',
            fontWeight: tb.text?.weight ?? 400,
            textAlign: (tb.text?.align ?? 'left') as React.CSSProperties['textAlign'],
          }}
          onFocus={(e) => e.currentTarget.select()}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault();
              void mutate('canvas.text', { id: tb.id, content: e.currentTarget.value }).catch(() => undefined);
              setEditing(false);
            }
            if (e.key === 'Escape') setEditing(false);
          }}
          onBlur={(e) => {
            if (e.currentTarget.value !== (tb.text?.content ?? '')) {
              void mutate('canvas.text', { id: tb.id, content: e.currentTarget.value }).catch(() => undefined);
            }
            setEditing(false);
          }}
        />
      ) : null}
    </>
  );
}
