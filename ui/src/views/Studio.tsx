/** 工作室 / Studio: rail + canvas + inspector + agent dock, with the project
 *  toolbar on top and all keyboard shortcuts wired to real methods. */

import { useEffect } from 'react';
import { useApp } from '../state/app';
import { isTauri } from '../api';
import type { Mut, RecentProject, V } from '../types';
import { Rail, SidePanel, BoardPicker } from '../components/Rail';
import { Canvas } from '../components/Canvas';
import { Inspector } from '../components/Inspector';
import { AgentPanel } from '../components/AgentPanel';
import { Glass } from '../components/Glass';
import { Badge, Btn, Empty, IconBtn } from '../components/Primitives';
import { Icon } from '../components/Icons';
import { nodeMap } from '../lib/doc';
import { round } from '../lib/format';

function StudioEmpty() {
  const { openSheet, read, loadProject } = useApp();
  const pick = async () => {
    const list = await read<RecentProject[]>('project.list', {});
    const first = list?.[0];
    if (first) await loadProject(first.p, { route: 'studio' });
  };
  return (
    <div className="page" style={{ display: 'grid', placeItems: 'center' }}>
      <Glass depth={2} className="card" style={{ width: 560 }} data-sheen>
        <Empty
          title="工作室还没有打开画布"
          action={
            <div className="row" style={{ gap: 8, flexWrap: 'wrap', justifyContent: 'center' }}>
              <Btn variant="primary" size="lg" icon="plus" onClick={() => openSheet('new')}>
                开始新画布
              </Btn>
              <Btn size="lg" icon="folder" onClick={() => void pick()}>
                打开最近的画布
              </Btn>
            </div>
          }
        />
      </Glass>
    </div>
  );
}

function StudioBar() {
  const { state, mutate, dispatch, openSheet, toast, save } = useApp();
  const doc = state.doc!;
  return (
    <div className="studio-bar">
      <IconBtn icon="undo" size="sm" tip="撤销 (⌘Z)" label="撤销" onClick={() => void mutate('history.undo', { n: 1 }).catch(() => undefined)} />
      <IconBtn icon="redo" size="sm" tip="重做 (⌘⇧Z)" label="重做" onClick={() => void mutate('history.redo', { n: 1 }).catch(() => undefined)} />
      <IconBtn
        icon="history"
        size="sm"
        tip="改动轨迹 · 人与 Agent 共用一条历史"
        label="历史"
        onClick={() => openSheet('history')}
      />
      <span style={{ width: 6 }} />
      <BoardPicker />
      <span style={{ width: 6 }} />
      <Btn
        size="sm"
        icon="save"
        onClick={async () => {
          await save();
        }}
        tip="保存项目 (⌘S)"
      >
        保存
      </Btn>
      <Btn
        size="sm"
        icon="pin"
        variant="quiet"
        onClick={() =>
          void mutate<{ snap?: string }>('project.snapshot', { label: '手动快照' })
            .then((r) => toast('ok', `快照已建 · ${r?.snap ?? ''}`))
            .catch(() => undefined)
        }
        tip="给当前修订打个快照"
      >
        快照
      </Btn>
      <span className="spacer" />
      <Btn
        size="sm"
        variant="quiet"
        icon="wand"
        onClick={() =>
          void mutate<{ conflicts?: unknown[]; changed?: string[] }>('layout.resolve', { b: state.board })
            .then((r) => toast('ok', `已重排 ${(r?.changed ?? []).length} 个图层`))
            .catch(() => undefined)
        }
        tip="按语义意图重排本板 layout.resolve"
      >
        求解
      </Btn>
      <Btn
        size="sm"
        variant="quiet"
        icon="check"
        onClick={() => {
          dispatch({ t: 'right-tab', tab: 'inspect' });
          void mutate<{ issues?: V[] }>('inspect.validate', { b: state.board, contrast: 1, lowres: 1 })
            .then((r) => {
              const list = (r?.issues ?? []) as never[];
              dispatch({ t: 'issues', issues: list, board: state.board });
              toast(list.length ? 'info' : 'ok', list.length ? `本板 ${list.length} 个问题` : '本板没有发现问题');
            })
            .catch(() => undefined);
        }}
        tip="检查并在右侧列出问题"
      >
        检查
      </Btn>
      <Btn size="sm" variant="primary" icon="export" onClick={() => openSheet('export')}>
        导出
      </Btn>
      <span className="badge" style={{ marginLeft: 6 }}>
        <Icon name="layers" size={13} />
        {doc.nodes.length}
      </span>
      <Badge tone={state.save === 'dirty' ? 'warn' : state.save === 'saving' ? 'blue' : 'ok'}>
        {state.save === 'dirty' ? '有改动' : state.save === 'saving' ? '保存中' : '已同步'}
      </Badge>
    </div>
  );
}

export function Studio() {
  const { state, mutate, select, save, toast } = useApp();
  const doc = state.doc;

  /* keyboard: the same methods the toolbar uses */
  useEffect(() => {
    if (!doc) return;
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      const typing = t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable);
      const mod = e.metaKey || e.ctrlKey;

      if (mod && e.key.toLowerCase() === 's') {
        e.preventDefault();
        void save();
        return;
      }
      if (typing) return;

      if (mod && e.key.toLowerCase() === 'z') {
        e.preventDefault();
        void mutate(e.shiftKey ? 'history.redo' : 'history.undo', { n: 1 }).catch(() => undefined);
        return;
      }
      if (mod && e.key.toLowerCase() === 'd') {
        e.preventDefault();
        if (state.sel.length)
          void mutate<{ ids?: string[] }>('canvas.duplicate', { ids: state.sel })
            .then((r) => r?.ids && select(r.ids))
            .catch(() => undefined);
        return;
      }
      if ((e.key === 'Delete' || e.key === 'Backspace') && state.sel.length) {
        e.preventDefault();
        void mutate('canvas.delete', { ids: state.sel })
          .then(() => select([]))
          .catch(() => undefined);
        return;
      }
      if (e.key === 'Escape') {
        select([]);
        return;
      }
      if (['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight'].includes(e.key) && state.sel.length) {
        e.preventDefault();
        const step = e.shiftKey ? 10 : 1;
        const dx = e.key === 'ArrowLeft' ? -step : e.key === 'ArrowRight' ? step : 0;
        const dy = e.key === 'ArrowUp' ? -step : e.key === 'ArrowDown' ? step : 0;
        const byId = nodeMap(doc);
        const cmds = state.sel
          .map((id) => byId.get(id))
          .filter((n) => !!n && !n.locked)
          .map((n) => ({ t: 'Transform', id: n!.id, x: round(n!.box.x + dx, 1), y: round(n!.box.y + dy, 1), w: n!.box.w, h: n!.box.h }));
        if (!cmds.length) return;
        void (cmds.length === 1
          ? mutate('canvas.transform', { id: cmds[0].id, x: cmds[0].x, y: cmds[0].y, w: cmds[0].w, h: cmds[0].h })
          : mutate('canvas.batch', { cmds })
        ).catch(() => undefined);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [doc, mutate, select, save, toast]);

  /* Tauri hands us real file paths on drop, which is exactly what `asset.import` wants */
  useEffect(() => {
    if (!isTauri || !doc) return;
    let un: (() => void) | null = null;
    void (async () => {
      try {
        const m = await import('@tauri-apps/api/webview');
        const wv = m.getCurrentWebview();
        un = await wv.onDragDropEvent((ev: { payload?: { type?: string; paths?: string[] } }) => {
          const paths = ev?.payload?.paths ?? [];
          if (ev?.payload?.type === 'drop' && paths.length) {
            void mutate<Mut>('asset.import', { paths, mode: 'copy', dedupe: 1 }).then((r) => {
              if (r) toast('ok', `已导入 ${paths.length} 张图`);
            });
          }
        });
      } catch {
        /* older api surface: the 素材 panel file picker still works */
      }
    })();
    return () => {
      un?.();
    };
  }, [doc, mutate, toast]);

  if (!doc) return <StudioEmpty />;

  return (
    <div className="studio">
      <StudioBar />
      <div className="studio-grid">
        <div className="side glass glass-2" data-sheen>
          <Rail />
          <SidePanel doc={doc} />
        </div>
        <div className="stage-wrap glass glass-dark" data-sheen>
          <Canvas />
          <AgentPanel />
        </div>
        <Inspector />
      </div>
    </div>
  );
}
