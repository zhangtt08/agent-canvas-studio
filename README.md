# AgentCanvas

**为 AI 代理而生的图片与设计引擎 —— 同一个引擎，三扇门。**

- 你在一个**无系统边框、液态玻璃**的桌面窗口里做设计；
- 智能体通过 **MCP / HTTP / CLI** 做**完全相同**的事，可以在没有界面的机器上跑；
- 两边改的是**同一份文档**，撤销是同一条历史，日志是同一条记录。

```
你拖动一个图层   ┐
内置 Agent 调用  ┼─→ 同一份 Scene Graph → 同一条 SVG 编译链路 → 同一个渲染器 → 真文件
外部 MCP 客户端  ┘
```

## 30 秒上手

```powershell
node scripts/build.mjs                 # 引擎 + 前端 + 桌面 exe，产物在 dist/
dist\AgentCanvas.exe                   # 双击打开，无系统边框的玻璃窗口
dist\bin\agentcanvas.exe caps          # 智能体侧：一次调用读完整个产品
dist\bin\agentcanvas.exe poster title=新品上市 image=product.png preset=poster
```

开发模式：

```powershell
npm --prefix ui install
cargo install tauri-cli --version "^2"   # 一次性
cargo tauri dev -c app                   # 热更新的桌面窗口
```

不装桌面 App 也能用（这是设计目标之一）：

```powershell
dist\bin\agentcanvas.exe http --port 8899   # REST + 素材/字体流
dist\bin\agentcanvas.exe mcp                # stdio MCP 服务，给任意智能体客户端
```

## 给智能体

**先读 [`AGENTS.md`](AGENTS.md)** —— 一页讲完调用方式、短键、语义布局、错误码和已知边界。
再读 `agentcanvas caps`（约 5 KB，包含全部方法、图片 op、模板、限额）。

MCP 挂载：

```json
{ "mcpServers": { "agentcanvas": { "command": "C:\\...\\dist\\bin\\agentcanvas.exe", "args": ["mcp"] } } }
```

HTTP：

```http
POST http://127.0.0.1:8899/rpc
{"m":"poster","p":{"title":"新品上市","image":"C:\\pics\\a.png","preset":"poster","f":"png"}}
```

工具面被刻意聚合成 ~40 个粗粒度条目，而不是上百个细方法——每个 schema 都要占上下文。

## 给人

| 页面 | 里面有什么 |
| --- | --- |
| **首页** | 最近工程卡片、尺寸预设、模板、本地接口状态与一键复制的 MCP 配置 |
| **工作室** | 左：画板/图层/素材/模板；中：真实画布（引擎自己画的 SVG）；右：样式/布局/检查；右下：Agent 面板 |
| **图片处理** | 拖入图片 → 链式操作（裁切/缩放/调色/锐化/水印/背景/圆角/压缩）→ 前后对比 → 批量导出 |
| **Agent 接入** | MCP 配置、HTTP 示例、工具清单、真实调用日志、任务进度与取消 |

界面里没有小字说明——所有提示都做成 tooltip、分段控件、空状态、检查面板里的可点击问题行。
所有面板都在同一层里，导出/设置/确认都是玻璃浮层，不跳页。

### 设计上的两个硬要求

1. **无系统边框**：窗口没有 OS 标题栏，最小化/最大化/关闭是 UI 自己的三枚玻璃圆钮，
   和顶栏同一材质、同一悬停语言；圆角 18px、1px 高光描边，最大化时自动去掉圆角。
2. **液态玻璃**：底层是一张缓慢漂移的渐变光斑 + 噪点，上层面板用
   `backdrop-filter: blur + saturate` 加内高光、边缘折射和跟随指针的镜面反射；
   三层深度（bar / panel / sheet）用不同模糊与投影区分。

## 能力

**设计侧**：多画板工程、图层树（组/蒙版/裁切）、文字与中英混排换行、语义约束排版
（`below` / `center_in` / `align_to` / `flex` / `grid` / `safe_margin` …）、渐变/描边/圆角/阴影/模糊、
模板与变量、系列变体、多比例重排、PNG/JPG/WebP/SVG/PDF 导出、机器可读检查报告。

**图片侧**：`crop resize scale pad rotate flip round border adjust blur sharpen noise gray
threshold dither tint overlay watermark grid stack background remove_bg strip optimize
palette dhash normalize equalize vignette pixelate emboss edge shadow trim`
+ 链式（`"crop 1:1 > scale .5 > sharpen 1.2"`）+ 批量 + 预设 + 感知哈希比对。

## 结构

```
core/    Rust 引擎：doc / geom / text / layout / svg / render / pdf / imageops /
         assets / project / commands / history / inspect / templates / jobs /
         session / rpc / http          ← 唯一的业务实现
cli/     agentcanvas.exe：一次性调用、mcp、http、caps、doctor
app/     Tauri 桌面壳：无边框玻璃窗口 + invoke("rpc")
ui/      React + Vite，只画界面，不实现任何业务规则
tests/   cargo 单测 + 场景 A/B/C/D/E 的端到端验收（真文件、真 sha256）
docs/  DELIVERY.md（交付报告：实现/命令/验证/限制）、INTERNAL-API.md（模块契约）、METHODS.md（方法面）、架构、接入、示例与故障恢复
examples/可直接跑的工程与导出结果
```

## 验证

```powershell
node tests/run-all.mjs        # cargo test + 场景 B(MCP 无界面) + E(HTTP/CLI) + A/C/D + UI 构建 + 桌面冒烟
```

## 边界（不粉饰）

- 没有模型密钥时，确定性能力全部可用；需要模型的能力（AI 抠图、以图生图、审美复核）返回
  `PROVIDER_NOT_CONFIGURED` 并给出替代方案，不会假装做过。
- 字体取本机已安装字体，交付包不含任何不可分发字体；缺中文字体时 `doctor` 会明确报出来。
- 浏览器预览与 resvg 导出在极端字距下可能有亚像素差异，**以导出文件为准**。
- 内置 Agent 在没有模型时走模板化确定性生成，界面会显示它实际调用了哪些工具。
