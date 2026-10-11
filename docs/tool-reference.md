# 工具参考（自动生成）

> 由 `node tools/gen-docs.mjs` 从当前构建的引擎生成，不要手改。
> 引擎 1.0.0 · schema 1.1.0 · 38 个 MCP 工具 · 97 个方法

MCP 客户端看到的是下面这些**聚合后的工具**；每个工具都转发到同一个 `rpc::call`，
所以工具清单不需要为了覆盖率而膨胀。任何未列出的方法都可以通过 `agentcanvas`
这个通用工具访问：`{"name":"agentcanvas","arguments":{"m":"layout.explain","p":{…}}}`。

| 工具 | 必填参数 | 说明 |
| --- | --- | --- |
| `agentcanvas_caps` | — | One call that describes the whole product: methods, ops, intents, limits, error codes. Call this before anything else. |
| `project_create` | name | Create a design project. Returns {p} which every other call needs as "p". |
| `project_open` | p | Open an existing project by id or path. |
| `project_get` | p | Read the whole document (scene graph) or a summary with depth:0. |
| `project_save` | p | Save to disk atomically. Returns path, rev and sha256. |
| `project_list` | — | All projects, newest first. |
| `artboard_create` | p | Add a board. w/h or preset or ratio (1:1, 16:9). |
| `artboard_duplicate` | p, b | Duplicate a board into other ratios, reflowing layout intents. |
| `asset_import` | p | Import image files into the project (deduped by sha256). Returns ids. |
| `canvas_add` | p, t | Add a layer: text\|image\|rect\|ellipse\|line\|polygon\|path\|svg\|group\|frame. Give it semantic layout via intent. |
| `canvas_update` | p, id | Deep-merge a patch into a node: style, layout, box, text. null removes a key. |
| `canvas_batch` | p, cmds | Many commands in one transaction: all succeed or none do. One revision bump. |
| `canvas_delete` | p | Delete layers (locked ones need force). |
| `canvas_align` | p, ids | Align or distribute layers: axis x\|y, how left\|center\|right\|top\|middle\|bottom. |
| `layout_apply` | p, id, intent | Attach semantic constraints (below/above/center_in/anchor/margin/size/flex/grid) and solve. |
| `layout_resolve` | p | Solve the whole board or one artboard; returns changed ids and conflicts. |
| `layout_explain` | p, id | Why is this node where it is? Ordered intent provenance, plus conflicts. |
| `inspect_validate` | p | Machine-readable quality report: offboard, text-overflow, contrast, low-res, missing asset/font, conflicts. Every issue carries numeric evidence and a fix command. |
| `inspect_fix` | p | Apply the suggested fixes (or dry:1 to see them). |
| `inspect_diff` | p | Structural diff between two snapshots or two projects. |
| `render_preview` | p | Rasterise the current document to a real PNG and return its path + url. Cheap, call it after edits. |
| `render_export` | p | Export PNG / JPG / WebP / SVG / PDF. Returns files with size, dims and sha256. |
| `render_svg` | p | The compiled SVG — the exact string the rasteriser consumes. |
| `history_undo` | p | Undo (n steps). Human and agent edits share one history. |
| `history_redo` | p | Redo (n steps). |
| `history_list` | p | The command trail with origin (desktop\|mcp\|http\|cli). |
| `template_use` | p, tpl | Instantiate a built-in or saved template into a project. |
| `template_variants` | p | Build a series: N boards sharing one layout with different colours/copy/assets. |
| `poster` | title | One call, finished poster: creates the project, imports images, lays out, solves, validates, auto-fixes, previews and exports. Best first choice for a whole deliverable. |
| `image_op` | in, op | Single raster operation on a file: crop, resize, scale, pad, rotate, flip, round, border, adjust, blur, sharpen, noise, gray, threshold, tint, overlay, watermark, background, remove_bg, trim, pixelate, vignette, optimize. |
| `image_chain` | in, chain | Run a chain of raster ops. chain accepts the compact string form "crop 1:1 > scale 0.5 > sharpen 1.2 > watermark 品牌". |
| `image_batch` | paths | Same chain over many files. Returns per-file ok/out/e without aborting the batch. |
| `image_probe` | path | Real dimensions, format, alpha, bytes, sha256, palette of a file. |
| `image_compare` | a, b2 | Similarity between two images: score 0-100, per-channel error, changed-pixel count, bounding box of the difference. |
| `text_measure` | text | Measure text with the same font engine the renderer uses: width, height, wrapped lines, fallback warnings. |
| `system_doctor` | — | Environment check: writable root, font count, CJK availability, port. |
| `system_tools` | full | Tool list. Default is `[{n,m,d}]` (4.8 KB); `full:1` adds the JSON Schemas (16.6 KB). |
| `batch` | jobs | Several method calls in one round trip. jobs:[{m,p},...] |

## 全部方法

```
caps  everything an agent needs in one call
help  10-line cheat sheet
doctor  environment + limits check
tools  MCP tool list with schemas
schema  document skeleton + error codes + cmd shapes
config  read settings, or set:{...} to write
demo  build a real poster end to end
batch  jobs:[{m,p}] in one round trip
log  recent call log
jobs  async job table
cancel  stop a job by id
project.create  name w h [preset] [boards]
project.open  p
project.get  p [depth:0]
project.list  
project.save  p
project.close  p
project.clone  p name
project.snapshot  p [label]
project.restore  p snap
project.export_bundle  p o
project.import_bundle  path
artboard.create  p w h [preset] [background]
artboard.list  p
artboard.resize  p b w h
artboard.duplicate  p b to:[ratios]
artboard.delete  p b
artboard.set  p b s:{}
asset.import  p path|paths[]
asset.list  p [q]
asset.inspect  p id
asset.replace  p id path
asset.remove  p id
asset.thumb  p id [w]
asset.missing  p
canvas.add  p b t x y w h [text|asset|style|intent]
canvas.add_many  p b nodes:[]
canvas.update  p id s:{}|l:{}|box:{}
canvas.set  p ids[] patch:{}
canvas.text  p id content
canvas.image  p id asset [fit]
canvas.transform  p id x y w h [rot]
canvas.style  p id fill|stroke|radius|opacity|fx
canvas.reorder  p id to [parent]
canvas.group  p ids[]
canvas.ungroup  p id
canvas.duplicate  p ids[]
canvas.delete  p ids[]|id
canvas.align  p ids[] axis how
canvas.distribute  p ids[] axis
canvas.batch  p cmds:[{t,...}] one transaction
layout.apply  p id intent:{} [mode]
layout.set_mode  p id mode
layout.resolve  p [b]
layout.explain  p id why is it there
layout.reflow  p b w h mode
layout.lock  p id pinned
inspect.validate  p [b] -> issues with evidence + fix
inspect.document  p tree summary
inspect.node  p id
inspect.geometry  p ids[]
inspect.text  p id metrics
inspect.conflicts  p b
inspect.assets  p usage
inspect.diff  p with:<project>|a snap b2 snap
inspect.fix  p [dry] apply suggested cmds
render.preview  p [b] -> real png + url
render.svg  p [b] [preview:1] [o]
render.export  p [b] f sc q o [transparent]
render.export_batch  p boards[] specs:[{f,sc,w,h,suffix}]
render.diff  a b -> similarity + changed box
history.undo  p [n]
history.redo  p [n]
history.list  p [n]
history.checkpoint  p [label]
history.compare  p from to
history.rollback  p rev|snap
template.save  p b name
template.list  
template.use  p tpl [vars]
template.vars  p vars:{title:..,product:..}
template.variants  p from colors[] copy:{} -> {boards,ids}
template.builtin  list built-in layouts
poster  one call: assets+copy+size -> project+preview+export
image.probe  path
image.op  in out op args:{}
image.chain  in out chain:[{op,..}] or "crop 400x400 > scale .5"
image.batch  paths[] chain|preset out [fmt q]
image.preset  name in out
image.presets  
image.ops  list every op
image.palette  path [n]
image.hash  paths[] dhash+ahash
image.compare  a b
text.measure  text family size -> w h lines
text.fonts  [q]
text.register  path
```

## 图片处理 op

`image.op {op}` / `image.chain` 接受的名字：

```
crop resize scale pad extend blurpad rotate flip round border adjust gray invert sepia threshold dither tint blur sharpen noise normalize autocontrast equalize vignette pixelate emboss edge shadow overlay watermark grid contact_sheet stack collage background remove_bg strip optimize palette dhash ahash trim border-trim
```

预设：`social_1080`, `ecommerce_main`, `wechat_cover`, `avatar`, `story_9_16`, `strip_4x3`, `thumbnail_256`, `print_a4`, `web_hero`, `compress_200k`

## 语义布局 intent

```
anchor margin margin_top margin_right margin_bottom margin_left above below left_of right_of center_in align_x align_y align_to size fit safe_margin distribute equal_gap keep_aspect_ratio flex grid priority
```

## 尺寸预设

| 名称 | 像素 | 说明 |
| --- | --- | --- |
| `poster` | 1080×1440 | 海报 3:4 |
| `square` | 1080×1080 | 方图 1:1 |
| `story` | 1080×1920 | 故事 9:16 |
| `4:5` | 1080×1350 | 竖图 4:5 |
| `banner` | 1920×1080 | 横幅 16:9 |
| `xhs` | 1242×1660 | 小红书 3:4 |
| `ecom-main` | 800×800 | 电商主图 |
| `long` | 750×3000 | 图文长图 |
| `a4` | 2480×3508 | A4 竖版 |
| `wechat` | 900×500 | 公众号头图 |
| `card` | 1600×500 | 卡片横幅 |
| `business-card` | 1050×600 | 名片 |

## 内置模板

```
poster_kv ecom_main story_ad card_quote banner_promo cover_editorial grid_collage long_image
```

## 短键

| 键 | 含义 |
| --- | --- |
| `b` | artboard |
| `chain` | image op list |
| `f` | format |
| `fields` | trim result |
| `id` | node |
| `in` | input path |
| `in[]` | list |
| `intent` | semantic layout intent |
| `l` | layout |
| `o` | out path |
| `p` | project |
| `pretty` | indent |
| `q` | quality |
| `rev` | expectedRevision |
| `s` | style |
| `sc` | scale |
| `t` | node type |

## 限额

```json
{
  "job_timeout_s": 600,
  "max_in_bytes": 200000000,
  "max_px": 60000000
}
```
