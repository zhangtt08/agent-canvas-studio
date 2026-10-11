/** Frameless window chrome. The OS frame is gone (`decorations(false)`), so these
 *  circular glass chips are the only window controls — they use the same material,
 *  radius and hover language as every other control in the app.
 *
 *  The bar itself is the drag region, and so is every non-interactive child. */

import { useEffect, useState } from 'react';
import { isTauri } from '../api';
import { useApp, type Route } from '../state/app';
import { Icon } from './Icons';
import { Badge, Seg } from './Primitives';

interface Win {
  minimize(): Promise<void>;
  toggleMaximize(): Promise<void>;
  close(): Promise<void>;
  isMaximized(): Promise<boolean>;
  startResizeDragging?(dir: string): Promise<void>;
}

let cached: Win | null = null;

async function win(): Promise<Win | null> {
  if (!isTauri) return null;
  if (cached) return cached;
  try {
    const m = await import('@tauri-apps/api/webviewWindow');
    cached = m.getCurrentWebviewWindow() as unknown as Win;
    return cached;
  } catch {
    return null;
  }
}

const ROUTES: { v: Route; label: string }[] = [
  { v: 'home', label: '首页' },
  { v: 'studio', label: '工作室' },
  { v: 'lab', label: '图片处理' },
  { v: 'access', label: 'Agent 接入' },
];

const SAVE_LABEL: Record<string, { text: string; tone: 'default' | 'blue' | 'ok' | 'warn' }> = {
  idle: { text: '未打开项目', tone: 'default' },
  dirty: { text: '有改动', tone: 'warn' },
  saving: { text: '保存中', tone: 'blue' },
  saved: { text: '已保存', tone: 'ok' },
};

export function TitleBar() {
  const { state, go, save } = useApp();
  const [max, setMax] = useState(false);

  /* maximised: square the slab and drop the resize ring */
  useEffect(() => {
    const root = document.documentElement;
    root.setAttribute('data-max', max ? '1' : '0');
  }, [max]);

  useEffect(() => {
    if (isTauri) {
      let dead = false;
      const poll = () => {
        void (async () => {
          const w = await win();
          if (!w || dead) return;
          setMax(await w.isMaximized().catch(() => false));
        })();
      };
      poll();
      const t = window.setInterval(poll, 900);
      return () => {
        dead = true;
        window.clearInterval(t);
      };
    }
    const onFs = () => setMax(Boolean(document.fullscreenElement));
    document.addEventListener('fullscreenchange', onFs);
    return () => document.removeEventListener('fullscreenchange', onFs);
  }, []);

  const act = (fn: (w: Win) => Promise<void>) => {
    void (async () => {
      const w = await win();
      if (w) await fn(w).catch(() => undefined);
    })();
  };

  const named = state.doc?.name ?? null;
  const save0 = SAVE_LABEL[state.save] ?? SAVE_LABEL.idle;

  return (
    <div className="tbar" data-tauri-drag-region>
      <div className="tbar-brand" data-tauri-drag-region>
        <span className="brand-mark" data-tauri-drag-region>
          <Icon name="sparkle" size={14} sw={1.9} />
        </span>
        <span data-tauri-drag-region>AgentCanvas</span>
      </div>

      <div className="tbar-ctx" data-tauri-drag-region>
        {state.route === 'studio' && named ? (
          <>
            <b data-tauri-drag-region>{named}</b>
            <Badge tone={save0.tone}>{save0.text}</Badge>
            <span className="num" data-tauri-drag-region>
              rev {state.rev}
            </span>
          </>
        ) : (
          <span data-tauri-drag-region>
            {state.route === 'lab' ? '位图链式处理' : state.route === 'access' ? '本地 API · MCP' : '画布与排版引擎'}
          </span>
        )}
        {state.route === 'studio' && named ? (
          <button type="button" className="li-x" onClick={() => void save()} data-tip="保存项目（Ctrl+S）" aria-label="保存">
            <Icon name="save" size={15} />
          </button>
        ) : null}
      </div>

      <div className="tbar-nav">
        <Seg options={ROUTES} value={state.route} onChange={go} tight />
      </div>

      <div className="wctl">
        <button
          type="button"
          className="wchip"
          aria-label="最小化"
          disabled={!isTauri}
          data-tip={isTauri ? '最小化' : '浏览器预览中由窗口外壳接管'}
          style={!isTauri ? { opacity: 0.5 } : undefined}
          onClick={() => act((w) => w.minimize())}
        >
          <Icon name="minimize" size={13} sw={1.7} />
        </button>
        <button
          type="button"
          className="wchip"
          aria-label={max ? '还原' : '最大化'}
          disabled={!isTauri}
          data-tip={isTauri ? (max ? '还原窗口' : '最大化窗口') : '浏览器预览中由窗口外壳接管'}
          style={!isTauri ? { opacity: 0.5 } : undefined}
          onClick={() =>
            act(async (w) => {
              await w.toggleMaximize();
              setMax((m) => !m);
            })
          }
        >
          <Icon name={max ? 'restore' : 'maximize'} size={13} sw={1.7} />
        </button>
        <button
          type="button"
          className="wchip wclose"
          aria-label="关闭"
          disabled={!isTauri}
          data-tip={isTauri ? '关闭窗口' : '浏览器预览中由窗口外壳接管'}
          style={!isTauri ? { opacity: 0.5 } : undefined}
          onClick={() => act((w) => w.close())}
        >
          <Icon name="close" size={13} sw={1.8} />
        </button>
      </div>

      <ResizeGrips />
    </div>
  );
}

/** 6px transparent inset ring: Windows still hit-tests the outer edge with
 *  `decorations(false)`, and `startResizeDragging` covers the webview side. */
function ResizeGrips() {
  const start = (dir: string) => {
    void (async () => {
      const w = await win();
      if (w?.startResizeDragging) await w.startResizeDragging(dir).catch(() => undefined);
    })();
  };
  const grip = (cls: string, dir: string) => (
    <div
      key={dir}
      className={`rz ${cls}`}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        start(dir);
      }}
    />
  );
  return (
    <>
      {grip('rz-n', 'North')}
      {grip('rz-s', 'South')}
      {grip('rz-e', 'East')}
      {grip('rz-w', 'West')}
      {grip('rz-ne', 'NorthEast')}
      {grip('rz-nw', 'NorthWest')}
      {grip('rz-sw', 'SouthWest')}
      {grip('rz-se', 'SouthEast')}
    </>
  );
}
