/** Errors and confirmations surface here — top-centre glass, auto-dismissing, with
 *  the machine code as a pill. The app never prints inline fine print. */

import { useApp } from '../state/app';
import { Icon } from './Icons';

const TONE: Record<string, { icon: 'check' | 'close' | 'info'; color: string }> = {
  ok: { icon: 'check', color: 'var(--ok)' },
  err: { icon: 'close', color: 'var(--err)' },
  info: { icon: 'info', color: 'var(--blue)' },
};

export function Toasts() {
  const { state, dispatch } = useApp();
  if (!state.toasts.length) return null;
  return (
    <div className="toasts">
      {state.toasts.map((t) => {
        const tone = TONE[t.kind] ?? TONE.info;
        return (
          <div key={t.id} className="toast glass glass-3" data-sheen>
            <span style={{ color: tone.color, display: 'grid', placeItems: 'center' }}>
              <Icon name={tone.icon} size={15} sw={2} />
            </span>
            <span className="toast-t">{t.text}</span>
            {t.code ? <span className="badge badge-blue mono">{t.code}</span> : null}
            <button
              type="button"
              className="li-x"
              aria-label="关闭提示"
              onClick={() => dispatch({ t: 'untoast', id: t.id })}
            >
              <Icon name="close" size={12} sw={2} />
            </button>
          </div>
        );
      })}
    </div>
  );
}
