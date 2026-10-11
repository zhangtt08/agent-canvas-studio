/** 首页 / Home: recent canvases with live engine previews, the primary action,
 *  size presets straight from `system.caps`, and the connection strip. */

import { useMemo } from 'react';
import { absUrl, MOCK, useRpc } from '../api';
import { useApp } from '../state/app';
import type { Preview, RecentProject, Summary } from '../types';
import { Badge, Btn, Chip, CopyBtn, Empty } from '../components/Primitives';
import { Glass } from '../components/Glass';
import { Icon, type IconName } from '../components/Icons';
import { timeAgo } from '../lib/format';

/** `render.svg` answers with a JSON string, so `useRpc<string>` hands back the
 *  quoted form — unwrap it before it reaches `dangerouslySetInnerHTML`. */
function unwrap(raw: unknown): string {
  if (typeof raw !== 'string') return '';
  if (raw.startsWith('"')) {
    try {
      const v = JSON.parse(raw) as unknown;
      if (typeof v === 'string') return v;
    } catch {
      /* not JSON after all — use it verbatim */
    }
  }
  return raw;
}

function ProjCard({ p, onOpen, onClose }: { p: RecentProject; onOpen: () => void; onClose: () => void }) {
  const prev = useRpc<Preview>('render.preview', { p: p.p, sc: 0.24 });
  /* `project.list` gives the board *count* only; the real pixel size needs
     `project.get depth:0`, which is a ~200 byte summary. */
  const head = useRpc<Summary>('project.get', { p: p.p, depth: 0 });
  const board0 = (head.data?.boards ?? [])[0];
  const boards = head.data?.boards?.length ?? p.boards ?? 0;
  return (
    <Glass depth={2} lift className="proj-card" onClick={onOpen} data-sheen>
      <div className="thumb">
        {prev.data ? (
          <img src={absUrl(prev.data.url)} alt={p.name} loading="lazy" />
        ) : prev.loading ? (
          <div className="skl" style={{ position: 'absolute', inset: 0 }} />
        ) : (
          <div style={{ position: 'absolute', inset: 0, display: 'grid', placeItems: 'center', color: 'var(--t3)' }}>
            <Icon name="image" size={22} />
          </div>
        )}
      </div>
      <div className="proj-card-b">
        <div style={{ minWidth: 0, flex: '1 1 auto' }}>
          <b>{p.name}</b>
          <div className="row" style={{ gap: 6, marginTop: 6, flexWrap: 'wrap' }}>
            <Badge tone="blue">
              {board0 ? `${Math.round(board0.w)}×${Math.round(board0.h)}` : `${p.layers ?? 0} 层`}
            </Badge>
            {boards > 1 ? <Badge>{boards} 个画板</Badge> : null}
            {p.updated ? (
              <span className="t3" style={{ fontSize: 13 }}>
                {timeAgo(String(p.updated))}
              </span>
            ) : null}
          </div>
        </div>
        <div className="row" style={{ gap: 2, opacity: 0.9 }}>
          <button
            type="button"
            className="li-x"
            aria-label="关闭项目"
            data-tip="关闭项目（从引擎内存中卸载）"
            onClick={(e) => {
              e.stopPropagation();
              onClose();
            }}
          >
            <Icon name="close" size={13} />
          </button>
          <button type="button" className="li-x" aria-label="打开" data-tip="在工作室中打开" onClick={onOpen}>
            <Icon name="chevron" size={14} />
          </button>
        </div>
      </div>
    </Glass>
  );
}

const FEATURES: { icon: IconName; k: string; v: string }[] = [
  { icon: 'layers', k: '一份场景图', v: '图层 · 画板 · 意图，三个入口写同一份文档' },
  { icon: 'wand', k: '语义布局', v: '「在标题下方 · 间距 90」会随画板尺寸重算' },
  { icon: 'export', k: '五种导出', v: 'PNG · JPG · WebP · SVG · PDF，同一趟 SVG 编译' },
];

export function Home() {
  const { state, go, read, loadProject, openSheet, toast, dispatch } = useApp();
  const list = useRpc<Summary[]>('project.list', {});
  const health = useRpc<{ ok: boolean; ver: string; port: number; fonts: number; root: string }>('system.health', {});

  const recents = useMemo(() => ((list.data ?? state.recent ?? []) as RecentProject[]).slice(0, 8), [list.data, state.recent]);
  const presets = state.caps?.presets ?? [];
  const port = health.data?.port ?? state.caps?.port ?? 8899;
  const base = `http://127.0.0.1:${port}/rpc`;
  const mcpCmd = `agentcanvas mcp --port ${port}`;
  const curl = `curl -s -X POST ${base} -H 'content-type: application/json' -d '{"m":"project.list","p":{}}'`;

  const open = async (p: string) => {
    const doc = await loadProject(p, { route: 'studio' });
    if (doc) dispatch({ t: 'save', save: 'saved' });
  };

  const runDemo = async () => {
    const r = await read<{ p?: string }>('system.demo', { o: 'demo' });
    if (!r) return;
    toast('ok', `示例海报已生成 · ${r.p ?? ''}`);
    list.reload();
    if (r?.p) await open(r.p);
  };

  const closeOne = (p: RecentProject) => {
    openSheet('confirm', {
      title: `关闭「${p.name}」`,
      body: '项目会从引擎内存中卸载，磁盘上的文件不受影响。',
      danger: true,
      confirm: '关闭',
      done: '已关闭',
      action: { m: 'project.close', p: { p: p.p } },
    });
  };

  return (
    <div className="page">
      <div className="page-in">
        {/* ------------------------------------------------------------ hero */}
        <section className="hero">
          <Glass depth={2} lift className="hero-copy" data-sheen>
            <span className="sec-no">本地引擎</span>
            <div className="row" style={{ gap: 8, flexWrap: 'wrap' }}>
              <Badge tone="grad">
                <Icon name="bolt" size={13} />
                桌面 · MCP · HTTP 同一套方法
              </Badge>
              <Badge>{health.data?.ver ? `v${health.data.ver}` : 'v1.0.0'}</Badge>
              {MOCK ? <Badge tone="warn">预览数据</Badge> : null}
            </div>
            <h1 className="display">
              给设计师的画布，
              <br />
              也给 <span className="grad-text">Agent</span> 的画布。
            </h1>
            <div className="hero-act">
              <Btn variant="primary" size="lg" icon="plus" onClick={() => openSheet('new')}>
                开始新画布
              </Btn>
              <Btn size="lg" icon="wand" onClick={() => void runDemo()} tip="用引擎跑通一张完整海报">
                运行示例
              </Btn>
              <Btn size="lg" variant="quiet" icon="settings" onClick={() => openSheet('settings')}>
                设置
              </Btn>
            </div>
            <div className="stack-6" style={{ marginTop: 24 }}>
              {FEATURES.map((f) => (
                <div key={f.k} className="row" style={{ gap: 10, alignItems: 'flex-start' }}>
                  <span className="feat-i">
                    <Icon name={f.icon} size={15} />
                  </span>
                  <span style={{ minWidth: 0 }}>
                    <b style={{ fontSize: 14, letterSpacing: '-0.01em' }}>{f.k}</b>
                    <span className="t2" style={{ fontSize: 13, marginLeft: 8 }}>
                      {f.v}
                    </span>
                  </span>
                </div>
              ))}
            </div>
            <div className="hero-stats">
              <span>
                <b>{presets.length}</b>
                <i>尺寸预设</i>
              </span>
              <span>
                <b>{state.caps?.node_types.length ?? 10}</b>
                <i>图层类型</i>
              </span>
              <span>
                <b>{state.caps?.intents.length ?? 19}</b>
                <i>布局意图</i>
              </span>
              <span>
                <b>{(state.caps?.methods ?? []).length || 90}</b>
                <i>RPC 方法</i>
              </span>
            </div>
          </Glass>

          <Glass depth={2} lift className="card" style={{ padding: 14 }} data-sheen>
            <span className="sec-no">实时预览</span>
            {recents[0] ? (
              <LivePreview pid={recents[0].p} name={recents[0].name} onOpen={open} />
            ) : (
              <Empty title="还没有画布" action={<Btn variant="primary" icon="plus" onClick={() => openSheet('new')}>开始新画布</Btn>} />
            )}
          </Glass>
        </section>

        {/* -------------------------------------------------------------- 01 */}
        <section className="sec">
          <div className="sec-h">
            <span className="sec-idx">01</span>
            <h2>最近画布</h2>
            <Badge tone="blue">{recents.length}</Badge>
            <span style={{ marginRight: 'auto' }} />
            <Btn size="sm" icon="refresh" onClick={() => list.reload()} tip="重新读取项目列表">
              刷新
            </Btn>
          </div>
          {recents.length ? (
            <div className="grid-proj">
              {recents.map((p) => (
                <ProjCard key={p.p} p={p} onOpen={() => void open(p.p)} onClose={() => closeOne(p)} />
              ))}
            </div>
          ) : (
            <Glass depth={2}>
              <Empty title="还没有画布" action={<Btn variant="primary" icon="plus" onClick={() => openSheet('new')}>开始新画布</Btn>} />
            </Glass>
          )}
        </section>

        {/* -------------------------------------------------------------- 02 */}
        <section className="sec">
          <div className="sec-h">
            <span className="sec-idx">02</span>
            <h2>尺寸预设</h2>
            <span style={{ marginRight: 'auto' }} />
            <Badge tone="blue">{presets.length}</Badge>
          </div>
          <Glass depth={2} className="card" data-sheen>
            <div className="grid-chips">
              {presets.map((p) => (
                <Chip key={p.k} onClick={() => openSheet('new', { preset: p.k, w: p.wh[0], h: p.wh[1] })} tip={`新建 ${p.d} 画布`}>
                  <span style={{ fontWeight: 700 }}>{p.d}</span>
                  <span className="num t3">
                    {p.wh[0]}×{p.wh[1]}
                  </span>
                </Chip>
              ))}
            </div>
          </Glass>
        </section>

        {/* -------------------------------------------------------------- 03 */}
        <section className="sec">
          <div className="sec-h">
            <span className="sec-idx">03</span>
            <h2>接入</h2>
            <span style={{ marginRight: 'auto' }} />
            <Btn size="sm" icon="chevron" onClick={() => go('access')}>
              打开 Agent 接入
            </Btn>
          </div>
          <Glass depth={2} className="card" data-sheen>
            <div className="row" style={{ gap: 8, marginBottom: 14, flexWrap: 'wrap' }}>
              <Badge tone="ok">
                <Icon name="check" size={13} />
                本地 API
              </Badge>
              <Badge tone="blue">127.0.0.1:{port}</Badge>
              <Badge>{health.data?.fonts ?? 0} 个字体</Badge>
              <Badge>{String(health.data?.root ?? '~/AgentCanvas')}</Badge>
            </div>
            <div className="stack-6">
              {[
                { icon: 'link' as const, text: base },
                { icon: 'code' as const, text: mcpCmd },
                { icon: 'bolt' as const, text: curl },
              ].map((l) => (
                <div className="copy-line" key={l.text}>
                  <Icon name={l.icon} size={14} />
                  <span className="trunc" style={{ flex: '1 1 auto' }}>
                    {l.text}
                  </span>
                  <CopyBtn text={l.text} label="复制" />
                </div>
              ))}
            </div>
          </Glass>
        </section>
      </div>
    </div>
  );
}

/** The right-hand hero card: the whole live document, straight from `render.svg`
 *  (no `b`, so every board compiles — `project.list` carries no board ids). */
function LivePreview({
  pid,
  name,
  onOpen,
}: {
  pid: string;
  name: string;
  onOpen: (pid: string) => void;
}) {
  const svg = useRpc<unknown>('render.svg', { p: pid, preview: 1 });
  const body = unwrap(svg.data);
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 12, height: '100%' }}>
      <div className="thumb" style={{ paddingTop: '84%', background: 'linear-gradient(160deg, rgba(23,29,63,.05), rgba(23,29,63,.02))' }}>
        <div style={{ position: 'absolute', inset: 0, display: 'grid', placeItems: 'center' }} dangerouslySetInnerHTML={{ __html: body || '<svg></svg>' }} />
      </div>
      <div className="row-b">
        <div style={{ minWidth: 0 }}>
          <div className="h3 trunc">{name}</div>
          <div className="t3" style={{ fontSize: 13 }}>
            render.svg · preview:1
          </div>
        </div>
        <Btn size="sm" icon="chevron" onClick={() => void onOpen(pid)} tip="在工作室中打开">
          打开
        </Btn>
      </div>
    </div>
  );
}
