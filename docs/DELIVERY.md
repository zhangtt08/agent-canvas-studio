# 交付报告 · AgentCanvas 1.0.0

生成时间：2026-10-10（本机 Windows 10.0.26200 / x64）。
本报告只写已经在同一台机器上跑出来的结果；没跑过的写进"尚未验证"。

## 1. 这是什么

一个 Rust 引擎、两个可执行文件、一份文档模型、一条渲染链路：

| | 桌面应用 | 后台运行时 |
| --- | --- | --- |
| 入口 | `dist/AgentCanvas.exe` | `dist/bin/agentcanvas.exe` |
| 形态 | Tauri 2 无边框透明窗口 + 液态玻璃 UI | CLI / MCP(stdio) / HTTP(回环) |
| 状态 | 同一个 project 目录、同一个 `revision` | 同上，互相即时可见 |

两边不是两套实现：`core/src/rpc.rs` 的 `call()` 是唯一出口，桌面窗口和 CLI 走同一个函数、
同一套短键、同一条 resvg 光栅化路径。窗口里改一笔，CLI 立刻读到新 `rev`；CLI 写一笔，
窗口下一次读取就换文档（`session::sync_with_disk`，被
`a write by another door becomes visible on the next read` 这条 Rust 测试钉住）。

设计要求的三条都已落地：无系统边框（`decorations(false) + transparent(true) + shadow(false)`，
自绘的最小化/最大化/关闭按钮与标题区同层，拖拽区用 `data-tauri-drag-region`）、
液态玻璃组件（`backdrop-filter: blur()+saturate()` + 漂移光斑 + 高光边 + 噪声层，
`ui/src/styles/glass.css`）、页面功能集成（首页 / 工作室 / 图像实验室 / Agent 接入四个视图，
调用时间线就在接入页里；不用小字提醒，用分组、层级、按钮位置和即时反馈表达可点性）。

## 2. 对照必须交付清单

| 交接文档 §13 | 在哪 |
| --- | --- |
| 1 源码/依赖/锁文件/编译配置/规范/启动脚本 | `core` `cli` `app` `ui` `scripts`，`Cargo.lock`、`rust-toolchain.toml`、`.cargo/config.toml`、`AGENTS.md` |
| 2 可运行桌面应用 | `dist/AgentCanvas.exe`（13.6 MiB）+ `WebView2Loader.dll` + `BUILD.json` |
| 3 Headless HTTP / MCP / CLI + 可复制命令 | `dist/bin/agentcanvas.exe`，`docs/mcp-setup.md`、`docs/api-reference.md`、应用内"接入"页直接生成 `mcpServers` JSON |
| 4 真实工作的 Document/Canvas/Layout/Asset/Renderer/Inspector/History/Persistence | `core/src/{doc,layout,assets,render,inspect,history,session,project}.rs` |
| 5 内置 Agent 接入、Provider 配置页、Tool Registry、调用记录 | `core/src/{provider,rpc}.rs`、设置页的模型区块 + `system.provider.probe`、`log` 视图 |
| 6 API 文档、工具表、Schema、示例请求与真实返回 | `docs/{METHODS,tool-reference,schema,api-reference}.md`，真实返回由 `node tools/api-capture.mjs` 生成 |
| 7 单海报/系列/编辑封面/批量比例示例工程 + 真实导出 | `examples/`，5 个固定 id 工程 + 35 个工件 + `MANIFEST.json`（27 张图带 sha256） |
| 8 测试代码、自动测试命令、执行结果摘要、端到端证明 | `tests/run-all.mjs`（10 个 suite），下方"验证结果"就是摘要 |
| 9 用户文档、架构文档、异常恢复说明 | `docs/{desktop-guide,architecture,provider-setup,INTERNAL-API,troubleshooting}.md` |
| 10 最后交付报告 | 本文件 |

## 3. 运行命令

```powershell
# 一次性打包（release + UI + dist）
node scripts/build.mjs

# 全量验收（10 个 suite，含 cargo test / MCP / HTTP / 场景 A-E / 桌面冒烟）
node tests/run-all.mjs

# 示例工程重建 + 二次运行验证逐图确定性
node examples/run-all.mjs
node examples/run-all.mjs

# 桌面
dist\AgentCanvas.exe

# 后台三件套
dist\bin\agentcanvas.exe caps
dist\bin\agentcanvas.exe poster title=夏日轻乳 image=a.png preset=poster f=png out=out
dist\bin\agentcanvas.exe mcp
dist\bin\agentcanvas.exe http --port 8899
curl -s -X POST http://127.0.0.1:8899/rpc -d '{\"m\":\"caps\"}'
```

Rust 工具链在这台机器上是 `x86_64-pc-windows-gnu`（没有 Windows SDK，MSVC link 不可用），
需要 `C:\Users\EDY\tools\mingw64\bin` 在 `PATH` 上；`tests/run-all.mjs` 已把这行放进它自己的 env。

## 4. 验证结果（同一次运行，2026-10-10 01:2x）

```
================ GATE ================
PASS  suite imports
PASS  release build (engine + cli)
PASS  cargo test                       103 passed / 0 failed  (53 单测 + 11 history + 21 inspect + 6 layout + 3 render 像素校验 + 9 templates)
PASS  scenario B (MCP headless)        61 passed / 0 failed
PASS  scenario E (HTTP + CLI)          47 passed / 0 failed
PASS  scenarios A/C/D (acceptance)     84 passed / 0 failed
PASS  ui typecheck (tsc)
PASS  ui build (vite)
PASS  desktop smoke (frameless window) 17 passed / 0 failed
PASS  shipped copy is current

10/10 suites green

examples: 13 passed / 0 failed   (第一次：建立 golden 记录)
examples: 13 passed / 0 failed   (第二次：27 张图 sha256 逐张相同)
```

2026-10-11 复跑（§4b 的三处修好之后）：同一套命令仍然 10/10，
但示例海报的 golden 全部重建——素材真的画上去了，当前逐图字节与 sha 见
`examples/MANIFEST.json`（例如 `examples/product-poster/out/product-poster.png` 242 KB、
`examples/headless-mcp/out/headless-mcp.png` 138 KB）。
证据不是字节数而是像素：把 `Some(&prj.dir)` 改回 `None`，新测试报
`the hero photo painted 0.0000 of the canvas`；改回来就绿。

桌面冒烟不是"能起来"：它启动打包后的 exe，从真实窗口里读 `system.health.pid`，
确认窗口连的就是这个引擎，再从 CLI 写一笔改动、要求窗口报出新 `rev`，
最后 taskkill 整棵树并确认进程真的退出。

本轮针对智能体调用做的适配，以及它的实测数字：

| 改动 | 效果 | 由什么钉住 |
| --- | --- | --- |
| `system.tools` 默认只回 `[{n,m,d}]`，`full=1` 才带 JSON Schema | 16 569 → 4 770 字节（-71%） | `system.tools without full=1 costs less than half` |
| `agentcanvas batch jobs=[{...}]` 不再被 CLI 的 batch 动词吞掉；`k:=json` 按文档生效 | 省往返的方法在 CLI 上真的能用 | 3 条新 acceptance 断言 |
| 整数不再写成 `1440.0`（`rpc::call` 出口统一 `whole_numbers`） | 文档返回 -1~2%，且小数值原样保留 | `whole_numbers_drop_the_dot...` + `a whole number is written without a decimal point` |
| `poster`/`project.create` 接受固定 `id`，路径可复现 | `preview.png` 等路径两次运行一致 | examples 的逐图 sha256 检查 |
| `system.health.root` 回真实绝对路径（原来是 `""`） | 智能体能直接抄进 MCP 配置；代价是这条返回多约 60 字节 | `system.health names the root it actually resolved` |
| examples 跑之前清空 `projects/`、`log/`、`recents.json` 与 `out/` | 第二次运行与第一次同世界，确定性检查才有意义 | `regenerated images match the previous golden record` |

每条新断言都先跑成红再跑成绿：`.0` 检查与 root 检查在改动前捕获的真实返回上确实会失败
（`"h":1440.0`、`"root":""`），golden 确定性检查用改写一位 sha 的方式证明它会红。

## 4b. 真实使用测试发现的三个缺陷（2026-10-11）

第二轮不是回归测试，是"拿它做一张真海报"。做图过程中发现：文件都生成了、`inspect.validate`
报 0 error、99 个 Rust 测试全绿——**但海报里没有照片**。三处同一个根因族：

| 症状 | 根因 | 修法 | 钉住它的测试 |
| --- | --- | --- | --- |
| `poster` 一句话出图，导出的 PNG 没有素材 | `render::export` 用同一个 `dir` 既当输出目录又当素材根目录；`templates::poster` 传的是 `exports/`，于是 `<project>/assets/*.png` 找不到，resvg 静默不画 | `render::export_with(..., Some(&prj.dir))` | `poster_actually_paints_the_imported_image`（数像素） |
| `render.preview` 写出的 PNG 也没有图（首页大卡片右下那块灰） | 两处：预览编译时 `inline_assets=false` + `asset?id=` 相对 URL，进程内的光栅化器无处解析；即使改成内联，`svg::compile` 仍不带工程目录，素材路径照样落空 | 预览改为内联 + `svg::compile_at(..., Some(&pr.dir))` | `preview_png_contains_the_photo_not_just_the_text` |
| 保存时刷新的工程缩略图（最近画布卡片）同样丢图 | `project::refresh_preview` 调 `to_bytes`，素材根目录是 `None` | 传 `Some(&p.dir)` | `the_saved_card_preview_contains_the_photo_too` |

同时补了一条"降级必须说出来"的通道：素材读不到时 `svg` 编译器记一条 warn，
`render.*` 的返回里带 `warns`（为空时不出现，不占 token）。字体回退这类正常提示也走同一通道，
所以测试断言的是"没有丢素材"，不是"没有任何 warn"。

**为什么原来那 192 条断言没抓到**：它们验的是"文件存在、字节非空、sha 对得上"，
golden 图比对是同一趟流水线的两次运行——一个系统性丢图的 bug 在两次运行里完全一致。
把断言换成"读回像素、数它是不是素材的颜色"之后，把 `Some(&prj.dir)` 改回 `None`，
测试立刻报 `the hero photo painted 0.0000 of the canvas`。这条教训记进了
`AGENTS.md`：判断一张海报有没有吃到素材，看像素，别看文件存在。

## 5. 尚未验证 / 没有做的

- **安装包**：`scripts/build.mjs --bundle` 需要 WiX 或 NSIS，本机没有；交付的是免安装 `dist/` 目录（exe + dll 同级）。双击可运行已验证，"安装向导"未验证。
- **出网调用模型**：只实现了可达性探测 `system.provider.probe`（真实 TCP + HTTP，测试里用 `TcpListener` 桩验证两条路由）。没有用任何真实密钥跑通一次生成请求；无密钥时内置 Agent 走确定性编辑工具，这部分是有证据的。
- **Apple / Linux 平台**：只在 Windows x64 编译与运行过。
- **10 亿像素级压力**：`max_px` 限额有测试，但没有用它压到崩溃来量化余量。

## 6. 外部依赖条件

- Windows 10+ 的 WebView2 运行时（Win11 自带；Win10 需 Evergreen Runtime）。
- Rust `stable-x86_64-pc-windows-gnu` + MinGW-w64（只在重新编译时需要）。
- Node 24（构建、测试、示例）。
- 系统字体：CJK 依赖系统里存在中文字体，`system.doctor` 会报 `cjk_ok`；没有时渲染走 fallback 并在 `text.measure` 的 `fell_back` 里如实说明。
- 模型 Provider 完全可选；不配也能用全部编辑/排版/导出能力。

## 7. 真实限制与已知问题

1. **origin = 最后一个写的人**。`meta.origin` 记录最后一次写入来自哪扇门（desktop/cli/mcp/http），
   不表示文档的"归属"。两扇门交替写时它会来回变，这是设计而非 bug，但读的人容易误解。
2. **手动放置后不再自动重排**。`Cmd::Transform` 会把节点视为用户接管（pin 语义），
   此后 `layout.resolve` 不动它；想交还给求解器要显式解锁。批量脚本里如果先 Transform 再 reflow，
   结果和预期不同。
3. **`clear_history` 只清撤销栈，不删磁盘上的 `history.jsonl` 之外的快照**；`versions/` 目录要手工清理。
4. **关闭窗口没有"未保存"弹窗**：autosave（默认 60s）+ 每次命令落盘兜底，进程被强杀时最多丢一次未 flush 的改动。
   这是"用交互代替小字提醒"的取舍，不是漏实现。
5. **`core` 编译有 23 条警告**：其中 13 条是 `cargo fix` 能直接吃掉的未用 import / 死代码
   （`undo_extra`、`rf`、`col_req`、`f8`、`crop_area` 等），另有 `viewBox` 字段命名（对齐外部 SVG 属性名，
   改成 snake_case 反而难读）、`text.rs:1469` 禁则表里 `'\u{FFE5}'` 写重了一处（重复项永不命中，行为不变）、
   以及 linker 关于 manifest 合并的提示。没有一条指向错误行为，但没清干净是我的债。
6. **`image.probe` 对 root 外的文本文件报 `UNSUPPORTED`（"这个格式解不出来"）**，
   而不是抱怨路径。读文件是允许跨 root 的（导入素材本来就从任意目录拿），写才受限——
   写越界会明确 `PATH_DENIED`，这条有 acceptance 断言。
7. **示例工程固定在 `examples/`**：`node examples/run-all.mjs` 会重建并覆盖 5 个固定 id 的工程目录。
   想在示例上手工改一版留档，请先复制出去。
8. **渲染降级只在返回里说，画布上还不画红字**：`render.*` 的 `warns` 现在会告诉智能体"素材读不到"，
   但桌面窗口里没有对应的视觉提示（按"不用小字提醒"的设计约束，正确做法应是在那块图的位置显示
   占位与素材名）。这是下一轮该补的 UI 债，不影响导出正确性。

## 8. 复现验收

```powershell
node scripts/build.mjs ; node tests/run-all.mjs ; node examples/run-all.mjs ; node examples/run-all.mjs
```

四步全绿即为交付状态。`tests/run-all.mjs` 里 `shipped copy is current` 会比对 `dist/` 与
`target/x86_64-pc-windows-gnu/release/` 的字节，专门防止"用旧二进制判分"这一类假绿。

上面的 10/10 是在删除 `target/` 之前跑的。删除之后又单独重跑了
`acceptance` / `mcp-e2e` / `http-e2e` 三套 e2e 与 `desktop smoke`（打包窗口），
此时 `tests/lib.mjs` 只能解析到 `dist/bin/agentcanvas.exe`——全绿，说明免安装的 `dist/`
自己就是完整的可交付物，不依赖编译缓存。要重新编译再 `cargo build --release` 即可。

## 9. 已删除的临时物

- `docs/INTEGRATION.md`：并行开发期间的 ADAPT 清单，它的删除条件是"每条都确认过且 cargo test 绿"。
  逐条核对结果：`http_port()` 已是 `http::port()`、`pub mod http/rpc` 已接回、
  各模块函数签名与 `docs/INTERNAL-API.md` 一致（不一致的地方以模块实际形态为准并写进了文档）、
  `revision` 只在 `session::with_project` 里 bump（A6 的 rev 连续性断言盯着）。清单里全部 20 项都已落地，故按它自己的规则删除。
- `.scratch/`、仓库根的 `AgentCanvas/`（默认 root 跑出来的工程/日志状态）、`shots/_base-*` 与散落的 `p_*` 截图、`target/`（12 GB 编译缓存，重新 `cargo build` 即回）。
  `dist/`、`examples/` 与其导出图保持原样——那是交付物本身。

2026-10-11 交付前最后一次精简，仓库 39 MB：

| 删掉 | 体积 | 怎么回来 |
| --- | --- | --- |
| `target/` | 7.3 GB | `cargo build --release --workspace` 约 10 分钟；`node tests/run-all.mjs` 的第 1 个 suite 就是它 |
| `ui/node_modules/` | 70 MB | `node scripts/build.mjs` 第 45 行自己 `npm install` |
| `shots/home.png`、`shots/poster-step.png` | 0.7 MB | 开发期草图，全仓无引用；`shots/shipped-*.png` 留着（打包窗口实拍），`node tests/ui.mjs` 可重生成 |
| `examples/log/`、`examples/recents.json` | 小 | 引擎运行态，`examples/run-all.mjs` 每次重建 |

删完用 `dist/bin/agentcanvas.exe` 重跑三套 e2e（场景 A/C/D、B、E）全绿：交付物不依赖编译缓存。
