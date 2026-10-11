/** Dockable Agent panel. It shows the plan and every tool call the run made.
 *
 *  There is no LLM key in this build, so the panel runs in 本地生成 mode: the user's
 *  phrasing is parsed deterministically and turned into real `canvas.*` / `layout.*`
 *  calls. The header states that plainly — the panel never claims model work. */

import { useEffect, useMemo, useRef, useState } from 'react';
import { absUrl, rpc } from '../api';
import { useApp, type AgentCall, type AgentMsg } from '../state/app';
import type { Intent, Mut, Preview, V } from '../types';
import { Badge, Btn, Chip, IconBtn } from './Primitives';
import { Icon } from './Icons';
import { uid } from '../lib/format';

/* ------------------------------------------------------------ local parser */

export interface Brief {
  title?: string;
  subtitle?: string;
  body?: string;
  accent?: string;
  preset?: string;
  w?: number;
  h?: number;
  assets: string[];
  kinds: string[];
  raw: string;
}

const COLOR_WORDS: [RegExp, string][] = [
  [/蓝[色]?|blue/i, '#4B6BFB'],
  [/紫[色]?|violet|purple/i, '#8B5CF6'],
  [/橙|珊瑚|coral|暖/i, '#FF7A59'],
  [/青|薄荷|cyan|teal/i, '#22D3EE'],
  [/红|red/i, '#E11D48'],
  [/绿|green/i, '#16A34A'],
  [/黑|深色|night/i, '#171D3F'],
  [/米|白|浅/i, '#F6F1E7'],
];

const SIZE_WORDS: [RegExp, string][] = [
  [/海报|3:4/, 'poster'],
  [/方图|1:1|方形/, 'square'],
  [/故事|9:16|竖屏/, 'story'],
  [/4:5/, '4:5'],
  [/横幅|16:9|banner/, 'banner'],
  [/小红书|xhs/, 'xhs'],
  [/电商|主图/, 'ecom-main'],
  [/长图/, 'long'],
  [/a4/i, 'a4'],
  [/公众号/, 'wechat'],
  [/名片/, 'business-card'],
];

function quoted(text: string, keys: string[]): string | undefined {
  for (const k of keys) {
    const re = new RegExp(`${k}\\s*[:：]?\\s*(?:“([^”]+)”|"([^"]+)"|「([^」]+)」|([^，,。;\\n]+))`, 'i');
    const m = re.exec(text);
    if (m) {
      const v = (m[1] ?? m[2] ?? m[3] ?? m[4] ?? '').trim();
      if (v) return v;
    }
  }
  return undefined;
}

export function parseBrief(text: string, assetNames: string[] = []): Brief {
  const out: Brief = { assets: [], kinds: [], raw: text };
  out.title = quoted(text, ['标题', '主标题', '大标题', 'headline', 'title']);
  out.subtitle = quoted(text, ['副标题', '副标', '小标题', 'subtitle']);
  out.body = quoted(text, ['正文', '文案', '描述', 'body']);
  const hex = /#([0-9a-f]{6}|[0-9a-f]{3})\b/i.exec(text);
  if (hex) out.accent = hex[0].toUpperCase();
  else {
    const hit = COLOR_WORDS.find(([re]) => re.test(text));
    if (hit) out.accent = hit[1];
  }
  const wh = /(\d{3,5})\s*[x×*]\s*(\d{3,5})/.exec(text);
  if (wh) {
    out.w = Number(wh[1]);
    out.h = Number(wh[2]);
  } else {
    const p = SIZE_WORDS.find(([re]) => re.test(text));
    if (p) out.preset = p[1];
  }
  const lower = text.toLowerCase();
  for (const n of assetNames) {
    if (lower.includes(n.toLowerCase()) || lower.includes(n.replace(/\.[^.]+$/, '').toLowerCase())) out.assets.push(n);
  }
  if (/(?:¥|￥)\s?\d|\d+\s?元|价格/.test(text)) out.kinds.push('价格');
  if (/按钮|预约|购买|下单|cta/i.test(text)) out.kinds.push('按钮');
  if (/角标|新品|\bnew\b/i.test(text)) out.kinds.push('角标');
  if (/分隔线|分割线/.test(text)) out.kinds.push('分隔线');
  if (!out.title) {
    const first = text.split(/[\n，,。.]/)[0]?.trim() ?? '';
    out.title = first.replace(/^(标题|主标题|大标题)\s*[:：]?\s*/, '').slice(0, 14) || '少即是多';
  }
  return out;
}

/* ------------------------------------------------------- local generation */

interface Plan {
  label: string;
  steps: { m: string; p: V; note: string }[];
}

function buildPlan(b: Brief, opts: { pid: string | null; board: string | null; boardW: number; boardH: number; assets: { id: string; name: string }[] }): Plan {
  const accent = b.accent ?? '#4B6BFB';
  const W = b.w ?? opts.boardW;
  const H = b.h ?? opts.boardH;
  const steps: Plan['steps'] = [];

  if (!opts.pid) {
    steps.push({
      m: 'poster',
      note: '还没有项目 · 一次调用生成完整海报',
      p: {
        title: b.title,
        subtitle: b.subtitle ?? 'LESS BUT BETTER',
        body: b.body ?? '',
        accent,
        preset: b.preset ?? 'poster',
        f: 'png',
        out: 'agent',
      },
    });
    return { label: '新建项目并出图', steps };
  }

  const nodes: V[] = [];
  const margin = Math.round(W * 0.08);
  const head: Intent = { anchor: 'top-left', margin_top: Math.round(H * 0.16), margin_left: margin, size: { w_ref: 0.84 } };
  nodes.push({
    t: 'text',
    name: '标题 · Agent',
    x: margin,
    y: Math.round(H * 0.16),
    w: W - margin * 2,
    h: Math.round(H * 0.13),
    text: b.title ?? '少即是多',
    size: Math.round(W * 0.1),
    weight: 800,
    color: '#171D3F',
    family: 'PingFang SC',
    ls: -Math.round(W * 0.003),
    intent: head,
  });
  nodes.push({
    t: 'rect',
    name: '色条 · Agent',
    x: margin,
    y: Math.round(H * 0.31),
    w: Math.round(W * 0.13),
    h: 8,
    style: { fill: { t: 'linear', angle: 90, stops: [{ at: 0, color: accent }, { at: 1, color: '#8B5CF6' }] }, radius: { all: 4 } },
    intent: { below: { of: '标题 · Agent', gap: 26, cross: 'start' }, anchor: 'left', margin_left: margin },
  });
  if (b.subtitle) {
    nodes.push({
      t: 'text',
      name: '副标题 · Agent',
      x: margin,
      y: Math.round(H * 0.36),
      w: Math.round(W * 0.72),
      h: Math.round(H * 0.05),
      text: b.subtitle,
      size: Math.round(W * 0.026),
      weight: 500,
      color: '#5A618A',
      ls: Math.round(W * 0.002),
      intent: { below: { of: '色条 · Agent', gap: 22, cross: 'start' }, margin_left: margin },
    });
  }
  if (b.assets.length && opts.assets[0]) {
    const a = opts.assets.find((x) => b.assets.includes(x.name)) ?? opts.assets[0];
    nodes.push({
      t: 'image',
      name: `主图 · ${a.name}`,
      asset: a.id,
      x: Math.round(W * 0.24),
      y: Math.round(H * 0.46),
      w: Math.round(W * 0.52),
      h: Math.round(W * 0.52),
      fit: 'cover',
      style: { radius: { all: Math.round(W * 0.03) }, fill: { t: 'image', asset: a.id, fit: 'cover', opacity: 1 } },
      fx: { shadows: [{ x: 0, y: 26, blur: 48, color: '#171D3F2E' }] },
      intent: { center_in: opts.board ?? undefined, size: { w_ref: 0.52, ratio: 1 }, safe_margin: 60 },
    });
  }
  if (b.body) {
    nodes.push({
      t: 'text',
      name: '正文 · Agent',
      x: margin,
      y: Math.round(H * 0.72),
      w: Math.round(W * 0.76),
      h: Math.round(H * 0.1),
      text: b.body,
      size: Math.round(W * 0.022),
      weight: 400,
      color: '#4C5474',
      lh: 1.65,
      intent: { anchor: 'bottom-left', margin_left: margin, margin_bottom: Math.round(H * 0.16), size: { w_ref: 0.76 } },
    });
  }
  if (b.kinds.includes('价格')) {
    const price = /(?:¥|￥)\s?\d+(?:\.\d+)?|\d+(?:\.\d+)?\s?元/.exec(b.raw)?.[0] ?? '¥39';
    nodes.push({
      t: 'text',
      name: '价格 · Agent',
      x: Math.round(W * 0.66),
      y: Math.round(H * 0.78),
      w: Math.round(W * 0.26),
      h: Math.round(H * 0.08),
      text: price || '¥39',
      size: Math.round(W * 0.062),
      weight: 800,
      color: accent,
      align: 'right',
      intent: { anchor: 'bottom-right', margin_right: margin, margin_bottom: Math.round(H * 0.14) },
    });
  }
  if (b.kinds.includes('按钮')) {
    nodes.push({
      t: 'rect',
      name: '按钮 · Agent',
      x: margin,
      y: Math.round(H * 0.84),
      w: Math.round(W * 0.32),
      h: Math.round(H * 0.06),
      style: { fill: { t: 'solid', color: accent }, radius: { all: Math.round(H * 0.03) } },
      intent: { anchor: 'bottom-left', margin_left: margin, margin_bottom: Math.round(H * 0.1) },
    });
    nodes.push({
      t: 'text',
      name: '按钮文字 · Agent',
      x: margin,
      y: Math.round(H * 0.855),
      w: Math.round(W * 0.32),
      h: Math.round(H * 0.03),
      text: '立即预约',
      size: Math.round(W * 0.024),
      weight: 700,
      color: '#FFFFFF',
      align: 'center',
      intent: { center_in: '按钮 · Agent' },
    });
  }
  if (b.kinds.includes('角标')) {
    nodes.push({
      t: 'rect',
      name: '角标 · Agent',
      x: Math.round(W * 0.78),
      y: Math.round(H * 0.07),
      w: Math.round(W * 0.15),
      h: Math.round(H * 0.04),
      style: { fill: { t: 'solid', color: '#171D3F' }, radius: { all: Math.round(H * 0.02) } },
      intent: { anchor: 'top-right', margin_top: Math.round(H * 0.07), margin_right: margin },
    });
  }
  if (b.kinds.includes('分隔线')) {
    nodes.push({
      t: 'line',
      name: '分隔线 · Agent',
      x: margin,
      y: Math.round(H * 0.92),
      w: W - margin * 2,
      h: 0,
      style: { stroke: { color: '#171D3F26', width: 1.5 } },
      intent: { size: { w_ref: 0.84 }, anchor: 'bottom', margin_bottom: Math.round(H * 0.08) },
    });
  }

  steps.push({ m: 'canvas.add_many', note: `一次事务写入 ${nodes.length} 个图层`, p: { b: opts.board, nodes } });
  steps.push({ m: 'layout.resolve', note: '按意图重排本板', p: { b: opts.board } });
  steps.push({ m: 'inspect.validate', note: '检查排版结果', p: { b: opts.board, contrast: 1 } });
  steps.push({ m: 'render.preview', note: '生成预览对比', p: { b: opts.board, sc: 0.32 } });

  const label = b.assets.length
    ? `写入 ${nodes.length} 个图层 · 用图 ${b.assets.join('、')} · ${accent.toUpperCase()}`
    : `写入 ${nodes.length} 个图层 · 主色 ${accent.toUpperCase()}`;
  return { label, steps };
}

/* -------------------------------------------------------------- call list */

function CallRow({ call, i }: { call: AgentCall; i: number }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="call">
      <button type="button" className="call-h" onClick={() => setOpen((v) => !v)}>
        <span className="badge" style={{ minWidth: 22, justifyContent: 'center' }}>
          {String(i + 1).padStart(2, '0')}
        </span>
        <span className="call-m">{call.m}</span>
        <span className="spacer" />
        <Badge tone={call.ok ? 'ok' : 'err'}>{call.ok ? `${call.ms}ms` : '失败'}</Badge>
        <Icon name="chevron" size={13} style={{ transform: open ? 'rotate(90deg)' : undefined, transition: 'transform var(--d1) var(--ease)' }} />
      </button>
      {open ? (
        <div className="call-b">
          <div className="row" style={{ gap: 6 }}>
            <span className="t3" style={{ fontSize: 13 }}>参数</span>
          </div>
          <pre>{JSON.stringify(call.p, null, 1)}</pre>
          <div className="row" style={{ gap: 6 }}>
            <span className="t3" style={{ fontSize: 13 }}>返回</span>
          </div>
          <pre>{call.summary}</pre>
        </div>
      ) : null}
    </div>
  );
}

function Thread({ msg }: { msg: AgentMsg }) {
  if (msg.from === 'me') {
    return (
      <div className="msg msg-me">
        <div className="bub">{msg.text}</div>
      </div>
    );
  }
  return (
    <div className="msg msg-ai" style={{ flexDirection: 'column', gap: 8 }}>
      <div className="bub" style={{ maxWidth: '100%' }}>
        {msg.text}
      </div>
      {msg.plan ? (
        <div className="plan">
          {msg.plan.map((p, i) => (
            <Chip key={i} dim icon={i === 0 ? 'wand' : 'chevron'}>
              {p}
            </Chip>
          ))}
        </div>
      ) : null}
      {msg.calls?.length ? (
        <div className="stack-6" style={{ width: '100%' }}>
          {msg.calls.map((c, i) => (
            <CallRow key={i} call={c} i={i} />
          ))}
        </div>
      ) : null}
      {msg.before || msg.after ? (
        <div className="row" style={{ gap: 8, width: '100%' }}>
          <div style={{ flex: 1 }}>
            <div className="thumb" style={{ paddingTop: '72%', background: '#fff' }}>
              {msg.before ? <img src={msg.before} alt="改动前" /> : <div className="skl" style={{ position: 'absolute', inset: 0 }} />}
            </div>
            <span className="t3" style={{ fontSize: 13 }}>改动前</span>
          </div>
          <Icon name="chevron" size={16} />
          <div style={{ flex: 1 }}>
            <div className="thumb" style={{ paddingTop: '72%', background: '#fff' }}>
              {msg.after ? <img src={msg.after} alt="改动后" /> : <div className="skl" style={{ position: 'absolute', inset: 0 }} />}
            </div>
            <span className="t3" style={{ fontSize: 13 }}>改动后</span>
          </div>
        </div>
      ) : null}
      {msg.error ? <Badge tone="err">{msg.error}</Badge> : null}
    </div>
  );
}

/* --------------------------------------------------------------- the dock */

const QUICK = ['标题：夏日轻乳 副标题：生牛乳发酵 配色：#4B6BFB 尺寸：1080×1440', '标题：城市夜跑 按钮：立即预约 角标：NEW 配色：青色', '标题：咖啡节 2026 正文：32 家独立烘焙 尺寸：小红书'];

export function AgentPanel() {
  const { state, dispatch, mutate, read, toast, loadProject } = useApp();
  const dispatchProject = (pid: string) => loadProject(pid, { route: 'studio' });
  const [text, setText] = useState('');
  const [busy, setBusy] = useState(false);
  const body = useRef<HTMLDivElement | null>(null);
  const doc = state.doc;
  const board = doc?.artboards.find((a) => a.id === state.board) ?? doc?.artboards[0] ?? null;
  const lastPrompt = useMemo(() => [...state.agent.thread].reverse().find((m) => m.from === 'me')?.text ?? '', [state.agent.thread]);

  useEffect(() => {
    const el = body.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [state.agent.thread.length]);

  const startResize = (e: React.PointerEvent) => {
    e.preventDefault();
    const s = state.agent.size;
    const move = (ev: PointerEvent) => {
      const dy = e.clientY - ev.clientY;
      const dx = e.clientX - ev.clientX;
      dispatch({
        t: 'agent',
        patch: { size: { w: Math.max(300, Math.min(720, s.w + dx)), h: Math.max(260, Math.min(window.innerHeight - 200, s.h + dy)) } },
      });
    };
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };

  const run = async () => {
    const prompt = text.trim();
    if (!prompt || busy) return;
    setText('');
    dispatch({ t: 'agent-msg', msg: { id: uid('m'), from: 'me', text: prompt } });
    dispatch({ t: 'agent', patch: { running: true } });
    setBusy(true);

    const brief = parseBrief(prompt, (doc?.assets ?? []).map((a) => a.name));
    const plan = buildPlan(brief, {
      pid: state.pid,
      board: board?.id ?? null,
      boardW: board?.width ?? 1080,
      boardH: board?.height ?? 1440,
      assets: doc?.assets ?? [],
    });

    const before = state.pid ? await read<Preview>('render.preview', { p: state.pid, b: board?.id, sc: 0.32 }, { silent: true }) : null;
    const calls: AgentCall[] = [];
    let error: string | undefined;

    for (const step of plan.steps) {
      const t0 = performance.now();
      let summary = '';
      let ok = true;
      try {
        if (step.m === 'canvas.add_many' || step.m === 'layout.resolve' || step.m === 'inspect.validate') {
          const r = await mutate<Mut>(step.m, step.p);
          if (r) {
            const list = (r.changed ?? []) as string[];
            summary =
              step.m === 'inspect.validate'
                ? `问题 ${((r as unknown as { issues?: unknown[] }).issues ?? []).length} 个`
                : `${list.length} 个图层已更新 · rev ${r.rev}${r.conflicts?.length ? ` · 冲突 ${r.conflicts.length}` : ''}`;
          } else ok = false;
        } else if (step.m === 'render.preview') {
          const r = await read<Preview>('render.preview', { p: state.pid, ...step.p }, { silent: true });
          summary = r ? `${r.w}×${r.h} · ${(r.bytes / 1024).toFixed(0)} KB` : '';
          ok = Boolean(r);
        } else if (step.m === 'poster') {
          const r = await rpc<V>('poster', { ...step.p, out: 'agent' });
          summary = `新项目 ${String(r.p ?? '')} · ${String(r.name ?? '')}`;
          if (r.p) await dispatchProject(String(r.p));
        } else {
          const r = await mutate(step.m, step.p);
          summary = r ? JSON.stringify(r).slice(0, 120) : '';
        }
      } catch (e) {
        ok = false;
        const err = e as Error & { code?: string };
        error = `${err.code ?? 'error'} · ${err.message}`;
        summary = err.message;
      }
      calls.push({ m: step.m, p: step.p, ok, summary, ms: Math.round(performance.now() - t0) });
      if (!ok) break;
    }

    const after = state.pid ? await read<Preview>('render.preview', { p: state.pid, b: board?.id, sc: 0.32 }, { silent: true }) : null;

    dispatch({
      t: 'agent-msg',
      msg: {
        id: uid('m'),
        from: 'ac',
        text: `本地按关键词解析出的方案，未调用任何模型。${error ? ` 停在第 ${calls.length} 步：${error}` : ''}`,
        plan: [plan.label, ...plan.steps.map((s) => s.note)],
        calls,
        before: before ? absUrl(before.url) : undefined,
        after: after ? absUrl(after.url) : undefined,
        error,
      },
    });
    dispatch({ t: 'agent', patch: { running: false, open: true } });
    setBusy(false);
    if (!error) toast('ok', `Agent 面板已执行 ${calls.length} 个方法`);
  };

  const send = () => void run();

  if (!state.agent.open) {
    return (
      <button type="button" className="dock-pill glass glass-3" data-sheen onClick={() => dispatch({ t: 'agent', patch: { open: true } })}>
        <span style={{ display: 'grid', placeItems: 'center', color: 'var(--blue)' }}>
          <Icon name="sparkle" size={16} />
        </span>
        Agent 面板
        <Badge tone="blue">{state.agent.thread.length}</Badge>
      </button>
    );
  }

  const { w, h } = state.agent.size;

  return (
    <div className="dock glass glass-3" style={{ width: w, height: h }} data-sheen>
      <div className="dock-rz" onPointerDown={startResize} data-tip="拖动改变面板大小" />
      <div className="dock-h" onClick={() => dispatch({ t: 'agent', patch: { open: false } })}>
        <span style={{ display: 'grid', placeItems: 'center', color: 'var(--blue)' }}>
          <Icon name="bolt" size={16} />
        </span>
        <span className="h3">Agent 面板</span>
        <Badge tone="warn">本地生成 · 未调用模型</Badge>
        <span className="spacer" />
        {state.agent.running ? (
          <Badge tone="blue">
            <Icon name="refresh" size={13} className="spin" />
            执行中
          </Badge>
        ) : null}
        <IconBtn icon="close" size="sm" tip="收起成胶囊" label="收起" onClick={() => dispatch({ t: 'agent', patch: { open: false } })} />
      </div>

      <div className="dock-b" ref={body}>
        {state.agent.thread.length ? (
          <div className="stack-6">
            {state.agent.thread.map((m) => (
              <Thread key={m.id} msg={m} />
            ))}
          </div>
        ) : (
          <div className="stack-6" style={{ gap: 12 }}>
            <div className="row" style={{ gap: 8 }}>
              <Badge tone="blue">
                <Icon name="info" size={13} />
                这一版没有模型密钥
              </Badge>
            </div>
            <div className="h3" style={{ lineHeight: 1.35 }}>
              用一句话描述画面，面板会解析成真实的 <span className="mono" style={{ fontSize: 13 }}>canvas.add_many</span> + <span className="mono" style={{ fontSize: 13 }}>layout.resolve</span> 调用，并把每一步列出来。
            </div>
            <div className="plan">
              {QUICK.map((q) => (
                <Chip key={q} onClick={() => setText(q)} tip="填入示例描述">
                  {q.slice(0, 12)}…
                </Chip>
              ))}
            </div>
          </div>
        )}
      </div>

      <div className="dock-f">
        <div className="row" style={{ gap: 6, marginBottom: 8, flexWrap: 'wrap' }}>
          <Btn
            size="sm"
            variant="quiet"
            icon="history"
            disabled={!lastPrompt}
            tip="把上一条描述填回输入框"
            onClick={() => lastPrompt && setText(lastPrompt)}
          >
            重填上一条
          </Btn>
          <Btn size="sm" variant="quiet" icon="trash" disabled={!state.agent.thread.length} tip="清空对话记录" onClick={() => dispatch({ t: 'agent', patch: { thread: [] } })}>
            清空
          </Btn>
          <span className="spacer" />
          <Badge>{doc ? `${doc.nodes.length} 图层` : '无项目'}</Badge>
        </div>
        <div className="row" style={{ gap: 8, alignItems: 'flex-end' }}>
          <textarea
            className="inp"
            rows={2}
            style={{ minHeight: 56, resize: 'none' }}
            placeholder="标题：夏日轻乳 副标题：生牛乳发酵 配色：#4B6BFB"
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
                e.preventDefault();
                send();
              }
            }}
          />
          <Btn variant="primary" icon="send" disabled={busy || !text.trim()} onClick={send} tip="执行（⌘/Ctrl+Enter）">
            生成
          </Btn>
        </div>
      </div>
    </div>
  );
}
