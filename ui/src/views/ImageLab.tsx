/** 图片处理 / Image Lab: the raster half. Files in, an op chain, a live
 *  before/after, presets and a batch table — all driven by `image.*`. */

import { useCallback, useEffect, useRef, useState } from 'react';
import { absUrl, fileUrl, useRpc } from '../api';
import { useApp } from '../state/app';
import type { ImageOut, ImageProbe, OpDef, PresetDef } from '../types';
import { ChainEditor, type Step } from '../components/ChainEditor';
import { Glass } from '../components/Glass';
import { Badge, Btn, Chip, Empty, Field, IconBtn, Range, Select, Switch, TextInput } from '../components/Primitives';
import { Icon } from '../components/Icons';
import { baseName, fmtBytes, stem } from '../lib/format';

interface LabFile {
  path: string;
  name: string;
  w: number;
  h: number;
  bytes: number;
  fmt: string;
  url?: string;
}

type BatchRow = { i: number; name: string; ok: boolean; out?: string; url?: string; bytes?: number; e?: string };

function before(f: LabFile | null): string {
  if (!f) return '';
  return f.url ?? fileUrl(f.path);
}

export function ImageLab() {
  const { state, dispatch, read, toast } = useApp();
  const lab = state.lab;
  const [pick, setPick] = useState<number | null>(0);
  const [busy, setBusy] = useState(false);
  const [over, setOver] = useState(false);
  const [batchBusy, setBatchBusy] = useState(false);
  const fileIn = useRef<HTMLInputElement | null>(null);
  const splitter = useRef<HTMLDivElement | null>(null);

  const caps = state.caps;
  const opDefs = useRpc<OpDef[]>('image.ops', {});
  const presetDefs = useRpc<PresetDef[]>('image.presets', {});
  const ops = opDefs.data?.map((o) => o.op) ?? caps?.image_ops ?? [];
  const presets = presetDefs.data ?? (caps?.image_presets ?? []).map((name) => ({ name, d: name }));

  const active = lab.files.find((f) => f.path === lab.active) ?? null;

  const addPaths = useCallback(
    async (paths: string[], previews: (string | undefined)[] = []) => {
      const added: LabFile[] = [];
      for (let i = 0; i < paths.length; i += 1) {
        const path = paths[i];
        const probe = await read<ImageProbe>('image.probe', { path }, { silent: true });
        added.push({
          path,
          name: baseName(path),
          w: probe?.w ?? 0,
          h: probe?.h ?? 0,
          bytes: probe?.bytes ?? 0,
          fmt: probe?.fmt ?? 'PNG',
          url: probe?.url ? absUrl(probe.url) : previews[i] ?? fileUrl(path),
        });
      }
      if (!added.length) return;
      dispatch({
        t: 'lab',
        patch: { files: [...lab.files, ...added], active: lab.active ?? added[0].path },
      });
      toast('ok', `已加入 ${added.length} 张图`);
    },
    [dispatch, lab.files, read, toast],
  );

  /* live: any chain / file change re-runs the chain on the active file */
  const runChain = useCallback(
    async (silent = false) => {
      if (!active || !lab.chain.length) return;
      if (!silent) setBusy(true);
      const out = `${lab.outDir}/${stem(active.name)}.${lab.fmt}`;
      const r = await read<ImageOut & { url?: string }>('image.chain', {
        in: active.path,
        out,
        chain: lab.chain.map((s) => ({ op: s.op, ...s.args })),
        fmt: lab.fmt,
        q: lab.q,
      });
      if (!silent) setBusy(false);
      if (r) {
        dispatch({
          t: 'lab',
          patch: { result: { out: r.out, url: r.url ? absUrl(r.url) : fileUrl(r.out), bytes: r.bytes, sha: r.sha, w: Array.isArray(r.w) ? r.w[0] : active.w, h: Array.isArray(r.w) ? r.w[1] : active.h } },
        });
      }
    },
    [active, dispatch, lab.chain, lab.fmt, lab.outDir, lab.q, read],
  );

  useEffect(() => {
    const t = window.setTimeout(() => void runChain(true), 420);
    return () => window.clearTimeout(t);
  }, [runChain]);

  const runPreset = async (name: string) => {
    if (!active) {
      toast('info', '先加入一张图');
      return;
    }
    const r = await read<ImageOut & { url?: string }>('image.preset', { name, in: active.path, out: `${lab.outDir}/${stem(active.name)}.${name}.${lab.fmt}` });
    if (r) {
      dispatch({ t: 'lab', patch: { result: { out: r.out, url: r.url ? absUrl(r.url) : fileUrl(r.out), bytes: r.bytes, sha: r.sha, w: active.w, h: active.h } } });
      toast('ok', `预设 ${name} 已应用`);
    }
  };

  const runBatch = async () => {
    if (!lab.files.length) return;
    setBatchBusy(true);
    const r = await read<{ r?: { i: number; ok: boolean; out?: string; url?: string; bytes?: number; e?: string }[] }>('image.batch', {
      paths: lab.files.map((f) => f.path),
      chain: lab.chain.map((s) => ({ op: s.op, ...s.args })),
      out: lab.outDir,
      fmt: lab.fmt,
      q: lab.q,
    });
    setBatchBusy(false);
    if (!r?.r) return;
    const rows: BatchRow[] = r.r.map((x, i) => ({
      i,
      name: lab.files[i]?.name ?? `#${i}`,
      ok: x.ok !== false,
      out: x.out,
      url: x.url ? absUrl(x.url) : x.out ? fileUrl(x.out) : undefined,
      bytes: x.bytes,
      e: x.e,
    }));
    dispatch({ t: 'lab', patch: { rows } });
    toast(rows.some((x) => !x.ok) ? 'info' : 'ok', `批处理完成 · ${rows.filter((x) => x.ok).length}/${rows.length}`);
  };

  const splitDrag = (e: React.PointerEvent) => {
    const el = splitter.current;
    if (!el) return;
    const move = (ev: PointerEvent) => {
      const r = el.getBoundingClientRect();
      dispatch({ t: 'lab', patch: { split: Math.max(0, Math.min(100, ((ev.clientX - r.left) / r.width) * 100)) } });
    };
    move(e.nativeEvent);
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };

  const dropFiles = (files: FileList | null) => {
    if (!files?.length) return;
    const list = Array.from(files);
    void addPaths(
      list.map((f) => f.webkitRelativePath || f.name),
      list.map((f) => URL.createObjectURL(f)),
    );
  };

  return (
    <div className="lab">
      {/* header */}
      <div className="row-b" style={{ flexWrap: 'wrap', gap: 12 }}>
        <div>
          <div className="row" style={{ gap: 8, marginBottom: 6 }}>
            <Badge tone="blue">
              <Icon name="crop" size={13} />
              image.chain
            </Badge>
            <Badge>{ops.length} 个算子</Badge>
            {lab.files.length ? <Badge tone="ok">{lab.files.length} 张</Badge> : null}
          </div>
          <h1 className="h1">图片处理</h1>
        </div>
        <div className="row" style={{ gap: 8 }}>
          <input ref={fileIn} type="file" accept="image/*" multiple style={{ display: 'none' }} onChange={(e) => dropFiles(e.target.files)} />
          <Btn variant="primary" icon="plus" onClick={() => fileIn.current?.click()}>
            添加图片
          </Btn>
          <Btn icon="bolt" variant="quiet" disabled={!lab.files.length || batchBusy} onClick={() => void runBatch()}>
            {batchBusy ? '批处理中' : '全部批处理'}
          </Btn>
        </div>
      </div>

      {/* 01 input + output settings */}
      <section className="sec">
        <div className="sec-h">
          <h2>输入与输出</h2>
          <span className="sec-no">01</span>
        </div>
        <div className="lab-top">
          <Glass depth={2} className="card" data-sheen>
            <div
              className={`drop ${over ? 'is-over' : ''}`}
              onDragOver={(e) => {
                e.preventDefault();
                setOver(true);
              }}
              onDragLeave={() => setOver(false)}
              onDrop={(e) => {
                e.preventDefault();
                setOver(false);
                dropFiles(e.dataTransfer.files);
              }}
            >
              <Icon name="image" size={26} />
              <div style={{ fontSize: 16, fontWeight: 800, letterSpacing: '-0.02em' }}>把图片拖进来</div>
              <div className="row" style={{ gap: 8 }}>
                <Btn size="sm" onClick={() => fileIn.current?.click()}>
                  选择文件
                </Btn>
                <Chip
                  onClick={() =>
                    void addPaths([
                      'assets/milk-bottle.png',
                      'assets/pour-over.png',
                      'assets/sneaker-night.png',
                      'assets/broken.heic',
                    ])
                  }
                  tip="用工作区里已有的文件试一次"
                >
                  用示例文件
                </Chip>
                <Btn size="sm" variant="quiet" disabled={!lab.files.length} onClick={() => dispatch({ t: 'lab', patch: { files: [], active: null, result: null, rows: [] } })}>
                  清空
                </Btn>
              </div>
            </div>

            {lab.files.length ? (
              <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(126px, 1fr))', gap: 10, marginTop: 12 }}>
                {lab.files.map((f) => (
                  <div
                    key={f.path}
                    className={`glass glass-inset ${lab.active === f.path ? 'is-on' : ''}`}
                    style={{ padding: 6, borderRadius: 14, cursor: 'pointer', borderColor: lab.active === f.path ? 'rgba(75,107,251,.65)' : undefined }}
                    onClick={() => dispatch({ t: 'lab', patch: { active: f.path } })}
                    data-tip={`${f.path} · ${f.w}×${f.h}`}
                    data-sheen
                  >
                    <div className="thumb thumb-sq" style={{ borderRadius: 10, background: '#fff' }}>
                      <img src={before(f)} alt={f.name} />
                    </div>
                    <div className="trunc" style={{ fontSize: 13, fontWeight: 700, marginTop: 6 }}>
                      {f.name}
                    </div>
                    <div className="row" style={{ gap: 4, marginTop: 2 }}>
                      <span className="t3" style={{ fontSize: 13 }}>
                        {f.w}×{f.h}
                      </span>
                      <button
                        type="button"
                        className="li-x"
                        style={{ marginLeft: 'auto' }}
                        aria-label="移出列表"
                        data-tip="移出列表"
                        onClick={(e) => {
                          e.stopPropagation();
                          const files = lab.files.filter((x) => x.path !== f.path);
                          dispatch({ t: 'lab', patch: { files, active: files[0]?.path ?? null } });
                        }}
                      >
                        <Icon name="close" size={12} sw={2} />
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            ) : null}
          </Glass>

          <Glass depth={2} className="card" data-sheen>
            <div className="card-h">
              <h3>输出</h3>
              <Badge tone="blue">{lab.fmt.toUpperCase()}</Badge>
            </div>
            <div className="stack-6">
              <Field label="格式">
                <div className="grid-chips">
                  {['png', 'jpg', 'webp'].map((f) => (
                    <Chip key={f} active={lab.fmt === f} onClick={() => dispatch({ t: 'lab', patch: { fmt: f } })} tip={`输出为 ${f.toUpperCase()}`}>
                      {f.toUpperCase()}
                    </Chip>
                  ))}
                </div>
              </Field>
              <Field label={`质量 ${lab.q}`}>
                <Range value={lab.q} min={40} max={100} onChange={(v) => dispatch({ t: 'lab', patch: { q: v } })} tip="image.chain 的 q 参数" />
              </Field>
              <Field label="输出目录">
                <TextInput value={lab.outDir} onChange={(v) => dispatch({ t: 'lab', patch: { outDir: v } })} mono tip="相对工作区根目录" />
              </Field>
              <div className="sw-row">
                <span>结果写盘后保留原图</span>
                <Switch
                  on
                  onChange={() => toast('info', '引擎始终写入新文件，不覆盖输入')}
                  tip="image.chain 的 out 是新路径"
                />
              </div>
              <div className="row" style={{ gap: 6, marginTop: 4 }}>
                <Btn size="sm" icon="bolt" disabled={!lab.files.length || batchBusy} onClick={() => void runBatch()}>
                  批处理全部
                </Btn>
                <IconBtn
                  icon="refresh"
                  size="sm"
                  tip="用当前链条重跑一次"
                  label="重跑"
                  disabled={!active}
                  onClick={() => void runChain()}
                />
              </div>
            </div>
          </Glass>
        </div>
      </section>

      {/* 02 chain */}
      <section className="sec">
        <div className="sec-h">
          <h2>处理链条</h2>
          <Badge tone="blue">{lab.chain.length} 步</Badge>
          <span style={{ marginRight: 'auto' }} />
          <span className="mono" style={{ fontSize: 12.5, color: 'var(--t3)' }}>
            chain → image.chain
          </span>
          <span className="sec-no">02</span>
        </div>
        <Glass depth={2} className="card" data-sheen>
          <ChainEditor
            chain={lab.chain as Step[]}
            ops={ops}
            active={pick}
            busy={busy}
            presets={presets}
            onChange={(c) => dispatch({ t: 'lab', patch: { chain: c } })}
            onPick={setPick}
            onRun={() => void runChain()}
            onPreset={(n) => void runPreset(n)}
          />
        </Glass>
      </section>

      {/* 03 result */}
      <section className="sec">
        <div className="sec-h">
          <h2>对比与批量结果</h2>
          <span className="sec-no">03</span>
        </div>
        <div className="lab-mid">
          <Glass depth={2} className="card" data-sheen>
            <div className="card-h">
              <h3>前后对比</h3>
              {busy ? <Badge tone="blue">处理中</Badge> : lab.result ? <Badge tone="ok">{fmtBytes(lab.result.bytes)}</Badge> : null}
            </div>
            {active ? (
              <>
                <div
                  className="ba"
                  ref={splitter}
                  style={{ ['--split' as string]: `${lab.split}%` }}
                  onPointerDown={splitDrag}
                  data-tip="拖动分割线"
                >
                  <img src={before(active)} alt="原图" />
                  {lab.result ? <img className="after" src={lab.result.url} alt="处理后" /> : null}
                  <span className="ba-line" />
                  <span className="ba-knob">
                    <Icon name="swap" size={15} />
                  </span>
                  <span className="ba-tag" style={{ left: 10 }}>
                    原图
                  </span>
                  <span className="ba-tag" style={{ right: 10 }}>
                    {lab.result ? `${lab.result.w}×${lab.result.h}` : '未处理'}
                  </span>
                </div>
                <div className="row" style={{ marginTop: 10, gap: 8, flexWrap: 'wrap' }}>
                  <span className="mono" style={{ fontSize: 12.5, color: 'var(--t2)' }}>
                    {lab.result?.out ?? '—'}
                  </span>
                  <span style={{ marginLeft: 'auto' }} />
                  {lab.result ? (
                    <>
                      <Badge tone="default">sha {lab.result.sha.slice(0, 8)}</Badge>
                      <Btn size="sm" icon="folder" onClick={() => window.open(lab.result?.url ?? '', '_blank')} tip="打开处理后的图片">
                        打开结果
                      </Btn>
                    </>
                  ) : null}
                </div>
              </>
            ) : (
              <Empty
                title="还没有选中图片"
                action={
                  <Btn variant="primary" icon="plus" onClick={() => fileIn.current?.click()}>
                    添加图片
                  </Btn>
                }
              />
            )}
          </Glass>

          <Glass depth={2} className="card" data-sheen>
            <div className="card-h">
              <h3>批量结果</h3>
              <Badge tone={lab.rows.length && lab.rows.every((r) => r.ok) ? 'ok' : lab.rows.length ? 'warn' : 'default'}>
                {lab.rows.filter((r) => r.ok).length}/{lab.rows.length || 0}
              </Badge>
            </div>
            {lab.rows.length ? (
              <div className="tbl-wrap">
                <table className="tbl">
                  <thead>
                    <tr>
                      <th>文件</th>
                      <th>结果</th>
                      <th>大小</th>
                      <th />
                    </tr>
                  </thead>
                  <tbody>
                    {lab.rows.map((r) => (
                      <tr key={`${r.i}-${r.name}`} className={r.ok ? '' : 'is-bad'}>
                        <td className="trunc" style={{ maxWidth: 150 }}>
                          {r.name}
                        </td>
                        <td>
                          {r.ok ? <Badge tone="ok">已输出</Badge> : <Badge tone="err">{r.e ?? '失败'}</Badge>}
                        </td>
                        <td className="mono" style={{ fontSize: 12 }}>
                          {r.bytes ? fmtBytes(r.bytes) : '—'}
                        </td>
                        <td style={{ textAlign: 'right' }}>
                          {r.url ? (
                            <Btn size="sm" variant="quiet" icon="folder" onClick={() => window.open(r.url, '_blank')} tip="打开这一张的结果">
                              打开
                            </Btn>
                          ) : null}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            ) : (
              <Empty
                title="还没有批处理结果"
                action={
                  <Btn variant="primary" icon="bolt" disabled={!lab.files.length} onClick={() => void runBatch()}>
                    批处理 {lab.files.length} 张
                  </Btn>
                }
              />
            )}
            <div className="grp" style={{ marginTop: 8 }}>
              <CompareCard paths={lab.files.map((f) => f.path)} />
            </div>
          </Glass>
        </div>
      </section>
    </div>
  );
}

/** `image.compare` needs two real files; it reports a 0-100 score. */
function CompareCard({ paths }: { paths: string[] }) {
  const { read } = useApp();
  const list = paths;
  const [a, setA] = useState('');
  const [b, setB] = useState('');
  const [out, setOut] = useState<{ score: number; mae: number[] } | null>(null);

  useEffect(() => {
    if (list.length >= 2) {
      setA((v) => (list.includes(v) ? v : list[0]));
      setB((v) => (list.includes(v) && v !== (list.includes(list[0]) ? list[0] : '') ? v : list[1] ?? ''));
    }
  }, [list]);

  const opts = list.map((p) => ({ v: p, label: baseName(p) }));
  return (
    <>
      <div className="grp-h">对比两张结果</div>
      <div className="stack-6">
        <div className="box4">
          <Select value={a} options={opts} onChange={setA} tip="第一张" ariaLabel="第一张" />
          <Select value={b} options={opts} onChange={setB} tip="第二张" ariaLabel="第二张" />
        </div>
        <div className="row" style={{ gap: 8 }}>
          <Btn
            size="sm"
            icon="sliders"
            disabled={list.length < 2 || !a || !b || a === b}
            tip="调用 image.compare"
            onClick={() => void read<{ score: number; mae: number[] }>('image.compare', { a, b2: b }).then((r) => r && setOut(r))}
          >
            比较
          </Btn>
          {out ? (
            <Badge tone="blue">
              相似度 {out.score.toFixed(1)} · 平均误差 {out.mae.map((x) => x.toFixed(1)).join(' / ')}
            </Badge>
          ) : null}
        </div>
      </div>
    </>
  );
}
