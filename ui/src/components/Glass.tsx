/** The material layer: what sits behind the glass, the specular sheen that
 *  follows the pointer, the sheet scaffold and the tooltip host.
 *
 *  One `pointermove` listener at the root writes `--mx/--my` on the nearest
 *  `[data-sheen]` ancestor — no per-frame React state anywhere. */

import {
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from 'react';
import { Icon } from './Icons';

/* ------------------------------------------------------------------ backdrop */

export function Backdrop() {
  return (
    <>
      <div className="bg-field" aria-hidden="true">
        <i className="orb-1" />
        <i className="orb-2" />
        <i className="orb-3" />
        <i className="orb-4" />
      </div>
      <svg className="bg-noise" aria-hidden="true">
        <filter id="ac-noise">
          <feTurbulence type="fractalNoise" baseFrequency="0.82" numOctaves="3" stitchTiles="stitch" />
          <feColorMatrix type="saturate" values="0" />
        </filter>
        <rect width="100%" height="100%" filter="url(#ac-noise)" />
      </svg>
    </>
  );
}

/* --------------------------------------------------------------- sheen layer */

export function GlassLayer({ children }: { children: ReactNode }) {
  const root = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const el = root.current;
    if (!el) return;
    let last: Element | null = null;
    const onMove = (e: PointerEvent) => {
      const target = (e.target as Element | null)?.closest?.('[data-sheen]') ?? null;
      if (!target) {
        last = null;
        return;
      }
      if (target !== last) {
        if (last instanceof HTMLElement) last.removeAttribute('data-sheen-on');
        last = target;
      }
      const r = (target as HTMLElement).getBoundingClientRect();
      const x = e.clientX - r.left;
      const y = e.clientY - r.top;
      (target as HTMLElement).style.setProperty('--mx', `${x.toFixed(1)}px`);
      (target as HTMLElement).style.setProperty('--my', `${y.toFixed(1)}px`);
    };
    el.addEventListener('pointermove', onMove, { passive: true });
    return () => el.removeEventListener('pointermove', onMove);
  }, []);

  return (
    <div ref={root} className="sheen-root" style={{ height: '100%' }}>
      {children}
    </div>
  );
}

/* -------------------------------------------------------------------- panel */

export type Depth = 1 | 2 | 3;

export interface GlassProps {
  depth?: Depth;
  dark?: boolean;
  inset?: boolean;
  sheen?: boolean;
  lift?: boolean;
  className?: string;
  style?: CSSProperties;
  children?: ReactNode;
  onClick?: () => void;
  onContextMenu?: (e: React.MouseEvent) => void;
  role?: string;
  'data-tauri-drag-region'?: boolean;
}

export function Glass({
  depth = 2,
  dark,
  inset,
  sheen = true,
  lift,
  className = '',
  style,
  children,
  onClick,
  onContextMenu,
  role,
  ...rest
}: GlassProps) {
  const cls = ['glass', inset ? 'glass-inset' : `glass-${depth}`, dark ? 'glass-dark' : '', lift ? 'lift' : '', className]
    .filter(Boolean)
    .join(' ');
  return (
    <div
      className={cls}
      style={style}
      data-sheen={sheen ? '' : undefined}
      onClick={onClick}
      onContextMenu={onContextMenu}
      role={role}
      {...rest}
    >
      {children}
    </div>
  );
}

/* --------------------------------------------------------------------- sheet */

export interface SheetProps {
  title: string;
  eyebrow?: string;
  icon?: ReactNode;
  width?: number;
  onClose: () => void;
  footer?: ReactNode;
  children: ReactNode;
  extra?: ReactNode;
}

export function Sheet({ title, eyebrow, width = 720, onClose, footer, children, extra }: SheetProps) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  return (
    <div className="scrim" onPointerDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="sheet glass glass-3" style={{ ['--sheet-w' as string]: `${width}px` }} data-sheen>
        <div className="sheet-h">
          <div style={{ marginRight: 'auto' }}>
            {eyebrow ? <div className="badge badge-blue" style={{ marginBottom: 6 }}>{eyebrow}</div> : null}
            <h2 className="h2">{title}</h2>
          </div>
          {extra}
          <button type="button" className="btn btn-sm btn-icon btn-quiet" onClick={onClose} aria-label="关闭">
            <Icon name="close" />
          </button>
        </div>
        <div className="sheet-b">{children}</div>
        {footer ? <div className="sheet-f">{footer}</div> : null}
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------ tooltips */

interface TipState {
  text: string;
  x: number;
  y: number;
}

/** Hosts hover tooltips for any element carrying `data-tip`. This is what the app
 *  uses instead of small-print helper text. */
export function TipLayer() {
  const [tip, setTip] = useState<TipState | null>(null);

  useEffect(() => {
    let cur: Element | null = null;
    const show = (e: PointerEvent) => {
      const t = (e.target as Element | null)?.closest?.('[data-tip]');
      if (!t || t === cur) return;
      cur = t;
      const text = t.getAttribute('data-tip');
      if (!text) return;
      const r = t.getBoundingClientRect();
      const below = r.top < 60;
      setTip({ text, x: r.left + r.width / 2, y: below ? r.bottom + 8 : r.top - 8 });
    };
    const hide = () => {
      cur = null;
      setTip(null);
    };
    window.addEventListener('pointerover', show, true);
    window.addEventListener('pointerout', hide, true);
    window.addEventListener('scroll', hide, true);
    window.addEventListener('pointerdown', hide, true);
    return () => {
      window.removeEventListener('pointerover', show, true);
      window.removeEventListener('pointerout', hide, true);
      window.removeEventListener('scroll', hide, true);
      window.removeEventListener('pointerdown', hide, true);
    };
  }, []);

  if (!tip) return null;
  return (
    <div
      className="tip"
      style={{
        left: tip.x,
        top: tip.y,
        transform: 'translate(-50%, var(--tip-shift, -100%))',
        ['--tip-shift' as string]: tip.y < 40 ? '0' : '-100%',
      }}
    >
      {tip.text}
    </div>
  );
}
