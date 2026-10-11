# 外部参考与可采纳项

对象：AgentCanvas 引擎（MCP/HTTP/CLI 表面、resvg 渲染链、CJK 排版、imageops、UI 液态玻璃）。
基线事实来自仓库本身（`cli/src/mcp.rs`、`core/src/text.rs`、`core/src/imageops.rs`、`core/src/rpc.rs`、`ui/src/styles/glass.css`、`Cargo.lock`），
外部结论只引用本次实际抓取到的页面，全部标注日期；抓不到的标「未验证」。

## 1. 结论摘要（采纳 / 观察 / 拒绝）

| 决定 | 事项 | 一句话理由 | 工作量 |
|---|---|---|---|
| 采纳 | MCP 图像返回改 `resource_link`，base64 收进 `inline:1` | 现在每次预览在响应里塞 ≈2 MB base64（`cli/src/mcp.rs:212-228`），是最大的 token 出血点 | S |
| 采纳 | 大结果去掉 `text`+`structuredContent` 双份 JSON | 同一载荷被序列化两遍；规范只说 SHOULD 兼容、不是 MUST | S |
| 采纳 | 补全 tool annotations + 修 `template_use` 被标只读的 bug | 规范有 destructive/idempotent 提示（见 §2.2），`mcp.rs:169` 把会建节点的模板套用标成 read-only | S |
| 采纳 | `project.list`/`log`/`history.list` 加 cursor 分页 | MCP 列表方法按 cursor/nextCursor 翻页（§2.3），现在无上限整表返回 | S |
| 采纳 | `tools/call` 接 `progressToken` + `notifications/cancelled` | jobs 系统已有进度/取消原语，只差没接 MCP 进度通道（§2.4） | M |
| 采纳 | HTTP 错误映射 409/404 + POST 幂等键 | Stripe 式 `Idempotency-Key`；现在所有 RPC 错误都是 400，重试语义靠猜（§2.5） | S |
| 采纳 | 载入图片时 `apply_icc` 转 sRGB（moxcms 已在依赖树） | image 0.25 解码不自动应用嵌入 ICC，AdobeRGB/P3 素材进海报必偏色（§3.1） | S |
| 采纳 | 用已在树里的 ttf-parser/rustybuzz 做测量（含 kerning） | text.rs 自认「no shaping, no kerning」，而 resvg 出口用 rustybuzz 整形 → 行宽不一致 | M |
| 采纳 | kinsoku 缺口清单第一批（§3.3 的 1/2/3/4 条） | 』】〕缺失、`「`双列、数字连字符、货币粘合都是可单测复现的错 | S |
| 采纳 | `inspect.validate` 默认裁剪 evidence、加 `emoji-risk` | 报告体量大且 emoji 覆盖完全不检查（§2.6） | S |
| 采纳 | 液态玻璃：@supports 回退 + 修 `.bg-noise` 无纹理 + backdrop root 审计 | MDN 列出触发 backdrop root 的属性清单，动画中的 opacity 祖先会杀掉子级模糊 | S |
| 采纳 | `chain` 串吸收 ffmpeg 标签/分号语法 | 现在表达不了 overlay 类双输入链，一个来回变两个 | M |
| 采纳 | `layout` intent 直接收常用 CSS 名（justify-content 等） | satori 证明「CSS 子集即接口」可让模型零学习成本使用，省 schema token | M |
| 观察 | `unicode-linebreak` 作 UAX#14 表驱动基座 | 纯 Rust、`line_breaks()` 现成（cosmic-text 就这么用）；但要与现有 kinsoku 覆写合并 | M |
| 观察 | imageproc（dilate/erode/median/convolve） | 补蒙版清理算子，MIT、纯 Rust；API 与自有 Img 互转是成本（§4） | M |
| 观察 | oxipng（PNG 无损二次压缩） | `optimize_from_memory` 纯 Rust；收益百分比官方文档未给数字，标未验证 | S |
| 观察 | image-webp 0.2.4 的 `WebPEncoder` 是否含 lossy | README 只证「纯 Rust WebP 编解码」；能则修掉「webp 恒 lossless」的诚实注记 | S |
| 观察 | vello / vello_cpu | vello 0.11 需 GPU（wgpu），vello_cpu「仍在积极开发」；不进核心 | L |
| 观察 | swash | 整形/字体剖析全但重；只有绕开 SVG 直绘时才值得 | L |
| 拒绝 | skia-safe | README 证实：预编译二进制靠下载，源码编译要 LLVM+Python+Ninja，Windows 只列 msvc 目标——违背 windows-gnu + 构建期不联网 | — |
| 拒绝 | HEIC/AVIF 编码 | ravif 需 cargo-c/纳什码表在 gnu 链上构建风险大；边界内但代价不划算（§7） | — |
| 拒绝 | 抄 tldraw 代码 | tldraw SDK 是「tldraw license」非 OSI；只抄概念（运行时 API/快照） | — |
| 拒绝 | one-tool-per-function 式工具爆炸 | opencv-mcp 反面教材：20 个单函数工具 + 纯路径返回，agent 无法看图 | — |
| 拒绝 | 聊天同步/多人协作/插件市场/云账户/视频/PDF 表单 | 超出图片与设计工具边界，业主明令 | — |

## 2. Agent 端 token 经济

规范锚点：MCP 2025-06-18 是当前基线；versioning 页显示最新稳定版已到 **2026-07-28**（legacy 含 2025-11-25），rmcp 已按 2026-07-28 实现——升级传输层时评估（读 2026-10-09）。
Anthropic《Writing effective tools for agents》实测红线：**工具响应默认限 25k token**；「more tools don't always lead to better outcomes」；错误要「prompt-engineer… clearly communicate specific and actionable improvements」——我们的 `hint` 设计正中该条，保持。

### 2.1 预览图像返回体（最大出血点）
现状 `tools/call render.preview`（`cli/src/mcp.rs:80-90,212-228`）：
```
content:[ {type:"text",text:"<JSON≈140B>"}, {type:"image",data:"<base64 PNG ≤4MB>"} ]
+ structuredContent:<同一 JSON 再一遍>
```
一张 1080×1350 PNG（≈1.5 MB）→ base64 ≈ 2.0 MB 字符进上下文（≈50 万字符级 token 负担）。
改为默认（render.preview 本就返回 `url` 字段，`GET /file/<rel>` 路由已存在）：
```
content:[ {type:"text",text:"{\"path\":\"out/p1.png\",\"url\":\"http://127.0.0.1:8899/file/out/p1.png\",\"w\":1080,\"h\":1350,\"bytes\":1572864,\"sha\":\"…\"}"} ]
```
`inline:1` 才附 base64（供不能访问 loopback 的客户端），`none:1` 供只想要字节的 agent。
规范依据：tools 页明令「A tool MAY return links to Resources」，示例即 `{ "type": "resource_link", "uri": …, "name": …, "mimeType": … }`。
**每次预览节省 ≈1.9–4 MB 字符；一轮 6 次预览≈从「爆上下文」回到 KB 级。**

### 2.2 structuredContent 双份问题
draft/2025-06-18 tools 页原文：「a tool that returns structured content **SHOULD** also return the serialized JSON in a TextContent block」——SHOULD 而非 MUST。
现状：所有成功响应 text+structuredContent 双写。`project.get`（depth=1 可达 40 KB）直接翻倍。
建议：>2 KB 的载荷只留 `text`（客户端按 text 解析即可）；同时借 `fields` 把默认投影收窄。
估算：`project.get` 单次省 8–40 KB；一天 200 次读文档省 MB 级。

### 2.3 注解（annotations）补全
规范页原话：「clients **MUST** consider tool annotations to be untrusted unless they come from trusted servers」——注解不影响正确性但影响客户端审批策略。
官方 filesystem server 的社区整改 issue（servers#2988, 2025-11）给出语义样板：
`write_file`＝readOnly:false + idempotent:true + destructive:true（同路径同内容重复→同终态、可覆盖）；`edit_file`＝非幂等；`move_file`＝destructive:false。
对照我们 `mcp.rs:159-169`：只有 `readOnlyHint`/`openWorldHint`。要做三件事：
1. 加 `destructiveHint`（`canvas.delete`、`asset.remove`、`project.close(force)` → true）、`idempotentHint`（`canvas.update`/`layout.apply` 带同 patch → true，`asset.import` dedupe:1 → true）。每条 +20–30 B，是一次性静态成本。
2. **修 bug**：`template_use` 在 `is_read_only` 名单里，但它向文档添加节点（`rpc.rs` tools 表：template.use → `{ids}`）——被标只读会导致自动审批客户端直接放行破坏性套版。
3. `inspect_*`/`layout_explain`/`image_probe` 标 readOnly:true 正确；但 `render_preview` 会**写文件**——严格语义应改为 readOnly:false + destructive:false + idempotent:true（与 filesystem 的 write_file 同一格）。

### 2.4 游标分页 + 进度/取消
- 规范：列表方法用 cursor 参数 → 响应带 `nextCursor`（resources 页抓取确认；tools/list 不分页）。
- 落地：`project.list`、`log`、`history.list`、`jobs` 加 `cursor`/`n`（默认 n=25，响应尾 `{next:"c17"}`）。50 工程 ≈ 4.5 KB → 首屏 2.2 KB。`mcp.rs resources()` 的 `take(50)` 同样改游标。
- 进度：spec 要求在 `params._meta.progressToken` 存在时经 `notifications/progress` 推送（`progress`,`total`, 并「SHOULD provide relevant human readable progress information」）。我们 `jobs::Handle::progress(done,stage)` 语义一一对应，只差转发；长导出期间 agent 不再轮询 `jobs`。
- 取消：spec 有 `notifications/cancelled{requestId}`，server SHOULD「Stop processing」。现 transport 串行读 stdin（`mcp.rs:15-65`），通知到达时根本处理不了。**结构性缺陷**：长 `render.export` 会冻住整个 MCP 通道。方案：tools/call 先 `spawn_blocking` + 立即回 `{job:"j1"}`，或把 stdin 读循环与执行分离；两案都以 `notifications/cancelled` → `jobs::cancel(id)` 收尾。工作量 M，但这是「fewer ways to get it wrong」里最重的一条。

### 2.5 HTTP 面（`core/src/http.rs`）
- 现在一切 RPC 失败映射 `400 Bad Request`。加映射表：`REVISION_CONFLICT→409`、`*_NOT_FOUND→404`、`TOO_LARGE→413`、`PROVIDER_NOT_CONFIGURED→503`、超时→`504`。`Fail` 已有 `retryable` 字段，HTTP 侧补 `Retry-After`。
- 幂等：Stripe 样板——POST 带 `Idempotency-Key`，重发同 body→返回原结果，同 key 异 body→报错，key 保留 24h（stripe docs 抓取）。落地到 `POST /v1/projects`、`commands:batch`、`render.export`：参数里加短键 `key`（沿用短键文化），成功后按 key 缓存结果 JSON。网络抖动时 agent 从「probe 文件是否存在→补导出」两次往返降为 1 次重试即命中。

### 2.6 `inspect.validate` 默认投影
现状每条 `{sev,code,node,msg,e,fix}` + evidence 全量。对照 Anthropic「filter extraneous metadata / verbosity enum」建议：
默认 `evidence` 收缩为单值数字（如 contrast 只留 `ratio:3.2`），完整证据 `ev:1` 才展开；`fields` 已可用但要写进 `caps` 的示例回路里。一次 validate（30 条 issue）估省 40–60%（≈2 KB→0.9 KB）。
新增 issue code：`emoji-risk`（emoji/ZWJ 覆盖率——`covers()` 只看单码位存在性，ZWJ 序列必然「假通过」，见 §3.3-5）。

### 2.7 明确不做的反例（外部已踩坑）
- opencv-mcp：约 20 个单函数工具、结果给本地路径——agent 看不到图、tool 表爆炸。我们 40 粗粒度工具 + `caps` 自描述是更优形态，保持。
- ps-mcp `save_image`：入参「路径或 base64 二选一」、出参纯 JSON 确认——正确方向，与 §2.1 同构。
- 未给 `outputSchema`：规范「Servers MUST provide structured results that conform to this schema」（若声明）。我们没声明也没验证负担；**观察**：声明热门工具的 outputSchema 可让严格客户端提前拦截畸形重试，代价 +300 B/工具，暂不做。

## 3. 渲染与字体

### 3.1 resvg 现状（保留）与备选
- 0.45.1 的 usvg 文档确认：文本「completely resolved」且**不做换行**——与我们「引擎定行、每行一个 `<text>` 显式 x/y」的契约（INTERNAL-API.md svg 节）完全匹配，这是正确架构,别动。
- GTK 团队 2026-02 的 SVG 渲染器更新信里，验收标尺就是「I found the resvg testsuite, which has broad coverage」——换引擎等于放弃可用回归网，**resvg 保留**。
- **色管**（真缺口）：`imageops::load`（imageops.rs:280-313）解码后直接 `to_rgba8`，从不 `apply_icc`；moxcms（BSD-3/Apache-2.0，「ICC transforms and proofing」）**已在 Cargo.lock**（image 0.25.10 的依赖）。嵌入 ICC ≠ sRGB 的素材（相机原片、印刷素材）会整体偏色且 `image.compare` 无法察觉。改：load 时若有 profile 即 `di.apply_icc()`，probe 回报 `{icc:true}`。
- skia-safe：**拒绝**。rust-skia README（抓取）：build.rs「attempts to download prebuilt binaries」，否则「building from source requiring LLVM, Python 3, and Ninja」，Windows 支持只列 `x86_64-pc-windows-msvc / aarch64-pc-windows-msvc`。同时踩我们两条红线：构建期下载 + windows-gnu 不支持。
- vello 0.11.0：「compute-centric 2D renderer」基于 wgpu（需 GPU），pre-1.0；vello-cpu 官方自评「The renderer is still under active development」。观察，不进核心。
- tiny-skia：BSD-3，无文本层（正是 resvg 存在的原因）；若想在 imageops 里做免 SVG 往返的直接合成（渐变背景/圆角裁切复用同一光栅器）可直用——它是 resvg 传递依赖，零新增。观察。
- ab_glyph（「loading, scaling, positioning and rasterizing OpenType font glyphs」，无整形）/fontdue 同类：我们的光栅走 resvg，不需要；`text_image`（水印）若要连字/kerning，改走 **已在树里的 ttf-parser 0.25.1 + rustybuzz 0.20.1**，不引新 crate。
- 直接删除自研 TrueType 表解析（text.rs:1790-1930 手啃 hmtx/maxp/cmap 的段）→ 换 ttf-parser：少 140 行高风险手写二进制解析，行为可测。M，收益是正确性（符号 cmap 格式 12 等边角）。

### 3.2 测量 vs 导出的整形漂移
text.rs 头注自认「advance-width only (no shaping, no kerning, no hinting)」；出口 resvg 用 rustybuzz 整形 → 同一行在测量里 711 px、在 PNG 里可能 703–715 px（英文大字距 `To VA` 类 kern 对最明显）。AGENTS.md 已把它写成「亚像素差异」，其实**可控**：
`Registry::measure` 内对整行跑一次 `rustybuzz::shape`（face 来自同一 fontdb），累加整形后的 advance。零新依赖（rustybuzz 在树），`text.measure` 加 `shaped:1` 过渡。M。

### 3.3 CJK 换行规则缺口清单（对照 UAX #14，tr14 抓取）
现文本只有手搓四类表（text.rs:1593-1634）。逐条缺口 → 修法（每条都能写成单测）：
1. **缺失的字面闭标点**（违反「CL: Prohibit line breaks before」）：CLOSE 表没有 `』】〕〗〙〟`（U+300F/3011/3015/3017/3019）——「【注意】」的 `】` 可以合法地成为行首。补表：S。
2. **`「`(U+300C) 同时在 CLOSE 和 OPEN**（text.rs:1620 vs 1628）→ 开侧禁止断后 + 闭侧禁止断前 = `「`两侧都不能断，而 `「`本应允许出现在行首。从 CLOSE 删除。S。
3. **数字语境连字符**（LB20/21：「break after HY, **except in numeric context**」）：`breakable` 里 `is_hyphen(a)` 先于 CJK 判定返回 true，「2019-2024」「GPT-4」会在 `4-` 后断行。规则：`b` 为 NU（数字）时不许断。S。
4. **货币/数字粘合**（LB25 区：符号「may not be separated from preceding numeric characters…」；PR 不许与后随数字分离）：`＄99`、`¥99元` 夹在 CJK 之间时 `is_cjk(a)||is_cjk(b)` 一律放行 → 行尾可悬 `＄`、行首可悬 `%`。给 PR(￥$＄£€)/PO(%‰°)/NU 加三角规则。S。
5. **emoji ZWJ 序列**：tokenize 按码位迭代且 ZWJ 两侧默认不可断 → 序列**碰巧**不拆行（合规，tr14 LB8「A ZWJ prevents breaks…」）；但宽度=各码位 advance 之和（3 人家庭 ≈3× 单 emoji），且 `covers()` 逐码位查 `notdef` 永远「过」。→ §2.6 `emoji-risk` + 整形测量（§3.2）一并修复（swash/rustybuzz 按 cluster 出宽）。M。
6. **`§`/`※`/单位（㎡㏄）**：不在任何表内；tr14 抓取未覆盖其类别判定——标**未验证**，先用表驱动基座验证再定（下条）。
7. **区域指示符旗标**：非 CJK 默认粘合，行为正确（LB14/LB15a「no break within a pair」）——记录为已对。
8. **CJ 二义性**（tr14：CJ「treat as NS or ID」）：现把小假名一律当 NS（禁止行首），日文纵向排版「正常断行模式」会偏保守。产品可接受，写进注释即可。
9. **根治项（观察）**：引 `unicode-linebreak`（docs.rs 抓取：「Implementation of the Line Breaking Algorithm described in Unicode Standard Annex #14」，API `line_breaks()` 返回断点迭代器）作断点基座，自表只做 kinsoku 覆写——cosmic-text 正是「unicode-linebreak + swash」组合（docs.rs 抓取），即 Zed 在用的生产栈。纯 Rust、无网络，M；API 风险低（函数纯度极高）。

## 4. 图片处理能力缺口（对照 Sharp 操作面，均在边界内）

Sharp 操作页原文条目（抓取）里我们没有的：`median`（中值滤波）、`dilate`/`erode`（形态学「Expand/Shrink foreground objects」）、`unflatten`（「alpha channel with all white pixels made fully transparent」——扫描件去底）、`affine`、`convolve`（自定义核）、`autoOrient`（EXIF 转正**并删 tag**：我们 load 时隐式转正，但探针/导出链上用户无法显式控制）。
对照我们的 op 表（imageops.rs:1100-4019）还缺：
| 缺口 op | 建议签名 | 用途 | 实现路径 |
|---|---|---|---|
| `median n=3` | `op=median` | 去椒盐噪、掩膜净化 | imageproc::median 或自写直方图滑窗 |
| `morph=erode\|dilate` | 对 `remove_bg mask:1` 的后处理闭环 | 修发丝/边缘毛刺 | imageproc morphology（MIT，树外，纯 Rust）|
| `unflatten tol=8` | 白底→透明 | 旧 logo/扫描件 | 20 行自写 |
| `levels lo,mi,hi[,ch]` | `adjust` 无逐通道输入曲线 | 影调控制 | 自写 LUT，叠加现有 adjust |
| `auto-level` | 每通道百分位拉伸 | `normalize` 是全局均值法，非 IM auto-level | 直方图 40 行 |
| `unsharp r,s,threshold` | 现 `sharpen` 疑似 3×3 卷积核（待确认） | 印刷/缩略图细节 | tiny-skia 无关，自写高斯两次采样 |
| `channel extract/combine` | 抠单通道/合成 alpha 遮罩 | 配 `mask:1` 闭环 | 已有像素循环改造 |
| `auto-orient`（显式） | 转正+删 EXIF（对齐 Sharp 语义） | 批处理目录照片 | probe 已有 orientation，加 op |
| `webp` 有损编码 | 现在恒 lossless 并诚实注记（imageops.rs:373-382） | 电商主图体积 | 先验证 image-webp 0.2.4 的 `WebPEncoder`（树内）是否含 lossy；否则引 libwebp 绑定= C 构建风险 → 拒绝 |
| PNG 出片二压 | `oxipng::optimize_from_memory`（纯 Rust，「bit depth reduction and metadata stripping」）| 预览/PNG 交付减体积（幅度%：官方页未给数，未验证） | 渲染后离线步，opt-in `ox:1` |
| JPEG 编码侧 | `jpeg-encoder 0.7.1`「pure Rust… RGB/CMYK, chroma subsampling」 | image 内置编码器已同源；只有要 CMYK JPEG（印刷）时值得 | 观察，暂不加 |
palette/dhash/ahash 已有自研（imageops.rs:3822-4011），`image-palette`/`imagehash` crate **不加**（重复建设）。
**chain 语法升级**（ffmpeg 手册抓取：「Filters in the same linear chain are separated by commas, and distinct linear chains are separated by semicolons」，`[tag]` 命名垫、`overlay` 双输入）：
现 chain 串是单链线性（`crop > scale > sharpen`）。加 `;` 分链 + `[t]` 垫标签：
`"a=[crop 1:1]a; b=[scale 0.8]b; c=[overlay a b gravity:south]"` → `image.chain` 一呼完成拼图+叠加，替代「导出中间图→二次 chain」两往返（每往返≈150 B + 文件 IO）。M。
GEGL/ImageMagick 的其余大项（分形噪声合成、FFT、色彩查找表）超出现在收益，**拒绝**并移入 §7。

## 5. 编辑器机制可借鉴点

- **satori（vercel/satori，README 抓取）**——与我们的 SVG 编译/语义排版最相关，四条直接可用：
  1. **CSS 词汇即接口**：satori 只实现「Only these static and visible elements and properties」，但靠标准 CSS 名（`flexWrap/gap/rowGap/textTransform`…全部逐条列 supported）让模型零提示词迁移。→ `intent` 增 CSS 同义名（`justify-content/align-items/gap/wrap`），`caps` 里以「你已会的 CSS，这些名字」表述。省的是每 agent 会话几 KB 的教学 token。
  2. **换行能力诚实标注**：「overflowWrap Not supported, use wordBreak: break-word」→ 我们在 `text.measure`/`inspect.text` 回报 `breakable:true/false`（该行是否可能有合法断点），agent 不再靠试错缩字号。
  3. 「Satori does not guarantee that the SVG will 100% match the browser-rendered HTML」与我们的「以 render.export 文件为准」同一哲学——把它做成 `inspect.diff(预览vs导出)` 常态化提示，而非只在 AGENTS.md 一行。
  4. HarfBuzz 整形引文（「improves glyph shaping for complex scripts such as Arabic」）：我们无阿拉伯文需求，标观察。
- **polotno（docs 抓取）**：`page.addElement({ type:'text', x:50, y:50 … })` 的扁平元素 JSON 与我们 `canvas.add` 同构；值得抄的是它的**模板库形态**（template=可参数化 addElement 序列）——对应 `template.save/use/vars`，已具备。抄一点：其文档强调「无 UI 渲染导出」（设计自动化 headless）——正是 `poster`，用 `doctor` 里回报「headless OK」强化 agent 信心。0 工作量。
- **tldraw（README 抓取）**：「Runtime API – drive the canvas at runtime with the Editor API」+ custom shapes——验证我们 batch/命令模型的路线正确。概念抄一：`bindings`（锚点随被绑图形移动）≈ 我们的 `below:n_x` 引用约束，补一条 `layout.explain` 的「锚点已失效」issue code `unlinked`（已有）。**许可证注意：「tldraw SDK is provided under the tldraw license」——不可抄代码，只抄概念。**
- **fabric.js（README 抓取）**：「built in shapes, controls, animations, image filters… JPG, PNG, JSON and SVG i/o」——SVG 导入（`t=svg`）在 fabric 里是黑盒降级；我们的 `svg.compile` 反向（导出）为主。可抄的是它的 origin 模型（anchor 相对点）：`canvas.transform` 已带 `origin`，保持。
- **Figma REST（file-endpoints 抓取）**：三点照做——
  1. `GET /files/:key?depth=` 的 `depth` 我们有（`project.get depth:0..2`）✔；
  2. `geometry=paths` **按需才回几何** → 给 `project.get` 加 `geom:1` 开关（默认不回 resolved boxes，大文档省 30–60%）；
  3. `GET /images/:key` 渲染尺寸「scale between 0.01 and 4」与「32 megapixels」硬上限：我们的 60 MP 自限合理，但 `render.export` 应把 `sc` clamp 到 [0.01,8] 并在越界时返回 400+hint（现在静默 clamp 与否未读，待确认）。

## 6. 液态玻璃实现清单（WebView2/Chromium）

现栈盘点（glass.css，全部已验证在库）：漂浮色散 orb 背景场、`backdrop-filter: blur+saturate(180%)`、inset 顶/底 1px 高光、`mask-composite: exclude` 渐变描边、指针 sheen（radial + soft-light）、scrim 二次模糊。方向正确，抄下面四条即可收官：

1. **@supports 回退（必做，S）**。MDN：backdrop-filter 属「Baseline 2024」（Chrome/Edge 76+、FF 103、Safari 9.1）——WebView2 恒支持，但 `doctor` 里做一次性 CSS.supports 探测，缺失时降级 `--g-blur:none;` + 提高 tint 不透明度（玻璃→磨砂塑料）。片段（自写，无授权问题）：
```css
@supports not (backdrop-filter: blur(1px)) {
  .glass,.glass-1,.glass-2,.glass-3,.slab{ backdrop-filter:none;
    background-image:linear-gradient(160deg,rgba(255,255,255,.86),rgba(255,255,255,.72)); }
}
```
2. **`.bg-noise` 现在是空声明**（只有 opacity，无纹理源，glass.css:80）。补内联 SVG feTurbulence data-URI——参数直接复用我们 svg.rs:633 已在用的同款：`<svg><filter id=n><feTurbulence type="fractalNoise" baseFrequency="0.9" numOctaves="2"/></filter><rect filter="url(#n)"/></svg>`。S；无授权问题（参数是通用配方）。
3. **backdrop root 审计（会出真 bug，S）**。MDN 清单：祖先若带 `filter`、`opacity<1`、`mask/clip-path`、`mix-blend-mode≠normal`、`will-change:<上述>`，则玻璃只糊祖先范围内的像素——**根背景消失**。核对点：`.scrim` 淡入（`animation` 动 opacity）期间，`.sheet`（玻璃）挂在其下 → 入场动画全程模糊失效，落定才「砰」地出玻璃。修法：把淡入改为动 scrim 的 `background-color`/独立遮罩层，sheet 的动画只动 `transform`（transform 不在 backdrop-root 触发清单里）。另：`.bg-field i{filter:blur(0)}` 无害（它是玻璃的**背景**侧，不是祖先）。
4. **边缘折射 `backdrop-filter: url(#liquid-glass-edge)`（选做，M，仅 sheet/按钮）**。LogRocket（2025-12-08, Rahul Chhodde）给出机制：用 `<feImage>` 把「沿元素边框的厚度图」烘焙进滤镜、`<feDisplacementMap scale=…>` 位移背景像素，产生 Apple 式弯折；实测限制一致：「Chrome supports it; Safari/Firefox do not」——对 WebView2 恰好全支持。**rdev/liquid-glass-react（MIT, 6.4k★）**同一路线且 README 自证：「Safari and Firefox only partially support the effect (displacement will not be visible)」。落地纪律：只给 `.glass-3`/`.scrim .sheet` 加，给整窗 rail 加会每帧重跑滤镜链（性能税）；提供 `data-refract="0"` 关。**授权**：LogRocket 代码未标许可——按机制重写为原创 CSS/SVG，rdev 的 MIT 源可参考不可整体搬运样式表。
5. **明确标注为 cargo-cult 不抄**：给玻璃加外发光 drop-shadow「氛围光」、`saturate(400%)` 无脑加强（我们 180% 在合理带）、pointer-tracking 的色散像差（chromatic aberration，rdev 默认项——WebView2 全屏跟手成本）、任何「毛玻璃=越大圆角越透」的调参文章（无一手实现）。
6. 透明窗口 ×backdrop-filter 的已知撕裂：Tauri 搜索结果存在 issue #15947（Win10 transparent always-on-top）与 #15512（backdrop-filter 不生效）标题线索，**正文未能抓取成功——未验证**。动作：`doctor` 增加一条真机自检（截图比对玻璃区与非玻璃区像素方差），异常时在 UI 提示降级开关，而不是假装问题不存在。

## 7. 明确拒绝的建议

| 建议 | 拒绝原因 |
|---|---|
| tldraw `@tldraw/sync` 多人协作 / 聊天历史同步 | 超出「图片/设计工具」边界；且 tldraw license 非 OSI |
| 插件市场 / 云账户 / 素材 CDN / 以图搜图云端服务 | 边界外 + 违背「离线确定性」；核心引擎运行期零网络是产品红线 |
| skia-safe 替换 resvg | 构建期下载预编译二进制或 LLVM+Ninja 源码编；Windows 仅 msvc（§3.1） |
| vello（GPU）进 raster 主链 | 需 wgpu/GPU；vello_cpu 官方承认「still under active development」 |
| HEIC / AVIF 编码 | 专利（HEIF）与 cargo-c/ravif 在 windows-gnu 的构建风险；解码需求由用户转 PNG 代解 |
| libwebp 的 C 绑定换有损 WebP | CMake/纳什式构建在 gnu 链风险高于收益；先验证树内 image-webp（§4） |
| opencv-mcp 式 20 单函数工具 / 每响应内联 base64 | 工具爆炸 + token 爆炸双踩（§2.7、§2.1） |
| 在 MCP 响应里默认回 `resource_link` 指向 `file://` | 客户端沙箱不可读本地盘——统一指 `GET /file/` HTTP URL（loopback 已有路由） |
| 全窗所有表面加 SVG 位移折射 | 每帧滤镜链重排；WebView2 掉帧（§6-4 限定 sheet/按钮） |
| 阿拉伯/梵文整段整形（harfruzz 全量 GSUB） | 无需求方；出现时走 §3.3-9 的 unicode-linebreak+swash 组合再议 |
| FFT/分形合成/GIMP 级滤镜全家桶 | 边界内但收益密度低于 §4 表中小算子；从 GEGL 抄概念不抄体积 |

## 8. 参考来源（均为 2026-10-09 实际抓取并引用其中具体机制）

- MCP 规范（2025-06-18）· Tools：structuredContent/outputSchema、resource_link 示例、annotations 不受信任 https://modelcontextprotocol.io/specification/2025-06-18/server/tools
- MCP 规范 · Changelog（structured tool output / resource links）https://modelcontextprotocol.io/specification/2025-06-18/changelog
- MCP 规范 · Resources（cursor 分页）https://modelcontextprotocol.io/specification/2025-06-18/server/resources
- MCP 规范（draft）· Tools（`additionalProperties:false`、text 兼容 SHOULD）https://modelcontextprotocol.io/specification/draft/server/tools
- MCP 规范 · Progress / Cancellation https://modelcontextprotocol.io/specification/2025-06-18/basic/utilities/progress https://modelcontextprotocol.io/specification/2025-06-18/basic/utilities/cancellation
- MCP 规范 · Versioning（最新 2026-07-28）https://modelcontextprotocol.io/specification/versioning
- Anthropic《Writing effective tools for agents》（25k token 上限、聚合工作流、错误提示工程）https://www.anthropic.com/engineering/writing-tools-for-agents
- MCP servers issue #2988（write_file/edit_file 注解样板）https://github.com/modelcontextprotocol/servers/issues/2988
- Stripe API · Idempotent requests https://docs.stripe.com/api/idempotent_requests
- rmcp（Rust SDK，spec 2026-07-28、server feature、tokio 必需）https://docs.rs/rmcp/latest/rmcp/
- opencv-mcp-server（20 单函数工具、路径返回）https://github.com/GongRzhe/opencv-mcp-server
- ps-mcp · save_image（JSON 确认 + 路径/base64 入参）https://glama.ai/mcp/servers/@duke0317/ps-mcp/tools/save_image
- UAX #14 换行算法（类表与 LB 规则引文）https://www.unicode.org/reports/tr14/
- unicode-linebreak https://docs.rs/unicode-linebreak · cosmic-text https://docs.rs/cosmic-text · swash https://docs.rs/swash · ab_glyph https://docs.rs/ab_glyph
- usvg（文本完全解析、不换行）https://docs.rs/usvg · tiny-skia https://docs.rs/tiny-skia · resvg testsuite 引用于 https://blogs.gnome.org/gtk/2026-02-25/an-update-on-svg-in-gtk/
- rust-skia README（二进制下载/LLVM+Ninja/仅 msvc）https://raw.githubusercontent.com/rust-skia/rust-skia/master/README.md
- vello https://docs.rs/vello · vello-cpu https://docs.rs/vello-cpu · moxcms https://docs.rs/moxcms · oxipng https://docs.rs/oxipng · image-webp https://docs.rs/image-webp · jpeg-encoder https://docs.rs/jpeg-encoder · imageproc https://docs.rs/imageproc
- satori README（CSS 支持清单、无 overflowWrap、HarfBuzz、浏览器差异不保证）https://github.com/vercel/satori
- polotno docs（page.addElement JSON）https://polotno.com/docs/ · tldraw README（Runtime API、license）https://github.com/tldraw/tldraw · fabric.js README https://github.com/fabricjs/fabric.js
- ffmpeg 滤镜图语法（逗号链/分号链/overlay 双输入）https://ffmpeg.org/ffmpeg-filters.html#Filtergraph-syntax
- Figma REST file-endpoints（ids/depth/geometry、images scale、32MP）https://developers.figma.com/docs/rest-api/file-endpoints/
- MDN backdrop-filter（backdrop root 触发清单、Baseline 2024、url() 语法）https://developer.mozilla.org/en-US/docs/Web/CSS/backdrop-filter
- LogRocket：CSS+SVG 液玻折射（feImage/feDisplacementMap，Chrome-only）https://blog.logrocket.com/how-create-liquid-glass-effects-css-and-svg/
- rdev/liquid-glass-react（MIT、位移在 Safari/FF 不可见）https://github.com/rdev/liquid-glass-react · 同主题仓库（iyinchao/liquid-glass-studio、huozhi/vaso）https://github.com/topics/liquid-glass-effect
- SVG2 文本模型（inline-size 换行 vs tspan x/y）https://www.w3.org/TR/SVG2/text.html
- 未抓全线索（未验证）：Tauri #15947 / #15512（transparent 窗口与 backdrop-filter）、image-webp lossy 编码细节、我们 `sharpen` 核型
