# AgentCanvas 智能体接入说明

给 AI 智能体（Claude / Codex / 任意 MCP 客户端）使用的**图片与设计引擎**。
人类在桌面窗口里看到的是同一套引擎、同一份文档、同一条调用日志。

## 一句话能力

一张海报、一套系列图、一批图片处理，一条命令出真文件：

```
agentcanvas poster title=新品上市 image=product.png preset=poster f=png out=campaign
```

## 三种调用方式

| 方式 | 用途 | 入口 |
| --- | --- | --- |
| CLI | 单次调用，一行 JSON | `agentcanvas <method> k=v` |
| MCP | 智能体工具挂载 | `agentcanvas mcp` |
| HTTP | 常驻服务、素材与字体流 | `agentcanvas http --port 8899` |

桌面 App 启动时会自动在 `127.0.0.1:8899`（占用则顺延到 8908）开启同一套 HTTP 接口，
所以 App 开着的时候，智能体不需要再单独起服务，也**不会**产生第二份工程状态。

## MCP 配置

```json
{
  "mcpServers": {
    "agentcanvas": {
      "command": "C:\\...\\agentcanvas\\dist\\bin\\agentcanvas.exe",
      "args": ["mcp"]
    }
  }
}
```

`agentcanvas caps` 的 `paths.exe` 字段会返回当前机器上的真实路径，可直接复制。

## 先读这一个，而不是 40 个 schema

```
agentcanvas caps
```

返回：方法名 + 一行描述、全部 image op、内置模板、尺寸预设、语义 intent 关键字、
短键含义、错误码表、限额。约 5 KB；完整 `tools/list` 反而更大，所以工具被刻意聚合成 40 个粗粒度条目。

## 省 token 的设计

1. **短键**：`{p,b,id,rev,t,s,l,o,sc,q,f,in}` 而不是 `{projectId,artboardId,nodeId,expectedRevision,...}`。
   一次 `canvas.add` 的完整成功响应约 90 字节。
2. **一次调用做完一件事**：`poster` 内部自己建工程、导入素材、排版、求解、检查、自动修复、预览、导出；
   智能体不需要 probe → create → add → solve → validate → export 六次往返。
3. **`batch`**：N 个调用一个来回。
   ```
   agentcanvas batch '[{"m":"caps"},{"m":"project.list"},{"m":"image.probe","path":"a.png"}]'
   ```
   失败项只返回 `{i,ok:false,e,code}`，不中断其余任务。
4. **`fields`**：`{"m":"project.get","fields":"rev,name"}` 只要两个键就返回两个键。
5. **`chain` 的字符串形式**：`"crop 1:1 > scale 0.5 > sharpen 1.2 > watermark 品牌"`，
   比等价的 JSON 数组短一半，且语义等价（两种写法都收）。
6. **`system.tools` 默认不带 Schema**：返回 `[{n,m,d}]`，实测 4 770 字节；`full=1` 才是
   16 569 字节的完整表。工具清单是智能体读的第一样东西，所以贵的那份必须是选项而不是默认值。
7. **整数就写成整数**：`rpc::call` 出口统一走 `util::whole_numbers`，`1440.0` → `1440`。
   一个文档返回里有几百个数字，`.0` 是每个数字白付的两个 token；小数（`205.2`）原样保留。
8. **可重复的路径**：`project.create` 和 `poster` 都收 `id`，同一个 id 落同一个工程目录，
   于是 `preview.png` 之类的路径在两次调用之间不变——智能体可以把输出路径当常量写进脚本。
   `examples/` 就是这样做到逐图 sha256 复现的。
   `docs/api-reference.md` 的真实返回示例由 `node tools/api-capture.mjs` 从 `dist/bin` 重跑生成。
6. **默认紧凑单行 JSON**，`--pretty` 才展开；stdout 只有结果，日志全在 stderr。
7. **错误自带 `hint`**：一次就能改对，不用试错第二轮。
   ```
   {"e":"no asset \"p1.png\"","code":"ASSET_NOT_FOUND","hint":"asset.import {path} first, then pass the returned id"}
   ```
8. **`@file` / `k=v`**：长参数（`cmds`、`nodes`、`jobs`）放文件里，命令行保持短。
9. **`--root DIR`**：智能体用短相对路径，返回值同样按 root 缩短。
10. **`rev` 乐观锁**：改之前带上你读到的 `rev`；过期时返回 `REVISION_CONFLICT` 和当前 `rev`，
    不会静默覆盖别人的修改，也不会需要你重新读整个文档。
11. **结构化检查报告**：`inspect.validate` 每条问题都带数值证据和一条可直接执行的 `fix` 命令，
    智能体不需要"看图猜问题"。

## 方法清单

| 方法 | 作用 | 关键参数 |
| --- | --- | --- |
| `caps` | 全部自描述 | — |
| `doctor` | 环境/字体/CJK/端口 | — |
| `help` | 12 行速查 | — |
| `tools` | MCP 工具 + JSON schema | `full` |
| `schema` | 文档骨架 + 错误码 + 命令形状 | — |
| `config` | 读设置；`set:{...}` 写设置（未知键直接报错） | `set` |
| `system.provider.probe` | 探测已配置的模型端点：`{status,ms,models[]}` | `url timeout` |
| `health` `version` `demo` | 端口/版本/示例工程 | — |
| `poster` | **一句话到成片** | `title image preset f out` |
| `batch` | 批量 | `jobs:[{m,p}]` |
| `project.*` | create open get list save close clone snapshot restore export_bundle import_bundle | `p` |
| `artboard.*` | create list resize duplicate delete set | `p b w h preset` |
| `asset.*` | import list inspect replace remove thumb missing | `p path id` |
| `canvas.*` | add add_many update set text image transform style reorder group ungroup duplicate delete align distribute batch | `p b t x y w h intent` |
| `layout.*` | apply set_mode resolve explain reflow lock | `p id intent mode` |
| `inspect.*` | validate document node geometry text conflicts assets diff fix | `p b dry` |
| `render.*` | preview svg export export_batch diff | `p b f sc q o` |
| `history.*` | undo redo list checkpoint compare rollback | `p n from to` |
| `template.*` | save list use vars variants builtin | `p tpl vars` |
| `image.*` | op chain batch preset presets ops probe palette hash compare | `in out op chain q` |
| `text.*` | measure fonts register | `text family size` |
| `jobs` `cancel` `log` `clear` | 进度与调用记录 | `id n` |

## 语义布局（这个产品的核心，别再用绝对坐标）

`intent` 里可以写：`anchor` `margin` `below` `above` `left_of` `right_of` `center_in`
`align_x` `align_y` `align_to` `size{w,h,w_ref,ratio,mode}` `fit` `safe_margin`
`distribute` `equal_gap` `keep_aspect_ratio` `flex{}` `grid{}` `priority`。

关系目标（`below` / `center_in` / `align_to` …）写**节点 id**，也可以直接写节点的
`name`——只要在文档里唯一。指向不存在或重名歧义目标的约束不会被悄悄忽略：
`canvas.add` 的 `warnings` 和 `layout.resolve` 的 `conflicts` 会把它点名。

实测踩过的三条，写下来省你一轮试错：

1. `intent.fit:"cover"` 是"把可用空间吃满"的**尺寸语义**，不是裁剪方式。它和 `size.ratio`
   同时用会把盒子撑到画板之外（1080×1440 的版面能算出 2304×1440），后面的 `below` 链
   就整排掉出画布。要一条固定高度的图带，写 `size:{w_ref:0.86,h:580}`，
   裁剪方式放在节点自己的 `image.fit` 上。
2. `align_x:"left"` 对齐的是**画板左边缘**，不是 safe margin。要保持页边距就用
   `align_to:{of:"<已定位节点>",edge:"left"}`，或干脆 `anchor:"top-left"` + `margin`。
3. `poster` 只认该版面声明过的变量（`poster_kv` 是 title / subtitle / image / accent / logo）。
   你没见过的键会被静默忽略——想塞正文就换版面（`tpl=long_image`）或者建完工程再
   `canvas.add`；`inspect.validate` 的 `errors:0` 不代表你的文案都上去了。

```
agentcanvas canvas.add p=proj_x b=b1 t=text text=标题 size=88 intent={"anchor":"top","margin":110}
agentcanvas canvas.add p=proj_x b=b1 t=image asset=as1 intent={"below":"n_headline","align_x":"center","w":710,"keep_aspect_ratio":true}
agentcanvas layout.resolve p=proj_x          # 求解，返回 changed 和 conflicts
agentcanvas layout.explain p=proj_x id=n_img # 它为什么在这儿
```

改尺寸用 `layout.reflow`（按约束重排）而不是把每个节点重新算一遍坐标——
系列海报保持统一版式靠的就是这个。手工拖过的节点会被标成 `manual`，之后的自动排版不会覆盖它。

## 数值约定

- 坐标一律 **CSS 像素**，原点画板左上角，y 向下；旋转 **度**、顺时针、绕 `origin`。
- 颜色 `#RRGGBB` / `#RRGGBBAA` / `rgb()` / `rgba()`，或 `$token` 引用画板调色板。
- 透明度 `0..1`；`quality` `1..100`；`sc`（scale）是倍率，不是像素。
- 时长/大小这类没有单位歧义的东西不存在——这个产品只有像素、度、百分比。
- 导出返回 `{path,w,h,bytes,sha,mime}`：**sha 是真实文件的 sha256**，用来证明文件确实写出来了。

## 建议执行回路

```
caps → poster|project.create → asset.import → canvas.add(+intent)
     → layout.resolve → inspect.validate → inspect.fix(dry=1)
     → render.preview → image.compare(前后) → render.export
```

`inspect.validate` 的 `ok:true` 只表示没有 error 级问题；warn 值得看一眼，info 是排版提示。

## 已知边界

- **没有接大模型密钥时**：所有确定性能力（排版、图形、图片处理、导出、检查）照常可用，
  `poster` 走内置模板而不是"理解需求"。需要模型的能力（AI 抠图、以图生图、审美复核）
  返回 `PROVIDER_NOT_CONFIGURED` 并给出确定性的替代方案，不会假装成功。
- `image.op remove_bg` 默认是**边框色/色度键**的确定性算法，复杂发丝边缘效果有限；
  `mask:1` 可以拿到 alpha 蒙版自己再处理。
- 字体用的是**本机已安装字体**，交付包里不含任何不可分发字体。缺中文字体时 `doctor` 会报
  `cjk_font:false`，中文会回退，`inspect.validate` 会出 `missing-font` / `tofu-risk`。
- 单张图上限 60 MP / 200 MB；异步任务默认 600 秒超时（`config` 可调）。
- 预览与导出共用同一条 SVG 编译链路，但浏览器排版与 resvg 排版在极端字距下可能有
  亚像素差异——以 `render.export` 出来的文件为准。
- 渲染器被迫降级时会在返回里带 `warns`（素材读不到、字体回退、背景模糊替代）。
  **`warns` 为空不代表图片画上了**：判断一张海报有没有吃到素材，看像素或比字节数，
  别看"文件存在"。`render.preview` 与 `render.export` 都把素材内联后再光栅化，
  所以落盘的 PNG 就是版面本来的样子。
- HTTP 只监听 `127.0.0.1`。要鉴权就在 `config` 里设 `token`，之后所有路由（除 `/health`）
  都要 `Authorization: Bearer`。
