/** 接入 / Access: the agent-facing half. MCP config, HTTP examples, the live tool
 *  list, the call log and the job table with cancel. */

import { useEffect, useMemo, useState } from 'react';
import { HTTP_BASE, MOCK, useRpc } from '../api';
import { useApp } from '../state/app';
import type { Doctor, Job, LogEntry, McpTool, V } from '../types';
import { Glass } from '../components/Glass';
import { Badge, Btn, Chip, CopyBtn, Empty, Select, Switch, TextInput } from '../components/Primitives';
import { Icon } from '../components/Icons';
import { fmtBytes, ms as fms, timeAgo } from '../lib/format';

function ToolRow({ t }: { t: McpTool }) {
  const [open, setOpen] = useState(false);
  const props = Object.entries(t.inputSchema.properties ?? {});
  return (
    <div className="call">
      <button type="button" className="call-h" onClick={() => setOpen((v) => !v)}>
        <span className="call-m">{t.name}</span>
        <Badge tone="blue">{t.method}</Badge>
        <span className="spacer" />
        <span className="mono" style={{ fontSize: 12, color: 'var(--t3)' }}>
          {props.length} 参数
        </span>
        <Icon name="chevron" size={13} style={{ transform: open ? 'rotate(90deg)' : undefined, transition: 'transform var(--d1) var(--ease)' }} />
      </button>
      {open ? (
        <div className="call-b">
          <span style={{ fontSize: 13, color: 'var(--t2)' }}>{t.description}</span>
          <div className="tbl-wrap">
            <table className="tbl">
              <thead>
                <tr>
                  <th>参数</th>
                  <th>类型</th>
                  <th>必填</th>
                </tr>
              </thead>
              <tbody>
                {props.map(([k, v]) => (
                  <tr key={k}>
                    <td className="mono">{k}</td>
                    <td className="mono" style={{ fontSize: 12 }}>
                      {String(v.type ?? 'any')}
                    </td>
                    <td>{(t.inputSchema.required ?? []).includes(k) ? <Badge tone="err">必填</Badge> : <Badge tone="default">可选</Badge>}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      ) : null}
    </div>
  );
}

export function Access() {
  const { state, read, toast, openSheet } = useApp();
  const [q, setQ] = useState('');
  const [n, setN] = useState(40);
  const [auto, setAuto] = useState(true);
  const [tick, setTick] = useState(0);
  const tools = useRpc<McpTool[]>('system.tools', { full: 1 });
  const log = useRpc<LogEntry[]>('log', { n });
  const jobs = useRpc<Job[]>('jobs', {});
  const doctor = useRpc<Doctor>('system.doctor', {});
  const caps = useRpc<V>('system.caps', {});

  useEffect(() => {
    if (!auto) return;
    const t = window.setInterval(() => setTick((v) => v + 1), 4000);
    return () => window.clearInterval(t);
  }, [auto]);

  useEffect(() => {
    if (!tick) return;
    log.reload();
    jobs.reload();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tick]);

  const port = state.health?.port ?? state.caps?.port ?? Number(caps.data?.port ?? 8899);
  const base = `http://127.0.0.1:${port}`;
  /* `caps.paths.root` is "" whenever the engine runs with the default root, so
     prefer the resolved absolute path from system.health / system.doctor. */
  const root = String(state.health?.root || doctor.data?.root || state.caps?.paths?.root || '~/AgentCanvas');

  const mcp = useMemo(
    () =>
      JSON.stringify(
        {
          mcpServers: {
            agentcanvas: {
              command: 'agentcanvas',
              args: ['mcp', '--port', port],
              env: { AGENTCANVAS_ROOT: root },
            },
          },
        },
        null,
        2,
      ),
    [port, root],
  );

  const examples = useMemo(() => {
    const poster = {
      m: 'poster',
      p: { title: '夏日轻乳', subtitle: '生牛乳发酵', accent: '#4B6BFB', preset: 'poster', f: 'png', out: 'agent' },
    };
    const batch = {
      m: 'canvas.batch',
      p: { p: state.pid ?? 'p_summer01', cmds: [{ t: 'Transform', id: 'n_photo', x: 252, y: 336, w: 576, h: 720 }] },
    };
    return [
      { label: '一次调用出图', code: `curl -s -X POST ${base}/rpc -H 'content-type: application/json' -d '${JSON.stringify(poster)}'` },
      { label: '读整份文档', code: `curl -s -X POST ${base}/rpc -d '{"m":"project.get","p":{"p":"${state.pid ?? 'p_summer01'}","depth":1}}'` },
      { label: '一条事务改多个图层', code: `curl -s -X POST ${base}/rpc -d '${JSON.stringify(batch)}'` },
      { label: '能力清单', code: `curl -s ${base}/caps` },
      { label: 'MCP 工具表', code: `curl -s ${base}/tools` },
      { label: '预览用的素材与字体', code: `${base}/asset?id=a_milkbottle\n${base}/font?family=PingFang SC` },
    ];
  }, [base, state.pid]);

  const filtered = (tools.data ?? []).filter(
    (t) => !q || t.name.toLowerCase().includes(q.toLowerCase()) || t.method.toLowerCase().includes(q.toLowerCase()),
  );

  const refreshAll = () => {
    tools.reload();
    log.reload();
    jobs.reload();
    doctor.reload();
  };

  return (
    <div className="acc">
      <div className="page-in" style={{ maxWidth: 1280, margin: '0 auto', display: 'flex', flexDirection: 'column', gap: 26 }}>
        <div className="row-b" style={{ flexWrap: 'wrap', gap: 12 }}>
          <div>
            <div className="row" style={{ gap: 8, marginBottom: 6, flexWrap: 'wrap' }}>
              <Badge tone="ok">
                <Icon name="check" size={13} />
                仅回环监听
              </Badge>
              <Badge tone="blue">127.0.0.1:{port}</Badge>
              <Badge>{(tools.data ?? []).length} 个 MCP 工具</Badge>
              <Badge>{state.health?.fonts ?? 0} 个字体</Badge>
              {MOCK ? <Badge tone="warn">预览数据</Badge> : null}
            </div>
            <h1 className="h1">Agent 接入</h1>
          </div>
          <div className="row" style={{ gap: 8 }}>
            <div className="row" style={{ gap: 6 }}>
              <span style={{ fontSize: 13, color: 'var(--t2)' }}>自动刷新</span>
              <Switch on={auto} onChange={setAuto} tip="每 4 秒重读工具表 / 调用日志 / 任务" />
            </div>
            <Btn icon="refresh" onClick={refreshAll} tip="立即重读">
              刷新
            </Btn>
          </div>
        </div>

        {/* 01 · MCP */}
        <section className="sec">
          <div className="sec-h">
            <h2>MCP 配置</h2>
            <Badge tone="blue">stdio</Badge>
            <span className="sec-no">01</span>
          </div>
          <div className="row2">
            <Glass depth={2} className="card" data-sheen>
              <div className="card-h">
                <h3>agentcanvas mcp</h3>
                <CopyBtn text={mcp} label="复制 JSON" />
              </div>
              <pre className="code">{mcp}</pre>
              <div className="row" style={{ gap: 6, marginTop: 12, flexWrap: 'wrap' }}>
                <Chip onClick={() => void read('clear', {}).then(() => { toast('ok', '调用日志已清空'); log.reload(); })} tip="清空 log 与 jobs">
                  清空日志
                </Chip>
                <Chip
                  onClick={() =>
                    void read<{ p?: string; files?: unknown[] }>('system.demo', { o: 'demo' }).then((r) => {
                      if (r?.p) {
                        toast('ok', `示例已生成 · ${r.p}`);
                        log.reload();
                        jobs.reload();
                      }
                    })
                  }
                  tip="跑一次 system.demo，看整条链路"
                >
                  运行示例海报
                </Chip>
                <Chip onClick={() => openSheet('settings')} tip="写入 system.config">
                  打开设置
                </Chip>
              </div>
            </Glass>

            <Glass depth={2} className="card" data-sheen>
              <div className="card-h">
                <h3>自检</h3>
                <Badge tone={doctor.data?.ok ? 'ok' : 'warn'}>{doctor.data?.ok ? '通过' : '有警告'}</Badge>
              </div>
              <div className="stack-6">
                <div className="kv">
                  <span>工作区可写</span>
                  <b>{doctor.data?.writable ? 'yes' : 'no'}</b>
                </div>
                <div className="kv">
                  <span>字体数</span>
                  <b>{doctor.data?.fonts ?? 0}</b>
                </div>
                <div className="kv">
                  <span>中文字体</span>
                  <b>{doctor.data?.cjk_font ? '可用' : '缺失'}</b>
                </div>
                <div className="kv">
                  <span>HTTP</span>
                  <b>{doctor.data?.http ? `:${doctor.data.port}` : '未启动'}</b>
                </div>
                <div className="kv">
                  <span>版本</span>
                  <b>{doctor.data?.version ?? '—'}</b>
                </div>
              </div>
              <div className="row" style={{ marginTop: 12, gap: 6 }}>
                <Btn size="sm" icon="refresh" onClick={() => doctor.reload()}>
                  重新自检
                </Btn>
                {(doctor.data?.warnings ?? []).length ? <Badge tone="warn">{doctor.data?.warnings.length} 条警告</Badge> : null}
              </div>
            </Glass>
          </div>
        </section>

        {/* 02 · HTTP */}
        <section className="sec">
          <div className="sec-h">
            <h2>HTTP 示例</h2>
            <Badge tone="blue">POST /rpc</Badge>
            <span className="sec-no">02</span>
          </div>
          <Glass depth={2} className="card" data-sheen>
            <div className="stack" style={{ gap: 12 }}>
              {examples.map((e) => (
                <div key={e.label}>
                  <div className="row" style={{ marginBottom: 6 }}>
                    <span className="h3">{e.label}</span>
                    <span className="spacer" />
                    <CopyBtn text={e.code} tip="复制这条命令" />
                  </div>
                  <pre className="code">{e.code}</pre>
                </div>
              ))}
            </div>
          </Glass>
        </section>

        {/* 03 · tools */}
        <section className="sec">
          <div className="sec-h">
            <h2>工具表</h2>
            <Badge tone="blue">{filtered.length}</Badge>
            <span style={{ marginRight: 'auto' }} />
            <div style={{ width: 260 }}>
              <TextInput value={q} onChange={setQ} placeholder="按名称或方法过滤" tip="system.tools" />
            </div>
            <span className="sec-no">03</span>
          </div>
          <Glass depth={2} className="card" data-sheen>
            {filtered.length ? (
              <div className="stack-6" style={{ maxHeight: 420, overflow: 'auto' }}>
                {filtered.map((t) => (
                  <ToolRow key={t.name} t={t} />
                ))}
              </div>
            ) : (
              <Empty title="没有匹配的工具" action={<Btn variant="primary" icon="close" onClick={() => setQ('')}>清空筛选</Btn>} />
            )}
          </Glass>
        </section>

        {/* 04 · jobs */}
        <section className="sec">
          <div className="sec-h">
            <h2>任务</h2>
            <Badge tone={(jobs.data ?? []).some((j) => j.ok === false) ? 'warn' : 'ok'}>{(jobs.data ?? []).length}</Badge>
            <span className="sec-no">04</span>
          </div>
          <Glass depth={2} className="card" data-sheen>
            {(jobs.data ?? []).length ? (
              <div className="stack" style={{ gap: 12 }}>
                {jobs.data?.map((j) => (
                  <div key={j.id}>
                    <div className="row-b" style={{ marginBottom: 6 }}>
                      <span className="row" style={{ gap: 8 }}>
                        <span className="badge">{j.kind}</span>
                        <span style={{ fontSize: 13, fontWeight: 700 }}>{j.label}</span>
                      </span>
                      <span className="row" style={{ gap: 8 }}>
                        <span className="mono" style={{ fontSize: 12, color: 'var(--t3)' }}>
                          {j.stage} · {Math.round(j.done * 100)}%
                        </span>
                        {j.ok === null || j.ok === undefined ? (
                          <Btn
                            size="sm"
                            variant="danger"
                            icon="stop"
                            onClick={() =>
                              void read<{ ok?: boolean }>('cancel', { id: j.id }).then((r) => {
                                toast(r?.ok ? 'info' : 'err', r?.ok ? '任务已取消' : '引擎没有找到这个任务');
                                jobs.reload();
                              })
                            }
                            tip="cancel {id} — 停掉这个任务"
                          >
                            取消
                          </Btn>
                        ) : (
                          <Badge tone={j.ok ? 'ok' : 'err'}>{j.ok ? '完成' : '失败'}</Badge>
                        )}
                      </span>
                    </div>
                    <div className={`bar ${j.ok === true ? 'done' : j.ok === false ? 'err' : ''}`}>
                      <i style={{ width: `${Math.min(100, Math.max(0, j.done * 100))}%` }} />
                    </div>
                  </div>
                ))}
              </div>
            ) : (
              <Empty title="没有正在跑的任务" action={<Btn variant="primary" icon="wand" onClick={() => void read('system.demo', { o: 'demo' }).then(() => jobs.reload())}>运行一次示例</Btn>} />
            )}
          </Glass>
        </section>

        {/* 05 · log */}
        <section className="sec">
          <div className="sec-h">
            <h2>调用日志</h2>
            <Badge tone="blue">{(log.data ?? []).length}</Badge>
            <span style={{ marginRight: 'auto' }} />
            <div style={{ width: 150 }}>
              <Select
                value={String(n)}
                options={[
                  { v: '20', label: '最近 20 条' },
                  { v: '40', label: '最近 40 条' },
                  { v: '80', label: '最近 80 条' },
                ]}
                onChange={(v) => setN(Number(v))}
                tip="log 的 n 参数"
                ariaLabel="条数"
              />
            </div>
            <span className="sec-no">05</span>
          </div>
          <Glass depth={2} className="card" data-sheen>
            {(log.data ?? []).length ? (
              <div className="tbl-wrap">
                <table className="tbl">
                  <thead>
                    <tr>
                      <th>方法</th>
                      <th>来源</th>
                      <th>耗时</th>
                      <th>结果</th>
                      <th>时间</th>
                    </tr>
                  </thead>
                  <tbody>
                    {log.data?.map((e, i) => (
                      <tr key={`${e.m}-${i}`} className={e.ok ? '' : 'is-bad'}>
                        <td className="mono">{e.m}</td>
                        <td>
                          <Badge tone={e.src === 'desktop' ? 'default' : e.src === 'mcp' ? 'blue' : 'warn'}>{e.src ?? '—'}</Badge>
                        </td>
                        <td className="mono">{e.ms ? fms(e.ms) : '—'}</td>
                        <td>{e.ok ? <Badge tone="ok">ok</Badge> : <Badge tone="err">{e.e || 'error'}</Badge>}</td>
                        <td className="t3">{e.t ? timeAgo(e.t) : '—'}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            ) : (
              <Empty title="还没有调用记录" action={<Btn variant="primary" icon="bolt" onClick={() => void read('project.list', {}).then(() => log.reload())}>产生一条</Btn>} />
            )}
            <div className="row" style={{ gap: 8, marginTop: 12 }}>
              <Badge tone="default">{`${HTTP_BASE}/rpc`}</Badge>
              <span className="spacer" />
              <Btn size="sm" variant="quiet" icon="trash" onClick={() => void read('clear', {}).then(() => { toast('ok', '已清空'); log.reload(); })}>
                清空
              </Btn>
            </div>
          </Glass>
        </section>

        <div className="row" style={{ gap: 10, flexWrap: 'wrap' }}>
          <Badge tone="blue">限制 · {(state.caps?.limits?.max_px as number ?? 0).toLocaleString()} px</Badge>
          <Badge>{fmtBytes(Number(state.caps?.limits?.max_in_bytes ?? 0))} 单文件</Badge>
          <Badge>任务超时 {String(state.caps?.limits?.job_timeout_s ?? 600)}s</Badge>
          <Badge>{(state.caps?.codes ?? []).length} 类错误码</Badge>
        </div>
      </div>
    </div>
  );
}
