/** Hand-written control primitives. Everything interactive in the app is built
 *  from these, so the hover language and blur material stay identical from the
 *  window chips down to the gradient stop handles. */

import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Icon, type IconName } from './Icons';
import { copyText } from '../api';
import { clamp, num as fnum, round } from '../lib/format';

/* ------------------------------------------------------------------ buttons */

export interface BtnProps {
  children?: ReactNode;
  icon?: IconName;
  iconRight?: IconName;
  variant?: 'default' | 'primary' | 'quiet' | 'danger';
  size?: 'sm' | 'md' | 'lg';
  active?: boolean;
  disabled?: boolean;
  tip?: string;
  type?: 'button' | 'submit';
  onClick?: () => void;
  style?: React.CSSProperties;
}

export function Btn({
  children,
  icon,
  iconRight,
  variant = 'default',
  size = 'md',
  active,
  disabled,
  tip,
  type = 'button',
  onClick,
  style,
}: BtnProps) {
  const cls = [
    'btn',
    variant === 'primary' ? 'btn-primary' : '',
    variant === 'quiet' ? 'btn-quiet' : '',
    variant === 'danger' ? 'btn-danger' : '',
    size === 'sm' ? 'btn-sm' : '',
    size === 'lg' ? 'btn-lg' : '',
    active ? 'is-on' : '',
  ]
    .filter(Boolean)
    .join(' ');
  return (
    <button
      type={type}
      className={cls}
      disabled={disabled}
      onClick={onClick}
      data-tip={tip}
      style={style}
    >
      {icon ? <Icon name={icon} size={size === 'sm' ? 14 : 16} /> : null}
      {children}
      {iconRight ? <Icon name={iconRight} size={size === 'sm' ? 14 : 16} /> : null}
    </button>
  );
}

export function IconBtn({
  icon,
  tip,
  onClick,
  active,
  disabled,
  danger,
  size = 'md',
  label,
}: {
  icon: IconName;
  tip: string;
  onClick?: () => void;
  active?: boolean;
  disabled?: boolean;
  danger?: boolean;
  size?: 'sm' | 'md';
  label?: string;
}) {
  return (
    <button
      type="button"
      className={['btn', 'btn-icon', size === 'sm' ? 'btn-sm' : '', active ? 'is-on' : '', danger ? 'btn-danger' : '']
        .filter(Boolean)
        .join(' ')}
      onClick={onClick}
      disabled={disabled}
      data-tip={tip}
      aria-label={label ?? tip}
    >
      <Icon name={icon} size={size === 'sm' ? 14 : 16} />
    </button>
  );
}

/* ------------------------------------------------------------- chips/badges */

export function Chip({
  children,
  onClick,
  active,
  tip,
  dim,
  icon,
  onRemove,
}: {
  children: ReactNode;
  onClick?: () => void;
  active?: boolean;
  tip?: string;
  dim?: boolean;
  icon?: IconName;
  onRemove?: () => void;
}) {
  return (
    <span
      className={['chip', active ? 'is-on' : '', dim ? 'chip-dim' : ''].filter(Boolean).join(' ')}
      onClick={onClick}
      role={onClick ? 'button' : undefined}
      tabIndex={onClick ? 0 : undefined}
      data-tip={tip}
      onKeyDown={onClick ? (e) => e.key === 'Enter' && onClick() : undefined}
      style={onClick ? { cursor: 'pointer' } : undefined}
    >
      {icon ? <Icon name={icon} size={14} /> : null}
      {children}
      {onRemove ? (
        <button
          type="button"
          className="chip-x"
          aria-label="移除"
          onClick={(e) => {
            e.stopPropagation();
            onRemove();
          }}
        >
          <Icon name="close" size={11} sw={2} />
        </button>
      ) : null}
    </span>
  );
}

export function Badge({
  children,
  tone = 'default',
  dot,
}: {
  children: ReactNode;
  tone?: 'default' | 'blue' | 'ok' | 'warn' | 'err' | 'grad';
  dot?: string;
}) {
  const cls = {
    default: 'badge',
    blue: 'badge badge-blue',
    ok: 'badge badge-ok',
    warn: 'badge badge-warn',
    err: 'badge badge-err',
    grad: 'badge badge-grad',
  }[tone];
  return (
    <span className={cls}>
      {dot ? <i className="dot" style={{ background: dot }} /> : null}
      {children}
    </span>
  );
}

/* ---------------------------------------------------------------- segmented */

export interface SegOpt<T extends string> {
  v: T;
  label?: string;
  icon?: IconName;
  tip?: string;
}

export function Seg<T extends string>({
  options,
  value,
  onChange,
  grow,
  tight,
}: {
  options: SegOpt<T>[];
  value: T;
  onChange: (v: T) => void;
  grow?: boolean;
  tight?: boolean;
}) {
  return (
    <div className={['seg', grow ? 'grow' : '', tight ? 'seg-tight' : ''].filter(Boolean).join(' ')} role="tablist">
      {options.map((o) => (
        <button
          key={o.v}
          type="button"
          role="tab"
          aria-selected={o.v === value}
          className={o.v === value ? 'is-on' : ''}
          data-tip={o.tip}
          onClick={() => onChange(o.v)}
        >
          {o.icon ? <Icon name={o.icon} size={14} /> : null}
          {o.label}
        </button>
      ))}
    </div>
  );
}

/* ------------------------------------------------------------------- fields */

export function Field({ label, children, hint }: { label?: string; children: ReactNode; hint?: string }) {
  return (
    <div className="field">
      {label ? (
        <label data-tip={hint}>
          {label}
          {hint ? (
            <span style={{ marginLeft: 5, display: 'inline-flex', verticalAlign: '-2px', color: 'var(--t3)' }}>
              <Icon name="info" size={13} />
            </span>
          ) : null}
        </label>
      ) : null}
      {children}
    </div>
  );
}

export function TextInput({
  value,
  onChange,
  placeholder,
  tip,
  mono,
  onEnter,
  ariaLabel,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  tip?: string;
  mono?: boolean;
  onEnter?: (v: string) => void;
  ariaLabel?: string;
}) {
  const [local, setLocal] = useState(value);
  const focused = useRef(false);
  useEffect(() => {
    if (!focused.current) setLocal(value);
  }, [value]);
  return (
    <input
      className="inp"
      style={mono ? { fontFamily: 'var(--mono)', fontSize: 12.5 } : undefined}
      value={local}
      placeholder={placeholder}
      data-tip={tip}
      aria-label={ariaLabel}
      onFocus={() => {
        focused.current = true;
      }}
      onChange={(e) => setLocal(e.target.value)}
      onBlur={() => {
        focused.current = false;
        if (local !== value) onChange(local);
      }}
      onKeyDown={(e) => {
        if (e.key === 'Enter') {
          (e.target as HTMLInputElement).blur();
          onEnter?.(local);
          onChange(local);
        }
        if (e.key === 'Escape') {
          setLocal(value);
          (e.target as HTMLInputElement).blur();
        }
      }}
    />
  );
}

/** The inspector's value cell: a 12px monospace number with a unit prefix. */
export function NumVal({
  tag,
  value,
  onChange,
  min = -100000,
  max = 100000,
  step = 1,
  dp = 1,
  suffix,
}: {
  tag: string;
  value: number | undefined;
  onChange: (v: number) => void;
  min?: number;
  max?: number;
  step?: number;
  dp?: number;
  suffix?: string;
}) {
  const [local, setLocal] = useState<string>(fnum(value, dp));
  const focused = useRef(false);
  useEffect(() => {
    if (!focused.current) setLocal(fnum(value, dp));
  }, [value, dp]);

  const commit = (raw: string) => {
    const parsed = Number(raw.replace(/[^\d.\-]/g, ''));
    if (!Number.isFinite(parsed)) {
      setLocal(fnum(value, dp));
      return;
    }
    onChange(round(clamp(parsed, min, max), dp));
  };

  return (
    <span className="val" data-tip={`${tag}${suffix ? ` · ${suffix}` : ''} · 步进 ${step}`}>
      <i>{tag}</i>
      <input
        inputMode="decimal"
        value={local}
        onFocus={(e) => {
          focused.current = true;
          e.currentTarget.select();
        }}
        onChange={(e) => setLocal(e.target.value)}
        onBlur={(e) => {
          focused.current = false;
          commit(e.target.value);
        }}
        onKeyDown={(e) => {
          const el = e as unknown as React.KeyboardEvent<HTMLInputElement>;
          if (el.key === 'Enter') {
            commit(local);
            (e.target as HTMLInputElement).blur();
          }
          if (el.key === 'ArrowUp' || el.key === 'ArrowDown') {
            el.preventDefault();
            const dir = el.key === 'ArrowUp' ? 1 : -1;
            const mult = el.shiftKey ? 10 : 1;
            const next = round(clamp((Number(local) || 0) + dir * step * mult, min, max), dp);
            setLocal(fnum(next, dp));
            onChange(next);
          }
          if (el.key === 'Escape') setLocal(fnum(value, dp));
        }}
      />
    </span>
  );
}

export function Select<T extends string>({
  value,
  options,
  onChange,
  tip,
  ariaLabel,
}: {
  value: T;
  options: { v: T; label: string }[];
  onChange: (v: T) => void;
  tip?: string;
  ariaLabel?: string;
}) {
  return (
    <span className="sel-wrap" data-tip={tip}>
      <select
        className="inp"
        value={value}
        aria-label={ariaLabel}
        onChange={(e) => onChange(e.target.value as T)}
      >
        {options.map((o) => (
          <option key={o.v} value={o.v}>
            {o.label}
          </option>
        ))}
      </select>
      <Icon name="chevronDown" size={14} />
    </span>
  );
}

export function Range({
  value,
  min,
  max,
  step = 1,
  onChange,
  tip,
}: {
  value: number;
  min: number;
  max: number;
  step?: number;
  onChange: (v: number) => void;
  tip?: string;
}) {
  const fill = ((value - min) / (max - min || 1)) * 100;
  return (
    <input
      type="range"
      min={min}
      max={max}
      step={step}
      value={value}
      data-tip={tip}
      style={{ ['--fill' as string]: `${clamp(fill, 0, 100)}%` }}
      onChange={(e) => onChange(Number(e.target.value))}
    />
  );
}

export function Switch({ on, onChange, tip }: { on: boolean; onChange: (v: boolean) => void; tip?: string }) {
  return (
    <button
      type="button"
      className={['sw', on ? 'is-on' : ''].filter(Boolean).join(' ')}
      role="switch"
      aria-checked={on}
      data-tip={tip}
      onClick={() => onChange(!on)}
    />
  );
}

export function SwitchRow({
  label,
  on,
  onChange,
  tip,
}: {
  label: string;
  on: boolean;
  onChange: (v: boolean) => void;
  tip?: string;
}) {
  return (
    <div className="sw-row">
      <span>{label}</span>
      <Switch on={on} onChange={onChange} tip={tip} />
    </div>
  );
}

export function CheckBox({
  checked,
  onChange,
  label,
  tip,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: string;
  tip?: string;
}) {
  return (
    <label className="cbox" data-tip={tip}>
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      {label}
    </label>
  );
}

export function ColorBtn({
  color,
  onChange,
  tip,
  alpha,
}: {
  color: string;
  onChange: (c: string) => void;
  tip?: string;
  alpha?: boolean;
}) {
  return (
    <button
      type="button"
      className={`cbtn ${alpha ? 'ck' : ''}`}
      data-tip={tip ?? color}
      style={{ backgroundImage: color.startsWith('#') || color.startsWith('rgb') ? color : undefined, backgroundColor: color }}
    >
      <input
        type="color"
        value={/^#[0-9a-f]{6}$/i.test(color) ? color : '#000000'}
        onChange={(e) => onChange(e.target.value)}
        aria-label="取色"
      />
    </button>
  );
}

export function CopyBtn({
  text,
  label,
  onCopied,
  size = 'sm',
  tip = '复制到剪贴板',
}: {
  text: string;
  label?: string;
  onCopied?: (ok: boolean) => void;
  size?: 'sm' | 'md';
  tip?: string;
}) {
  const [done, setDone] = useState(false);
  return (
    <Btn
      size={size}
      variant={done ? 'primary' : 'default'}
      icon={done ? 'check' : 'copy'}
      tip={tip}
      onClick={async () => {
        const ok = await copyText(text);
        setDone(ok);
        onCopied?.(ok);
        window.setTimeout(() => setDone(false), 1500);
      }}
    >
      {label}
    </Btn>
  );
}

/* --------------------------------------------------------------- feedback */

export function Bar({ value, tone }: { value: number; tone?: 'done' | 'err' }) {
  return (
    <div className={['bar', tone === 'done' ? 'done' : '', tone === 'err' ? 'err' : ''].filter(Boolean).join(' ')}>
      <i style={{ width: `${clamp(value * 100, 0, 100)}%` }} />
    </div>
  );
}

export function Empty({
  title,
  action,
  icon = 'sparkle',
  note,
}: {
  title: string;
  action?: ReactNode;
  icon?: IconName;
  /** One short line of guidance. Rendered at 14px, never as a paragraph —
   *  anything longer belongs in a tooltip or a sheet. */
  note?: string;
}) {
  return (
    <div className="empty">
      <svg className="empty-ill" viewBox="0 0 92 72" fill="none" aria-hidden="true">
        <rect x="6" y="10" width="60" height="48" rx="10" fill="rgba(255,255,255,.7)" stroke="rgba(23,29,63,.16)" />
        <rect x="18" y="4" width="60" height="48" rx="10" fill="rgba(75,107,251,.14)" stroke="rgba(75,107,251,.4)" />
        <path d="M28 30h40M28 38h24" stroke="rgba(23,29,63,.28)" strokeWidth="2.4" strokeLinecap="round" />
        <circle cx="72" cy="20" r="6" fill="url(#eg)" />
        <defs>
          <linearGradient id="eg" x1="66" y1="14" x2="78" y2="26">
            <stop offset="0" stopColor="#4B6BFB" />
            <stop offset="1" stopColor="#8B5CF6" />
          </linearGradient>
        </defs>
      </svg>
      <h4>{title}</h4>
      {note ? <p className="empty-note">{note}</p> : null}
      <div className="row" style={{ justifyContent: 'center' }}>
        {action ?? (
          <span className="badge">
            <Icon name={icon} size={13} />
            还没有内容
          </span>
        )}
      </div>
    </div>
  );
}

/* ------------------------------------------------------------ context menu */

export interface MenuSpec {
  label: string;
  icon?: IconName;
  danger?: boolean;
  disabled?: boolean;
  kbd?: string;
  onClick: () => void;
}

export function GlassMenu({
  x,
  y,
  items,
  onClose,
}: {
  x: number;
  y: number;
  items: MenuSpec[];
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    window.addEventListener('pointerdown', onDown, true);
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('pointerdown', onDown, true);
      window.removeEventListener('keydown', onKey);
    };
  }, [onClose]);

  return (
    <div ref={ref} className="menu glass glass-3" style={{ left: Math.min(x, window.innerWidth - 210), top: Math.min(y, window.innerHeight - items.length * 34 - 20) }} data-sheen>
      {items.map((it, i) => (
        <button
          key={i}
          type="button"
          className={it.danger ? 'danger' : ''}
          disabled={it.disabled}
          style={it.disabled ? { opacity: 0.4, cursor: 'not-allowed' } : undefined}
          onClick={() => {
            if (it.disabled) return;
            it.onClick();
            onClose();
          }}
        >
          {it.icon ? <Icon name={it.icon} size={14} /> : <span style={{ width: 14 }} />}
          {it.label}
          {it.kbd ? <kbd>{it.kbd}</kbd> : null}
        </button>
      ))}
    </div>
  );
}
