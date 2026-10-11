/** Glass sheets over the current surface: 新建 / 导出 / 设置 / 确认 / 模板 /
 *  说明 / 布局解释 / 历史. Nothing here is a separate page. */

import { useEffect, useState } from 'react';
import { fileUrl, useRpc } from '../api';
import { useApp } from '../state/app';
import type { Caps, HistoryEntry, Mut, Summary, V } from '../types';
import { Sheet } from './Glass';
import { Badge, Btn, Chip, ColorBtn, CopyBtn, Empty, Field, IconBtn, NumVal, Range, Select, Switch, TextInput } from './Primitives';
import { Icon } from './Icons';
import { findBoard } from '../lib/doc';
import { fmtBytes, shortSha, timeAgo } from '../lib/format';

/* ---------------------------------------------------------------- new sheet */

function NewSheet({ close, preset }: { close: () => void; preset?: string }) {
  const { state, read, loadProject, toast } = useApp();
  const [name, setName] = useState('未命名画布');
  const [p, setP] = useState(preset ?? 'poster');
  const [w, setW] = useState(1080);
  const [h, setH] = useState(1440);
  const [extra, setExtra] = useState(0);
  const [dir, setDir] = useState('');
  const [busy, setBusy] = useState(false);
  const presets = state.caps?.presets ?? [];

  const pick = (k: string, ww: number, hh: number) => {
    setP(k);
    setW(ww);
    setH(hh);
  };

  const create = async () => {
    setBusy(true);
    const boards = Array.from({ length: extra }, (_, i) => ({ w, h, name: `画板 ${i + 2}` }));
    const r = await read<Summary>('project.create', { name, w, h, preset: p, ...(boards.length ? { boards } : {}), ...(dir ? { dir } : {}) });
    if (!r) {
      setBusy(false);
      return;
    }
    toast('ok', `已创建「${r.name}」`);
    await loadProject(r.p, { route: 'studio' });
    setBusy(false);
    close();
  };

  return (
    <Sheet
      title="开始新画布"
      eyebrow="project.create"
      width={680}
      onClose={close}
      footer={
        <>
          <Btn variant="quiet" onClick={close}>
            取消
          </Btn>
          <Btn variant="primary" icon="plus" disabled={busy || !name.trim()} onClick={() => void create()}>
            {busy ? '创建中' : '创建并进入工作室'}
          </Btn>
        </>
      }
    >
      <div className="stack" style={{ gap: 16 }}>
        <Field label="项目名">
          <TextInput value={name} onChange={setName} tip="显示在标题栏与导出文件名里" />
        </Field>

        <Field label="尺寸预设">
          <div className="grid-chips">
            {presets.map((x) => (
              <Chip key={x.k} active={x.k === p} onClick={() => pick(x.k, x.wh[0], x.wh[1])} tip={`${x.d} · ${x.wh[0]}×${x.wh[1]}`}>
                {x.d}
              </Chip>
            ))}
          </div>
        </Field>

        <div className="row" style={{ gap: 10, alignItems: 'flex-end' }}>
          <Field label="宽">
            <NumVal tag="W" value={w} min={1} max={20000} dp={0} onChange={setW} />
          </Field>
          <Field label="高">
            <NumVal tag="H" value={h} min={1} max={20000} dp={0} onChange={setH} />
          </Field>
          <Field label="比例">
            <span className="badge" style={{ height: 32 }}>{(w / Math.max(1, h)).toFixed(3)}</span>
          </Field>
          <div style={{ flex: 1 }} />
          <Field label="额外画板">
            <div className="row" style={{ gap: 4 }}>
              {[0, 1, 2, 3].map((n) => (
                <button key={n} type="button" className={`btn btn-sm ${extra === n ? 'is-on' : ''}`} onClick={() => setExtra(n)} data-tip={`再加 ${n} 个同尺寸画板`}>
                  {n}
                </button>
              ))}
            </div>
          </Field>
        </div>

        <Field label="工作区目录">
          <TextInput value={dir} onChange={setDir} placeholder={String(state.caps?.paths?.root ?? '~/AgentCanvas')} tip="留空则用引擎默认工作区" mono />
        </Field>
      </div>
    </Sheet>
  );
}

/* -------------------------------------------------------------- export sheet */

const FORMATS = [
  { v: 'png', label: 'PNG', d: '透明 · 无损' },
  { v: 'jpg', label: 'JPG', d: '照片类 · 有损' },
  { v: 'webp', label: 'WebP', d: '更小 · 现代浏览器' },
  { v: 'svg', label: 'SVG', d: '矢量 · 可嵌入字体' },
  { v: 'pdf', label: 'PDF', d: '印刷 · 每板一页' },
];

function ExportSheet({ close }: { close: () => void }) {
  const { state, read, toast } = useApp();
  const doc = state.doc;
  const [f, setF] = useState('png');
  const [sc, setSc] = useState(2);
  const [q, setQ] = useState(92);
  const [transparent, setTransparent] = useState(false);
  const [boards, setBoards] = useState<string[]>(() => (state.board ? [state.board] : []));
  const [out, setOut] = useState('exports');
  const [files, setFiles] = useState<{ path: string; bytes: number; sha: string; w?: number; h?: number; url?: string }[]>([]);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!boards.length && state.board) setBoards([state.board]);
  }, [boards.length, state.board]);

  const run = async () => {
    if (!doc) return;
    setBusy(true);
    const r = await read<{ files?: { path: string; bytes: number; sha: string; w: number; h: number; url?: string }[]; n?: number }>(
      'render.export',
      { b: boards.length === 1 ? boards[0] : undefined, boards, f, sc, q, transparent, o: out },
    );
    setBusy(false);
    if (!r?.files) return;
    setFiles(r.files);
    toast('ok', `已导出 ${r.files.length} 个文件`);
  };

  if (!doc)
    return (
      <Sheet title="导出" onClose={close}>
        <Empty title="还没有打开画布" />
      </Sheet>
    );

  const total = files.reduce((s, x) => s + x.bytes, 0);

  return (
    <Sheet
      title="导出"
      eyebrow="render.export"
      width={760}
      onClose={close}
      footer={
        <>
          {files.length ? <span className="t3" style={{ fontSize: 13, marginRight: 'auto' }}>共 {files.length} 个文件 · {fmtBytes(total)}</span> : null}
          <Btn variant="quiet" onClick={close}>
            关闭
          </Btn>
          <Btn variant="primary" icon="export" disabled={busy || !boards.length} onClick={() => void run()}>
            {busy ? '渲染中' : `导出 ${boards.length} 个画板`}
          </Btn>
        </>
      }
    >
      <div className="stack" style={{ gap: 16 }}>
        <Field label="格式">
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(120px, 1fr))', gap: 10 }}>
            {FORMATS.map((x) => (
              <button
                key={x.v}
                type="button"
                className={`proj-card glass glass-2 ${f === x.v ? 'is-on' : ''}`}
                style={{ padding: 12, borderRadius: 16, textAlign: 'left', borderColor: f === x.v ? 'rgba(75,107,251,.7)' : undefined }}
                onClick={() => setF(x.v)}
                data-sheen
              >
                <div className="row-b">
                  <span className="h3">{x.label}</span>
                  {f === x.v ? <Icon name="check" size={15} /> : null}
                </div>
                <span className="t3" style={{ fontSize: 13 }}>{x.d}</span>
              </button>
            ))}
          </div>
        </Field>

        <div className="row" style={{ gap: 14, alignItems: 'flex-end', flexWrap: 'wrap' }}>
          <Field label="倍率">
            <div className="row" style={{ gap: 4 }}>
              {[1, 2, 3, 4].map((n) => (
                <button key={n} type="button" className={`btn btn-sm ${sc === n ? 'is-on' : ''}`} onClick={() => setSc(n)} data-tip={`输出 ${n} 倍尺寸`}>
                  {n}x
                </button>
              ))}
            </div>
          </Field>
          <div style={{ flex: '1 1 180px', minWidth: 160 }}>
            <Field label={`质量 ${q}`}>
              <Range value={q} min={40} max={100} onChange={setQ} tip="仅对有损格式生效" />
            </Field>
          </div>
          <Field label="输出目录">
            <TextInput value={out} onChange={setOut} mono tip="相对工作区的路径" />
          </Field>
        </div>

        <div className="row" style={{ gap: 16, flexWrap: 'wrap', alignItems: 'center' }}>
          <SwitchRowLabel label="透明背景" on={transparent} onChange={setTransparent} disabled={f === 'jpg' || f === 'pdf'} tip="JPG / PDF 不支持透明" />
          <span className="badge" style={{ marginLeft: 'auto' }}>
            输出约 {doc.artboards.filter((a) => boards.includes(a.id)).reduce((s, a) => s + a.width * a.height, 0).toLocaleString()} px² · {sc}x
          </span>
        </div>

        <Field label="画板">
          <div className="grid-chips">
            {doc.artboards.map((a) => (
              <Chip
                key={a.id}
                active={boards.includes(a.id)}
                onClick={() => setBoards((v) => (v.includes(a.id) ? v.filter((x) => x !== a.id) : [...v, a.id]))}
                tip={`${a.name} · ${a.layers.length} 层`}
              >
                {a.name}
                <span className="num t3">
                  {Math.round(a.width)}×{Math.round(a.height)}
                </span>
              </Chip>
            ))}
            <Chip onClick={() => setBoards(doc.artboards.map((a) => a.id))} tip="全选">
              全部
            </Chip>
          </div>
        </Field>

        {files.length ? (
          <div className="stack-6">
            {files.map((x, i) => (
              <div key={i} className="row" style={{ gap: 10, padding: '8px 10px', borderRadius: 12, background: 'rgba(255,255,255,.42)', border: '1px solid rgba(255,255,255,.6)' }}>
                {x.url ? (
                  <img src={x.url} alt="" style={{ width: 34, height: 44, objectFit: 'cover', borderRadius: 6, background: '#fff' }} />
                ) : (
                  <span className="dot" style={{ background: 'var(--grad)' }} />
                )}
                <span style={{ flex: '1 1 auto', minWidth: 0 }}>
                  <span className="trunc" style={{ display: 'block', fontSize: 13, fontWeight: 700 }}>
                    {x.path.split(/[\\/]/).pop()}
                  </span>
                  <span className="t3" style={{ fontSize: 12, fontFamily: 'var(--mono)' }}>
                    {x.w ?? 0}×{x.h ?? 0} · {fmtBytes(x.bytes)} · sha {shortSha(x.sha)}
                  </span>
                </span>
                <CopyBtn text={x.path} tip="复制完整路径" />
                <Btn size="sm" icon="folder" onClick={() => window.open(fileUrl(x.path), '_blank')} tip="在浏览器里打开导出的文件">
                  打开文件
                </Btn>
              </div>
            ))}
          </div>
        ) : null}
      </div>
    </Sheet>
  );
}

function SwitchRowLabel({ label, on, onChange, tip, disabled }: { label: string; on: boolean; onChange: (v: boolean) => void; tip?: string; disabled?: boolean }) {
  return (
    <div className="row" style={{ gap: 10, opacity: disabled ? 0.45 : 1 }}>
      <span style={{ fontSize: 13, fontWeight: 600, color: 'var(--t2)' }}>{label}</span>
      <Switch on={on} onChange={(v) => !disabled && onChange(v)} tip={disabled ? tip : '点击切换'} />
    </div>
  );
}

/* ------------------------------------------------------------ settings sheet */

/** Engine settings are the keys `system.config` actually accepts; the two switches at
 *  the bottom are window preferences and are stored on this machine only. */
const PROVIDERS = [
  { v: '', label: '不使用' },
  { v: 'ollama', label: 'Ollama' },
  { v: 'openai', label: 'OpenAI 兼容' },
  { v: 'custom', label: '自定义' },
];

function SettingsSheet({ close }: { close: () => void }) {
  const { read, toast, state, dispatch } = useApp();
  const cfg = useRpc<V>('system.config', {});
  const [draft, setDraft] = useState<V>({});
  const [probe, setProbe] = useState<{ busy: boolean; r?: V; e?: V }>({ busy: false });
  const engine = { ...(cfg.data ?? {}), ...(draft as V) } as V;
  const prov = { ...(engine.provider as V), ...((draft.provider as V) ?? {}) } as V;
  const num = (k: string, d: number) => (typeof engine[k] === 'number' ? (engine[k] as number) : d);
  const str = (k: string, d: string) => (typeof engine[k] === 'string' ? (engine[k] as string) : d);
  const set = (k: string, v: unknown) => setDraft((d) => ({ ...d, [k]: v }));
  const setProv = (k: string, v: string) => setDraft((d) => ({ ...d, provider: { ...((d.provider as V) ?? {}), [k]: v } }));
  const dirty = Object.keys(draft).length > 0;

  const save = async () => {
    const r = await read<V>('system.config', { set: draft });
    if (r) {
      toast('ok', '设置已写入引擎');
      setDraft({});
      cfg.reload();
      close();
    }
  };
  const test = async () => {
    setProbe({ busy: true });
    const r = await read<V>('system.provider.probe', {}, { silent: true });
    setProbe(r ? { busy: false, r } : { busy: false, e: { e: '没有响应' } });
  };

  return (
    <Sheet
      title="设置"
      eyebrow="system.config"
      width={660}
      onClose={close}
      footer={
        <>
          <Btn variant="quiet" disabled={!dirty} onClick={() => setDraft({})}>
            放弃改动
          </Btn>
          <Btn variant="primary" icon="save" disabled={!dirty} onClick={() => void save()}>
            保存设置
          </Btn>
        </>
      }
    >
      <div className="stack" style={{ gap: 16 }}>
        <div className="box4">
          <Field label="自动保存间隔">
            <NumVal tag="S" value={num('autosaveSecs', 30)} min={0} max={600} dp={0} onChange={(v) => set('autosaveSecs', v)} />
          </Field>
          <Field label="导出目录">
            <TextInput value={str('exportDir', 'exports')} onChange={(v) => set('exportDir', v)} mono tip="相对工作区根目录" />
          </Field>
          <Field label="图像上限（百万像素）">
            <NumVal tag="M" value={num('maxImageMp', 40)} min={0.1} max={200} step={0.5} dp={1} onChange={(v) => set('maxImageMp', v)} />
          </Field>
          <Field label="最大导出倍率">
            <NumVal tag="X" value={num('renderScaleMax', 4)} min={1} max={8} dp={0} onChange={(v) => set('renderScaleMax', v)} />
          </Field>
          <Field label="撤销步数上限">
            <NumVal tag="N" value={num('historyCap', 200)} min={1} max={5000} dp={0} onChange={(v) => set('historyCap', v)} />
          </Field>
          <Field label="调用日志行数">
            <NumVal tag="L" value={num('logCapLines', 2000)} min={100} max={100000} dp={0} onChange={(v) => set('logCapLines', v)} />
          </Field>
        </div>

        <div className="sw-row">
          <span>记录每一次调用</span>
          <Switch on={engine.logCalls !== false} onChange={(v) => set('logCalls', v)} tip="接入页的时间线来源" />
        </div>

        <div className="sec-h">
          <b>模型 Provider</b>
          <Badge tone={prov.baseUrlSet ? 'ok' : 'default'}>{prov.baseUrlSet ? '已填地址' : '未配置'}</Badge>
          <Badge tone={(prov.apiKeySet as boolean) ?? engine.tokenSet ? 'ok' : 'default'}>{(prov.apiKeySet as boolean) ?? engine.tokenSet ? '密钥已就绪' : '无密钥'}</Badge>
        </div>
        <Field label="类型">
          <Select value={String(prov.name ?? '')} options={PROVIDERS} onChange={(v) => setProv('name', v)} ariaLabel="Provider 类型" />
        </Field>
        <Field label="服务地址">
          <TextInput value={String(prov.baseUrl ?? '')} onChange={(v) => setProv('baseUrl', v)} placeholder="http://127.0.0.1:11434" mono tip="本地服务用 http；不保存任何密钥" />
        </Field>
        <div className="box4">
          <Field label="模型名">
            <TextInput value={String(prov.model ?? '')} onChange={(v) => setProv('model', v)} placeholder="留空则用服务默认" mono />
          </Field>
          <Field label="密钥环境变量">
            <TextInput value={String(prov.apiKeyEnv ?? '')} onChange={(v) => setProv('apiKeyEnv', v)} placeholder="AGENTCANVAS_API_KEY" mono tip="只记名字，不记值" />
          </Field>
        </div>
        <div className="row" style={{ gap: 8, alignItems: 'center' }}>
          <Btn variant="quiet" icon="bolt" disabled={probe.busy} onClick={() => void test()}>
            {probe.busy ? '检测中' : '测试连接'}
          </Btn>
          {probe.r ? <Badge tone="ok">{String(probe.r.ms ?? '—')} ms · {String(probe.r.n_models ?? 0)} 个模型</Badge> : null}
          {probe.e ? <Badge tone="warn">{String((probe.e as V).e ?? '不可达')}</Badge> : null}
          {Array.isArray(probe.r?.models) ? (probe.r!.models as string[]).slice(0, 4).map((m) => <Chip key={m}>{m}</Chip>) : null}
        </div>

        <Field label="访问令牌">
          <div className="row" style={{ gap: 8, alignItems: 'center' }}>
            <TextInput
              value={String(draft.token ?? '')}
              onChange={(v) => set('token', v)}
              placeholder={engine.tokenSet ? '已设置 · 输入以替换' : '留空则仅回环可访问'}
              mono
              tip="HTTP / MCP 调用的 Bearer 令牌；写入后任何读取都只回 presence"
            />
            {engine.tokenSet ? (
              <Btn variant="quiet" onClick={() => set('token', null)} disabled>
                清除
              </Btn>
            ) : null}
          </div>
        </Field>

        <div className="sec-h">
          <b>本窗口</b>
          <Badge>仅本机</Badge>
        </div>
        <div className="stack-6">
          <div className="sw-row">
            <span>吸附到画板与相邻图层</span>
            <Switch
              on={state.prefs.snapping}
              onChange={(v) => dispatch({ t: 'prefs', prefs: { snapping: v } })}
              tip="拖动时的对齐辅助"
            />
          </div>
          <div className="sw-row">
            <span>显示安全区</span>
            <Switch on={state.prefs.safeArea} onChange={(v) => dispatch({ t: 'prefs', prefs: { safeArea: v } })} tip="画布上的虚线框" />
          </div>
        </div>

        <div className="row" style={{ gap: 8 }}>
          <Badge tone="blue">工作区 {String(state.caps?.paths?.root ?? '—')}</Badge>
          <Badge>引擎 v{state.health?.ver ?? '—'}</Badge>
          <Badge>{state.health?.fonts ?? 0} 个字体</Badge>
        </div>
      </div>
    </Sheet>
  );
}


/* ------------------------------------------------------------- confirm sheet */

function ConfirmSheet({ close, props }: { close: () => void; props: V }) {
  const { mutate, toast } = useApp();
  const [busy, setBusy] = useState(false);
  const action = props.action as { m: string; p: V };
  const run = async () => {
    setBusy(true);
    const r = await mutate<Mut>(action.m, action.p);
    setBusy(false);
    if (r) {
      toast('ok', String(props.done ?? '已完成'));
      close();
    }
  };
  return (
    <Sheet
      title={String(props.title ?? '确认操作')}
      eyebrow="不可逆"
      width={460}
      onClose={close}
      footer={
        <>
          <Btn variant="quiet" onClick={close}>
            取消
          </Btn>
          <Btn variant="danger" icon={props.danger ? 'trash' : 'check'} disabled={busy} onClick={() => void run()}>
            {busy ? '执行中' : String(props.confirm ?? (props.danger ? '删除' : '确认'))}
          </Btn>
        </>
      }
    >
      <div className="h3" style={{ lineHeight: 1.5, fontWeight: 500 }}>
        {String(props.body ?? '')}
      </div>
      <div className="copy-line" style={{ marginTop: 14 }}>
        <Icon name="bolt" size={14} />
        <span className="trunc" style={{ flex: 1 }}>
          {JSON.stringify({ m: action.m, ...action.p })}
        </span>
        <Badge tone="blue">{action.m}</Badge>
      </div>
    </Sheet>
  );
}

/* ------------------------------------------------------------- template sheet */

function TemplateSheet({ close, props }: { close: () => void; props: V }) {
  const { state, mutate, read, toast } = useApp();
  const doc = state.doc;
  const mode = String(props.mode ?? 'save');
  const board = findBoard(doc ?? null, state.board);
  const [name, setName] = useState(board ? `${board.name} 模板` : '我的模板');
  const [colors, setColors] = useState<string[]>(['#4B6BFB', '#FF7A59', '#22D3EE']);
  const [titles, setTitles] = useState(['第一张', '第二张', '第三张']);
  const [ratios, setRatios] = useState<string[]>(['1:1', '9:16']);
  const [busy, setBusy] = useState(false);
  const tplList = useRpc<{ tpl: string; d?: string; builtin?: boolean }[]>('template.list', {});

  const run = async () => {
    if (!doc || !board) return;
    setBusy(true);
    if (mode === 'save') {
      const r = await read<{ tpl?: string }>('template.save', { b: board.id, name });
      toast('ok', r?.tpl ? `模板「${r.tpl}」已保存` : '模板已保存');
    } else if (mode === 'variants') {
      const r = await mutate<{ boards?: string[] }>('template.variants', { from: doc.artboards.indexOf(board), colors, copy: { title: titles } });
      toast('ok', `已生成 ${r?.boards?.length ?? 0} 个变体画板`);
    } else {
      const r = await mutate<{ created?: string[] }>('artboard.duplicate', { b: board.id, to: ratios });
      toast('ok', `已复制成 ${ratios.join(' / ')}`);
      void r;
    }
    setBusy(false);
    tplList.reload();
    close();
  };

  const title = mode === 'save' ? '把画板存成模板' : mode === 'variants' ? '生成系列变体' : '复制成其他比例';

  return (
    <Sheet
      title={title}
      eyebrow={mode === 'save' ? 'template.save' : mode === 'variants' ? 'template.variants' : 'artboard.duplicate'}
      width={620}
      onClose={close}
      footer={
        <>
          <Btn variant="quiet" onClick={close}>
            取消
          </Btn>
          <Btn variant="primary" icon="wand" disabled={busy || !board} onClick={() => void run()}>
            {busy ? '执行中' : '执行'}
          </Btn>
        </>
      }
    >
      {!board ? (
        <Empty title="先选中一个画板" />
      ) : mode === 'save' ? (
        <div className="stack" style={{ gap: 14 }}>
          <Field label="模板名">
            <TextInput value={name} onChange={setName} tip="在模板面板里显示" />
          </Field>
          <div className="row" style={{ gap: 8 }}>
            <Badge tone="blue">{board.name}</Badge>
            <Badge>{board.layers.length} 层</Badge>
            <Badge>{tplList.data?.filter((t) => t.builtin).length ?? 0} 个内置版式可用</Badge>
          </div>
        </div>
      ) : mode === 'variants' ? (
        <div className="stack" style={{ gap: 14 }}>
          <Field label="主色">
            <div className="row" style={{ gap: 8, flexWrap: 'wrap' }}>
              {colors.map((c, i) => (
                <div key={i} className="row" style={{ gap: 4 }}>
                  <ColorBtn color={c} onChange={(v) => setColors((list) => list.map((x, k) => (k === i ? v : x)))} />
                  <IconBtn icon="close" size="sm" tip="移除这个配色" label="移除" disabled={colors.length < 2} onClick={() => setColors((list) => list.filter((_, k) => k !== i))} />
                </div>
              ))}
              <Btn size="sm" icon="plus" disabled={colors.length > 5} onClick={() => setColors((l) => [...l, '#8B5CF6'])}>
                加一色
              </Btn>
            </div>
          </Field>
          <Field label="每版标题">
            <div className="stack-6">
              {titles.map((t, i) => (
                <TextInput key={i} value={t} onChange={(v) => setTitles((list) => list.map((x, k) => (k === i ? v : x)))} tip={`第 ${i + 1} 版`} />
              ))}
            </div>
          </Field>
        </div>
      ) : (
        <div className="stack" style={{ gap: 14 }}>
          <Field label="目标比例">
            <div className="grid-chips">
              {['1:1', '4:5', '3:4', '16:9', '9:16', '16:5'].map((r) => (
                <Chip key={r} active={ratios.includes(r)} onClick={() => setRatios((v) => (v.includes(r) ? v.filter((x) => x !== r) : [...v, r]))} tip={`复制成 ${r}`}>
                  {r}
                </Chip>
              ))}
            </div>
          </Field>
          <div className="row" style={{ gap: 8 }}>
            <Badge tone="blue">{board.name}</Badge>
            <Badge>{ratios.length} 个新画板</Badge>
          </div>
        </div>
      )}
    </Sheet>
  );
}

/* ------------------------------------------------------------- explain sheet */

function ExplainSheet({ close, props }: { close: () => void; props: V }) {
  const { state } = useApp();
  const doc = state.doc;
  const [id, setId] = useState(String(props.id ?? ''));
  const r = useRpc<{
    name?: string;
    mode?: string;
    pinned?: boolean;
    order?: string[];
    steps?: { intent: string; won: boolean; note: string }[];
    box?: { x: number; y: number; w: number; h: number };
  }>('layout.explain', { p: doc?.project_id, id }, Boolean(doc && id));

  return (
    <Sheet
      title="这一层为什么在这里"
      eyebrow="layout.explain"
      width={560}
      onClose={close}
      footer={
        <div className="row" style={{ gap: 8, width: '100%' }}>
          <Select
            value={id}
            options={(doc?.nodes ?? []).map((n) => ({ v: n.id, label: n.name }))}
            onChange={setId}
            tip="换一个图层看"
            ariaLabel="图层"
          />
          <Btn variant="quiet" onClick={close}>
            关闭
          </Btn>
        </div>
      }
    >
      {!r.data ? (
        <Empty title="没有可解释的图层" />
      ) : (
        <div className="stack" style={{ gap: 14 }}>
          <div className="row" style={{ gap: 8, flexWrap: 'wrap' }}>
            <Badge tone="blue">{r.data.name ?? id}</Badge>
            <Badge>模式 {r.data.mode}</Badge>
            {r.data.pinned ? <Badge tone="warn">已钉住</Badge> : null}
            {r.data.box ? <Badge tone="default">{`${Math.round(r.data.box.x)}, ${Math.round(r.data.box.y)} · ${Math.round(r.data.box.w)}×${Math.round(r.data.box.h)}`}</Badge> : null}
          </div>
          <div className="stack-6">
            {(r.data.steps ?? []).map((s, i) => (
              <div key={i} className="row" style={{ gap: 10, padding: '9px 10px', borderRadius: 12, background: 'rgba(255,255,255,.4)', border: '1px solid rgba(255,255,255,.6)' }}>
                <span className="badge" style={{ minWidth: 22, justifyContent: 'center' }}>
                  {String(i + 1).padStart(2, '0')}
                </span>
                <span className="mono" style={{ fontSize: 12.5, color: 'var(--blue-deep)', minWidth: 96 }}>
                  {s.intent}
                </span>
                <span style={{ flex: '1 1 auto', fontSize: 13 }}>{s.note}</span>
                {s.won ? <Badge tone="ok">生效</Badge> : <Badge tone="default">被覆盖</Badge>}
              </div>
            ))}
            {!r.data.steps?.length ? <Empty title="这一层没有意图 · 坐标是手动的" /> : null}
          </div>
          <div className="row" style={{ gap: 6 }}>
            <Badge tone="default">求解顺序</Badge>
            <span className="mono" style={{ fontSize: 12.5, color: 'var(--t2)' }}>
              {(r.data.order ?? []).join(' → ')}
            </span>
          </div>
        </div>
      )}
    </Sheet>
  );
}

/* -------------------------------------------------------------- history sheet */

function HistorySheet({ close }: { close: () => void }) {
  const { state, mutate, read, toast } = useApp();
  const list = useRpc<HistoryEntry[]>('history.list', { p: state.pid, n: 40 }, Boolean(state.pid));
  const [snap, setSnap] = useState<string | null>(null);

  return (
    <Sheet
      title="改动轨迹"
      eyebrow="history.list"
      width={700}
      onClose={close}
      footer={
        <>
          <Btn
            size="sm"
            variant="quiet"
            icon="undo"
            onClick={() =>
              void mutate('history.undo', { n: 1 })
                .then(() => list.reload())
                .catch(() => undefined)
            }
          >
            撤销一步
          </Btn>
          <Btn
            size="sm"
            variant="quiet"
            icon="redo"
            onClick={() =>
              void mutate('history.redo', { n: 1 })
                .then(() => list.reload())
                .catch(() => undefined)
            }
          >
            重做一步
          </Btn>
          <span style={{ flex: 1 }} />
          <Btn
            size="sm"
            icon="save"
            onClick={async () => {
              const r = await read<{ snap?: string }>('project.snapshot', { label: '手动' });
              if (r?.snap) {
                setSnap(r.snap);
                toast('ok', `快照 ${r.snap}`);
              }
            }}
          >
            打个快照
          </Btn>
          <Btn variant="primary" onClick={close}>
            完成
          </Btn>
        </>
      }
    >
      <div className="stack" style={{ gap: 12 }}>
        {snap ? (
          <div className="row" style={{ gap: 8 }}>
            <Badge tone="ok">
              <Icon name="check" size={13} />
              快照 {snap}
            </Badge>
            <Btn
              size="sm"
              variant="quiet"
              icon="undo"
              onClick={() =>
                void read('project.restore', { p: state.pid, snap })
                  .then(() => {
                    toast('ok', '已回到快照');
                    list.reload();
                  })
                  .catch(() => undefined)
              }
            >
              回到这个快照
            </Btn>
          </div>
        ) : null}
        {list.data?.length ? (
          <div className="tbl-wrap">
            <table className="tbl">
              <thead>
                <tr>
                  <th>修订</th>
                  <th>方法</th>
                  <th>来源</th>
                  <th>改动</th>
                  <th>时间</th>
                </tr>
              </thead>
              <tbody>
                {list.data.map((h, i) => (
                  <tr key={i}>
                    <td className="mono">{h.rev}</td>
                    <td className="mono">{h.label}</td>
                    <td>
                      <Badge tone={h.origin === 'desktop' ? 'default' : h.origin === 'mcp' ? 'blue' : 'warn'}>{h.origin}</Badge>
                    </td>
                    <td className="mono" style={{ fontSize: 12 }}>
                      {(h.changed ?? []).length ? `${(h.changed ?? []).length} 个图层` : '—'}
                    </td>
                    <td className="t3">{h.at ? timeAgo(h.at) : '—'}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <Empty title="还没有改动记录" />
        )}
      </div>
    </Sheet>
  );
}

/* ------------------------------------------------------------------ help */

const KEYS: [string, string][] = [
  ['拖动画布', '移动选中图层，松开时提交 canvas.transform'],
  ['8 个控制点', '缩放，Shift 锁比例'],
  ['顶部圆点', '旋转，Shift 每 15° 吸附'],
  ['空格 / 中键拖动', '平移画布'],
  ['滚轮', '以光标为中心缩放'],
  ['框选', '空白处按下并拖动'],
  ['双击文字', '就地改文案'],
  ['⌘/Ctrl + D', '复制选中图层'],
  ['Del', '删除选中图层'],
  ['⌘/Ctrl + Z / ⇧Z', '撤销 / 重做'],
  ['方向键', '1px · Shift 10px'],
  ['⌘/Ctrl + S', '保存项目'],
];

function HelpSheet({ close, props }: { close: () => void; props: V }) {
  const caps = useRpc<Caps>('system.caps', {});
  const topic = String(props.topic ?? 'keys');
  return (
    <Sheet title={topic === 'keys' ? '画布操作' : '关于这版界面'} eyebrow="system.caps" width={620} onClose={close}>
      <div className="stack" style={{ gap: 16 }}>
        {topic === 'keys' ? (
          <div className="tbl-wrap">
            <table className="tbl">
              <tbody>
                {KEYS.map(([k, v]) => (
                  <tr key={k}>
                    <td style={{ width: 168 }}>
                      <span className="mono" style={{ fontSize: 12.5, fontWeight: 700 }}>
                        {k}
                      </span>
                    </td>
                    <td style={{ fontSize: 13 }}>{v}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <div className="stack-6">
            <div className="h3" style={{ fontWeight: 500, lineHeight: 1.6 }}>
              画布显示的就是引擎编译出来的 SVG，素材与字体走 <span className="mono" style={{ fontSize: 13 }}>/asset</span> 与 <span className="mono" style={{ fontSize: 13 }}>/font</span>，和导出用的是同一批字节。
            </div>
          </div>
        )}
        <div className="row" style={{ gap: 8, flexWrap: 'wrap' }}>
          <Badge tone="blue">{caps.data?.methods.length ?? 0} 个方法</Badge>
          <Badge>{caps.data?.node_types.length ?? 0} 种图层</Badge>
          <Badge>{caps.data?.intents.length ?? 0} 类意图</Badge>
          <Badge>{caps.data?.image_ops.length ?? 0} 个位图算子</Badge>
        </div>
        <div className="copy-line">
          <Icon name="link" size={14} />
          <span className="trunc" style={{ flex: 1 }}>
            {(caps.data?.loop ?? 'inspect.validate → canvas.batch → render.preview → render.export') as string}
          </span>
          <CopyBtn text={String(caps.data?.loop ?? '')} tip="复制这条调用链" />
        </div>
      </div>
    </Sheet>
  );
}

/* ------------------------------------------------------------------- host */

export function Sheets() {
  const { state, closeSheets } = useApp();
  const sheet = state.sheets[0];
  if (!sheet) return null;
  const props = (sheet.props ?? {}) as V;
  const close = () => closeSheets();
  switch (sheet.name) {
    case 'new':
      return <NewSheet close={close} preset={props.preset ? String(props.preset) : undefined} />;
    case 'export':
      return <ExportSheet close={close} />;
    case 'settings':
      return <SettingsSheet close={close} />;
    case 'confirm':
      return <ConfirmSheet close={close} props={props} />;
    case 'template':
      return <TemplateSheet close={close} props={props} />;
    case 'explain':
      return <ExplainSheet close={close} props={props} />;
    case 'history':
      return <HistorySheet close={close} />;
    case 'help':
      return <HelpSheet close={close} props={props} />;
    default:
      return null;
  }
}
