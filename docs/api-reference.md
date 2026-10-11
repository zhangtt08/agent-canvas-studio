# HTTP API

Base：`http://127.0.0.1:8899`（桌面 App 启动即开启；独立运行用 `agentcanvas http --port 8899`）。
只监听回环地址。设置 token 后（`{"m":"config","p":{"set":{"token":"..."}}}` 或环境变量
`AGENTCANVAS_TOKEN`），除 `/health` 外所有路由要求 `Authorization: Bearer <token>`。

## 一个入口就是全部

```http
POST /rpc
Content-Type: application/json

{"m":"canvas.add","p":{"p":"proj_x","b":"b1","t":"text","text":"标题","intent":{"anchor":"top","margin":110}}}
```

`m` 是 `docs/METHODS.md` 里的任意方法，`p` 是它的参数。返回是紧凑单行 JSON。

也支持查询式（方便 curl 和调试）：

```
GET /rpc?m=project.get&p={"p":"proj_x","fields":"rev,name"}
GET /rpc?m=image.probe&path=a.png
```

`fields` 参数把返回裁到你要的键——省 token 也省眼睛。

## REST 形状（同一套业务，只是别名）

| 路由 | 等价方法 |
| --- | --- |
| `POST /v1/projects` | `project.create` |
| `GET /v1/projects` | `project.list` |
| `GET /v1/projects/:id` | `project.get` |
| `POST /v1/projects/:id/commands:batch` | `canvas.batch` |
| `POST /v1/projects/:id/render` | `render.export` |
| `POST /v1/projects/:id/validate` | `inspect.validate` |
| `DELETE /v1/projects/:id` | `project.close` |

```http
POST /v1/projects/proj_x/commands:batch
{"expectedRevision":3,"commands":[
  {"t":"Text","id":"title","content":"新品上市"},
  {"t":"LayoutIntent","id":"product","mode":"constraint","intent":{"below":"title","gap":90,"align_x":"center"}}
]}
```

```json
{"ok":true,"rev":4,"changed":["title","product"],"created":[],"warnings":[]}
```

## 自描述

| 路由 | 内容 |
| --- | --- |
| `GET /health` | `{ok,ver,port}`，无需鉴权 |
| `GET /caps` | 全量自描述（约 5 KB） |
| `GET /tools` | MCP 工具清单 + JSON Schema |

## 素材与字体流

这三个路由是给 webview 用的，也是给智能体核对文件用的：

```
GET /asset?id=<asset_id>        → 导入的原始素材字节（image/png …）
GET /font?family=Arial&weight=700 → 引擎实际用来排版的那份字体文件
GET /file?path=preview.png      → 工程目录内的任意产物
```

`/font` 是"预览与导出一致"的关键：浏览器拿到的字体文件和 resvg 用的是同一份，
所以换行位置不会只在其中一边成立。

## 错误格式

```json
{"e":"no asset \"p1.png\"","code":"ASSET_NOT_FOUND","hint":"asset.import {path} first, then pass the returned id"}
```

| code | 含义 | 通常怎么办 |
| --- | --- | --- |
| `INVALID_SCHEMA` | 参数缺失/形状不对 | 看 `details`，或先 `caps` |
| `INVALID_VALUE` | 值越界（尺寸、颜色、角度） | 看 `details.value` |
| `REVISION_CONFLICT` | 你读的 `rev` 过期 | 用 `details.rev` 重读后重试（`retryable:true`） |
| `LOCKED` | 图层被锁 | 人明确锁的；确需覆盖传 `force:true` |
| `LAYOUT_CONFLICT` | 约束互相矛盾（含环） | 看 `details.cycle`，删掉一条约束 |
| `ASSET_MISSING` | 素材文件不在盘上 | 重新 `asset.import` 或 `asset.replace` |
| `FONT_MISSING` | 字体没安装 | `text.fonts` 看可用字体，或 `text.register` |
| `TOO_LARGE` | 超尺寸/体积上限 | 先 `image.op resize` 缩小 |
| `PATH_DENIED` | 越出沙箱根目录 | 相对路径，或 `--root DIR` |
| `PROVIDER_NOT_CONFIGURED` | 需要模型的能力 | 用返回 `hint` 里的确定性替代 |
| `JOB_TIMEOUT` / `CANCELLED` | 任务超时/被取消 | 提高 `config.job_timeout_s` 或减小批量 |

判定成功只认结构：`ok===true` 或没有 `code`。**不要**用返回文本里有没有"完成"来判断。

## curl 速查

```bash
B=http://127.0.0.1:8899
curl -s $B/health
curl -s -XPOST $B/rpc -d '{"m":"caps"}' | head -c 400
curl -s -XPOST $B/rpc -d '{"m":"poster","p":{"title":"新品上市","image":"C:\\pics\\a.png","preset":"poster"}}'
curl -s -XPOST $B/rpc -d '{"m":"project.get","p":{"p":"proj_x","fields":"rev,name"}}'
```

## 真实返回示例

<!-- api:begin -->
以下每一段都是 `agentcanvas` 1.0.0（schema 1.1.0）在 Windows 上的真实 stdout，
由 `node tools/api-capture.mjs` 生成，不是手写示例。重跑即可刷新。

**一次调用出图** — 返回 1448 字节

```powershell
agentcanvas poster --root <root> "id=doc-poster" "name=无界面出片" "title=夏日轻乳" "subtitle=生牛乳发酵" "body=冷链直送，只有三种配料。" "image=<img>" "accent=#4B6BFB" "preset=poster" "f=png" "out=<root>/out"
```

```json
{"boards":["b_5zw7rzh"],"dir":"projects/doc-poster","files":[{"bytes":53543,"f":"png","h":1440,"mime":"image/png","path":"out/无界面出片.png","sha":"8c9f1937a1e56a2a8de97b00768d508b73c56e06a3066331c2e9f49b2fea6ce9","w":1080},{"bytes":9832,"f":"png","h":432,"mime":"image/png","path":"out/无界面出片-preview.png","sha":"108aadf9bf52f062da582d72b371c8e151b21287fbe864d25b4f7bfa34bd1bf1","w":324}],"ids":["t_5zwlawd-0","t_5zwlawd-1","t_5zwlawd-2","t_5zwlawd-3","t_5zwlawd-4"],"issues":[{"b":"b_5zw7rzh","c":"text-overflow","e":{"font_size":38.57,"have":52.07,"lines":1,"need":54,"truncated":false},"f":{"id":"t_5zwlawd-2","patch":{"text":{"overflow":"shrink","size":37.19}},"t":"NodeSet"},"m":"\"t_5zwlawd-2\" needs 54px but has 52.07px (1 lines)","n":"t_5zwlawd-2","s":"warn"},{"b":"b_5zw7rzh","c":"tiny-target","e":{"h":10,"min":10,"need":48,"w":72},"m":"\"t_5zwlawd-3\" is a tap/brand target under 48px","n":"t_5zwlawd-3","s":"warn"},{"b":"b_5zw7rzh","c":"text-overflow","e":{"font_size":32.79,"have":44.27,"lines":1,"need":45.91,"truncated":false},"f":{"id":"t_5zwlawd-4","patch":{"text":{"overflow":"shrink","size":31.62}},"t":"NodeSet"},"m":"\"t_5zwlawd-4\" needs 45.91px but has 44.27px (1 lines)","n":"t_5zwlawd-4","s":"warn"},{"b":"b_5zw7rzh","c":"tiny-target","e":{"h":44.27,"min":44.27,"need":48,"w":420},"m":"\"t_5zwlawd-4\" is a tap/brand target under 48px","n":"t_5zwlawd-4","s":"warn"}],"p":"doc-poster","report":{"errors":0,"ok":true,"warns":4}}
```

**工程摘要 + fields 裁剪** — 返回 109 字节

```powershell
agentcanvas project.get --root <root> "p=doc-poster" "depth=0" "fields=p,name,rev,boards"
```

```json
{"boards":[{"b":"b_5zw7rzh","h":1440,"n":5,"name":"无界面出片","w":1080}],"name":"无界面出片","p":"doc-poster","rev":1}
```

**排版后的第一个节点** — 返回 2881 字节

```powershell
agentcanvas project.get --root <root> "p=doc-poster" "depth=1" "fields=p,nodes"
```

```json
{"nodes":[{"artboard":"b_5zw7rzh","box":{"h":576,"w":669.6,"x":205.2,"y":273.6},"flip_x":false,"flip_y":false,"id":"t_5zwlawd-0","image":{"asset":"as_5zxt82l","fit":"contain"},"layout":{"intent":{"anchor":"center","below":{"cross":"center","gap":24,"of":"t_5zwlawd-1"},"size":{"h_ref":0.4,"w_ref":0.62}},"mode":"constraint","provenance":["size.w_ref=0.62","size.h_ref=0.4","aspect=0.75","anchor=center","below:t_5zwlawd-1+24","safe_margin=72"]},"locked":false,"name":"hero","opacity":1,"origin":"center","rotation":0,"type":"image","visible":true,"z_role":"content"},{"artboard":"b_5zw7rzh","box":{"h":129.6,"w":928.8,"x":75.6,"y":120},"flip_x":false,"flip_y":false,"id":"t_5zwlawd-1","layout":{"intent":{"anchor":"top","margin_top":120,"size":{"min_h":120,"w_ref":0.86}},"mode":"constraint","provenance":["size.w_ref=0.86","hug.h=129.6","anchor=top","safe_margin=72"]},"locked":false,"name":"headline","opacity":1,"origin":"center","rotation":0,"text":{"align":"left","color":"rgb(255,255,255)","content":"夏日轻乳","line_ratio":1.2,"lines":["夏日轻乳"],"overflow":"shrink","size":96},"type":"text","visible":true,"z_role":"content"},{"artboard":"b_5zw7rzh","box":{"h":52.07,"w":864,"x":75.6,"y":265.6},"flip_x":false,"flip_y":false,"id":"t_5zwlawd-2","layout":{"intent":{"align_to":{"edge":"left","of":"t_5zwlawd-1"},"below":{"gap":16,"of":"t_5zwlawd-1"},"size":{"w_ref":0.8}},"mode":"constraint","provenance":["size.w_ref=0.8","hug.h=52.07","below:t_5zwlawd-1+16","align_to:t_5zwlawd-1:left","safe_margin=72"]},"locked":false,"name":"subtitle","opacity":1,"origin":"center","rotation":0,"text":{"align":"left","color":"rgb(199,205,219)","content":"生牛乳发酵","line_ratio":1.4,"lines":["生牛乳发酵"],"overflow":"shrink","size":38.57},"type":"text","visible":true,"z_role":"label"},{"artboard":"b_5zw7rzh","box":{"h":10,"w":72,"x":72,"y":1316.87},"flip_x":false,"flip_y":false,"id":"t_5zwlawd-3","layout":{"intent":{"left_of":{"cross":"middle","gap":16,"of":"t_5zwlawd-4"},"size":{"h":10,"w":72}},"mode":"constraint","provenance":["size.w=72","size.h=10","left_of:t_5zwlawd-4","safe_margin=72"]},"locked":false,"name":"brandbar","opacity":1,"origin":"center","rotation":0,"style":{"fill":{"color":"#4B6BFB","t":"solid"},"radius":{"all":6}},"type":"rect","visible":true,"z_role":"decor"},{"artboard":"b_5zw7rzh","box":{"h":44.27,"w":420,"x":86,"y":1299.73},"flip_x":false,"flip_y":false,"id":"t_5zwlawd-4","layout":{"intent":{"anchor":"bottom-left","margin_bottom":96,"margin_left":86,"size":{"min_h":44,"w":420}},"mode":"constraint","provenance":["size.w=420","hug.h=44.27","anchor=bottom-left","safe_margin=72"]},"locked":false,"name":"brand","opacity":1,"origin":"center","rotation":0,"text":{"align":"left","color":"rgb(255,255,255)","content":"yourbrand","line_ratio":1.4,"lines":["yourbrand"],"overflow":"shrink","size":32.79},"type":"text","visible":true,"z_role":"label"}],"p":"doc-poster"}
```

**体检** — 返回 1019 字节

```powershell
agentcanvas inspect.validate --root <root> "p=doc-poster"
```

```json
{"boards":[{"b":"b_5zw7rzh","ink":0.347,"nodes":5}],"errors":0,"infos":0,"issues":[{"b":"b_5zw7rzh","c":"text-overflow","e":{"font_size":38.57,"have":52.07,"lines":1,"need":54,"truncated":false},"f":{"id":"t_5zwlawd-2","patch":{"text":{"overflow":"shrink","size":37.19}},"t":"NodeSet"},"m":"\"t_5zwlawd-2\" needs 54px but has 52.07px (1 lines)","n":"t_5zwlawd-2","s":"warn"},{"b":"b_5zw7rzh","c":"tiny-target","e":{"h":10,"min":10,"need":48,"w":72},"m":"\"t_5zwlawd-3\" is a tap/brand target under 48px","n":"t_5zwlawd-3","s":"warn"},{"b":"b_5zw7rzh","c":"text-overflow","e":{"font_size":32.79,"have":44.27,"lines":1,"need":45.91,"truncated":false},"f":{"id":"t_5zwlawd-4","patch":{"text":{"overflow":"shrink","size":31.62}},"t":"NodeSet"},"m":"\"t_5zwlawd-4\" needs 45.91px but has 44.27px (1 lines)","n":"t_5zwlawd-4","s":"warn"},{"b":"b_5zw7rzh","c":"tiny-target","e":{"h":44.27,"min":44.27,"need":48,"w":420},"m":"\"t_5zwlawd-4\" is a tap/brand target under 48px","n":"t_5zwlawd-4","s":"warn"}],"ok":true,"warns":4}
```

**文字度量** — 返回 200 字节

```powershell
agentcanvas text.measure --root <root> "text=夏日轻乳 生牛乳发酵" "family=Inter" "size=72" "max_w=520"
```

```json
{"ascender":52.41796875,"descender":-15.15234375,"fell_back":true,"font":"Segoe UI","height":194.4,"line_height":97.2,"lines":["夏日轻乳 生牛","乳发酵"],"overflow":false,"truncated":false,"width":451.72265625}
```

**位图链（字符串形式）** — 返回 126 字节

```powershell
agentcanvas image.chain --root <root> "in=<img>" "out=<root>/out/chain.webp" "chain=crop 1:1 > scale 0.6 > sharpen 1 > watermark AgentCanvas" "q=82"
```

```json
{"bytes":263464,"out":"out/chain.webp","sha":"e13fcb2b43ca498ec2b8eb8e5ca5f5f731acf20186e30d87f3310cd1ee51e464","w":[432,432]}
```

**一个来回跑多个调用** — 返回 251 字节

```powershell
agentcanvas batch --root <root> "jobs=[{&quot;m&quot;:&quot;version&quot;,&quot;p&quot;:{}},{&quot;m&quot;:&quot;system.health&quot;,&quot;p&quot;:{}}]"
```

```json
{"ok":true,"r":[{"build":"1.0.0 windows","i":0,"ok":true,"schema":"1.1.0","ver":"1.0.0"},{"fonts":82,"i":1,"jobs":0,"ok":true,"pid":23472,"port":0,"root":"C:/Users/EDY/Documents/Qoder/2026-10-09/2bdc159f/.scratch/api","schema":"1.1.0","ver":"1.0.0"}]}
```

**错误：方法名写错** — 返回 204 字节

```powershell
agentcanvas nope.method --root <root>
```

```json
{"code":"INVALID_SCHEMA","details":{"near":["artboard.set","canvas.set","demo","history.redo","layout.reflow"]},"e":"unknown method \"nope.method\"","hint":"run caps, or help for the 10-line cheat sheet"}
```

**错误：写到 root 外面** — 返回 173 字节

```powershell
agentcanvas render.export --root <root> "p=doc-poster" "o=C:/Users/you/Desktop/x.png"
```

```json
{"code":"PATH_DENIED","e":"cannot write outside root C:\\Users\\EDY\\Documents\\Qoder\\2026-10-09\\2bdc159f\\.scratch\\api","hint":"use a relative path, or pass --root DIR"}
```

<!-- api:end -->
