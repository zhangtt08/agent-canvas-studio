# AgentCanvas RPC method surface

One flat namespace of `domain.action` methods. `rpc::call(method, params, ctx)` is
the only entry point; CLI, HTTP (`POST /rpc`), MCP tools and the desktop webview all
reach it. MCP exposes the same surface aggregated to ~28 tools.

## Token rules (why the shapes look like this)

- **Short keys** in params and results: `p`=project, `b`=board/artboard, `id`=node,
  `rev`=revision, `t`=type, `s`=style, `l`=layout, `w/h/x/y`, `q`=quality, `sc`=scale,
  `f`=format, `o`=out path, `n`=count, `v`=value.
  The whole document (`project.get depth:1`) uses the same spellings, so there is no
  second vocabulary to learn: `revision`/`project_id` only ever appear inside a
  stored `project.json`, never in a reply.
- **Compact single-line JSON** on stdout/HTTP; `pretty:1` opts into indentation.
- **One call does one job**: `poster` builds a whole board from a spec; `image.chain`
  runs a filter list; `canvas.batch` applies N commands in one transaction.
- **`fields`** trims a result to the keys asked for: `{"m":"project.get","fields":"rev,boards"}`.
- **Errors carry `hint`** so the next call is usually right first time.
- **`caps` is the one document an agent needs** — methods + short keys + limits, ~4 KB.

Mutating methods accept `{"p":"proj_x","rev":7}`. A stale `rev` returns
`REVISION_CONFLICT` with `{rev:<current>}` in `details`; the caller re-reads and retries.
Results of mutations are `{"ok":true,"rev":8,"changed":["node_id",...],"warnings":[]}`.

## Methods

| method | params (short keys) | result |
|---|---|---|
| `system.caps` | — | methods, ops, presets, short keys, limits, paths, port |
| `system.health` | — | `{ok,ver,port,pid,jobs,fonts,root}` — `root` is the resolved absolute path |
| `system.version` | — | `{ver,schema,build}` |
| `system.doctor` | — | env checks + what is missing |
| `system.tools` | `full:0/1` | default `[{n,m,d}]` 4.8 KB; `full:1` adds JSON Schema, 16.6 KB |
| `system.config` | `set:{...}` | settings; unknown keys are refused, secrets come back as `…Set` presence only |
| `system.provider.probe` | `url timeout` | `{ok,url,status,ms,auth,n_models,models[]}` — reachability of the configured endpoint |
| `system.demo` | `o:dir` | builds a real poster end-to-end, returns files |
| `project.create` | `name,w,h,preset,boards:[{w,h,name}],dir` | `{p,rev,boards,dir}` |
| `project.open` | `p` (id or path) | `{p,rev,name,boards,assets}` |
| `project.get` | `p,depth:0..2` | document (or summary) |
| `project.list` | — | `[{p,name,boards,updated}]` |
| `project.save` | `p` | `{path,rev,sha}` |
| `project.close` | `p` | ok |
| `project.clone` | `p,name` | `{p}` |
| `project.snapshot` | `p,label` | `{snap}` |
| `project.restore` | `p,snap` | `{rev}` |
| `project.export_bundle` | `p,o` | `{path,bytes,files}` |
| `project.import_bundle` | `path,dir` | `{p}` |
| `project.recent` | `n` | list |
| `artboard.create` | `p,w,h,name,preset,at` | `{b}` |
| `artboard.list` | `p` | `[{b,name,w,h,layers}]` |
| `artboard.resize` | `p,b,w,h,reflow:1` | `{changed}` |
| `artboard.duplicate` | `p,b,to:[ratios]` | `{boards}` |
| `artboard.delete` | `p,b` | ok |
| `artboard.set` | `p,b,s:{...}` | `{changed}` |
| `asset.import` | `p,path\|paths:[],mode:copy,dedupe:1` | `[{id,name,w,h,bytes,sha,dup}]` |
| `asset.list` | `p,q` | `[{id,name,w,h,kind}]` |
| `asset.inspect` | `p,id` | full record + usage |
| `asset.replace` | `p,id,path` | `{id,sha}` |
| `asset.remove` | `p,id,force` | ok |
| `asset.thumb` | `p,id,w` | `{path}` |
| `asset.missing` | `p` | `[{id,uri}]` |
| `canvas.add` | `p,b,t,x,y,w,h,s,l,text,asset,at,name` | `{id,rev}` |
| `canvas.add_many` | `p,b,nodes:[...]` | `{ids,rev}` |
| `canvas.update` | `p,id,s:..,l:..,box:{x,y,w,h},...` (deep merge) | `{changed}` |
| `canvas.set` | `p,ids:[],patch:{...}` | `{changed}` |
| `canvas.text` | `p,id,content` \| `p,b,id,content` | `{changed}` |
| `canvas.image` | `p,id,asset,fit` | `{changed}` |
| `canvas.transform` | `p,id,x,y,w,h,rot,flip_x,flip_y` | `{changed}` |
| `canvas.style` | `p,id,fill,stroke,radius,opacity,blend,fx` | `{changed}` |
| `canvas.reorder` | `p,id,to,board,parent` | `{z}` |
| `canvas.group` | `p,ids` / `p,id` (ungroup) | `{id}` |
| `canvas.duplicate` | `p,ids,at,dx,dy` | `{ids}` |
| `canvas.delete` | `p,ids,force` | `{deleted}` |
| `canvas.align` | `p,ids,axis:x\|y,how:left\|center\|right\|top\|middle\|bottom` | `{changed}` |
| `canvas.distribute` | `p,ids,axis` | `{changed}` |
| `canvas.batch` | `p,cmds:[{t,...}]` | `{rev,changed}` (one transaction) |
| `layout.apply` | `p,id,intent:{...},mode:constraint\|manual\|flex\|grid` | `{box,prov}` |
| `layout.resolve` | `p,b` (solve) | `{changed,conflicts}` — a solve that moves nothing leaves `rev` alone; `conflicts` names cycles **and** relations whose target does not exist |
| `layout.explain` | `p,id` | intent order + which won |
| `layout.reflow` | `p,b,w,h,mode:scale\|reflow\|adaptive` | `{changed}` |
| `layout.lock` | `p,id,pinned:0/1` | ok |
| `inspect.validate` | `p,b?,opts:{contrast:1,lowres:1}` | `{issues:[{sev,code,node,msg,e,fix}],ok}` |
| `inspect.document` | `p` | summary tree |
| `inspect.node` | `p,id` | resolved geometry + style |
| `inspect.geometry` | `p,ids` | boxes |
| `inspect.text` | `p,id` | metrics, lines, font, overflow |
| `inspect.conflicts` | `p,b` | constraint cycles |
| `inspect.diff` | `p,a:snap1,b2:snap2` \| `paths:[i1,i2]` | structural or pixel diff |
| `inspect.assets` | `p` | usage map |
| `inspect.fix` | `p,issues:[..]\|auto:1,dry:0` | `{cmds,changed}` |
| `render.preview` | `p,b?,sc,w?,fmt:png,o?` | `{path,url,w,h,bytes,sha}` — rasterised from inlined assets, so the PNG is what the board actually looks like |
| `render.svg` | `p,b?,embed:0\|1` | SVG string (or `{path}`) |
| `render.export` | `p,b?:all,f:png\|jpg\|webp\|svg\|pdf,sc,q,o,transparent,bg` | `[{path,f,w,h,bytes,sha,mime}]` |
| `render.export_batch` | `p,boards:[],specs:[{f,sc,w,h,suffix}]` | `{outputs,jobs}` |
| `render.jobs` | — | `[{id,kind,label,done,stage,ok}]` |
| `render.cancel` | `id` | ok |
| `render.outputs` | `p?,n` | recent artifacts |
| `history.undo` / `history.redo` | `p,n` | `{rev,did}` |
| `history.list` | `p,n` | trail with origin + changed |
| `history.compare` | `p,from,to` | diff |
| `history.rollback` | `p,rev\|snap` | `{rev}` |
| `template.save` | `p,b,name` | `{tpl}` |
| `template.list` | — | built-ins + user templates |
| `template.use` | `p,tpl,b?,at?` | `{ids}` |
| `template.vars` | `p,vars:{...}` | `{changed}` |
| `template.variants` | `p,from:n,colors:[],copy:[],boards:[]` | `{ids,boards}` |
| `image.probe` | `path` | `{w,h,fmt,alpha,bytes,sha,orientation}` |
| `image.op` | `in,out,op,args:{...},q,fmt` | `{out,w,h,bytes,sha}` |
| `image.chain` | `in,out,chain:[{op,...}]\|"crop 400x400 > scale 0.5 > sharpen"`,preset | `{out,...}` |
| `image.batch` | `paths:[]\|glob,in,out,chain,pattern` | `[{ok,out,e?}]` |
| `image.preset` | `name,in,out` | `{out}` (`social_1080 ecommerce_main wechat_cover avatar strip4x3 ...`) |
| `image.palette` | `path,n` | `[{hex,count}]` |
| `image.hash` | `paths:[]` | `{dhash,ahash}` for near-duplicate checks |
| `image.compare` | `a,b` | `{score,mae,w,h,box}` |
| `image.thumb` | `in,out,w,q` | `{out,bytes}` |
| `text.measure` | `text,family,size,weight,ls,max_w,lh` | `{w,h,lines,font,fell_back}` |
| `text.fonts` | `q?` | families + faces |
| `text.register` | `path` | `{family}` |
| `batch` | `jobs:[{m,p}]` (also `ops`,`n`) | `[{i,ok,...}]` — never aborts the rest |
| `log` | `n` | call log |
| `clear` | — | clears log/jobs |

`poster` is a convenience: `{"m":"poster","spec":{...}}` creates a project, imports
assets, adds nodes with intents, solves, validates and exports — one round trip.
