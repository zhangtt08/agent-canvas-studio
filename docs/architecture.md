# 架构

## 一句话

一份版本化 Scene Graph 是唯一真相；桌面窗口、HTTP、MCP、CLI 都是它的投影和写入口；
预览和导出共用同一条 SVG 编译链路。

```
              ┌──────────────────────────────────────────┐
              │ 内置 Agent / 外部 Agent / 你 / CI 脚本    │
              └───────┬──────────────┬──────────┬────────┘
                      │              │          │
              invoke("rpc")     MCP stdio   HTTP / CLI
              (Tauri 窗口)      tools/call  POST /rpc · agentcanvas m
                      └──────┬───────┴─────┬────┘
                             │             │
                     ┌───────▼──────┐  ┌───▼────────────────────────────┐
                     │ rpc::call    │  │ http.rs  /asset /font /file     │
                     │ 校验·rev·日志 │  │ (给 webview 和给 agent 是同一个) │
                     └───────┬──────┘  └────────────────────────────────┘
                             │
        ┌────────────────────▼─────────────────────┐
        │            Application layer             │
        │  project · commands · history · jobs     │
        │  inspect · templates · assets            │
        └────────────────────┬─────────────────────┘
                             │  唯一状态：Document (doc.rs)
        ┌──────────┬─────────┼─────────┬──────────┐
        │ layout   │ text    │ svg     │ imageops │
        │ 约束求解 │ 字体测量 │ 编译器  │ 栅格处理 │
        └──────────┴─────────┴────┬────┴────┬─────┘
                                  │         │
                            ┌─────▼───┐  ┌──▼──────────────┐
                            │ render  │  │ pdf             │
                            │ resvg   │  │ FlateDecode RGB │
                            └─────────┘  └─────────────────┘
```

## 关键不变量

**1. 唯一真相源。** `core/src/doc.rs` 的 `Document` 是唯一状态。UI 不维护"自己的那份图层列表"，
它渲染 `project.get` 的返回、并把自己的操作发回同一个 `rpc::call`。因此不存在
"GUI 改了但后台不知道"这类 bug——两边根本是同一份内存里的项目（`session` 的进程级注册表）。

**2. 命令式写入。** 每一次修改都是一个 `commands::Cmd`：`NodeAdd / Transform / NodeSet /
LayoutIntent / Group / Reflow / Composite ...`。命令携带自己的逆命令（`Applied::undo`），
历史栈只是命令列表，所以撤销不需要快照整份文档，且人工拖拽和智能体调用产生**同一种**历史条目。
`Composite` 是事务：先克隆文档再逐条应用，任何一条失败就整体回滚，revision 不动。

**3. 意图与结果分离。** 节点存 `layout.intent`（你要什么）和 `layout.box`（求解结果）。
重排、换比例、系列变体都只改意图；手工拖过的节点会被标成 `manual`/`pinned`，
下一次自动排版不会悄悄覆盖它。`layout.explain` 读 `provenance` 回答"它为什么在这儿"。

**4. 一条绘制路径。** `svg::compile(doc, opts)` 是唯一的绘制实现。
预览时 `opts.preview=true`，图片走 `/asset?id=`、字体走 `/font?family=`（本地 HTTP 提供），
浏览器和 resvg 因此拿到**同一份字体文件**；导出时 `opts.inline_assets=true`，
同一字符串交给 resvg。差异只剩栅格化器本身，不再有"两套渲染代码"。

**5. 可观察。** 每个写调用返回 `{ok, rev, changed}`；每个失败返回
`{e, code, hint?, details?, retryable?}`；异步任务有 id、进度、阶段、日志、可取消。
导出返回真实 `sha256`。判定成功靠结构，不靠自然语言"已完成"。

## 分层与依赖方向

```
layer 0  error · util · geom · doc          无内部依赖
layer 1  text · assets · imageops · session · jobs
layer 2  layout · svg · pdf
layer 3  render · inspect · templates · project · history · commands
layer 4  rpc                                唯一聚合点
layer 5  http                               只有传输和素材流
```

上层可以依赖下层，反向不允许；`rpc.rs` 是唯一允许看见所有模块的地方。
这条规则让"改渲染不影响排版"成为结构上的保证而不是口头承诺。

## 并发与一致性

- `rev` 乐观锁：写调用可带 `rev=<读到的值>`，过期返回 `REVISION_CONFLICT` + 当前值。
- 进程级项目注册表用 `Mutex` 保护，一次 `with_project` 内完成读-改-写，避免半更新。
- 落盘是原子的：写 `*.tmp` 再 `rename`（`util::write`），崩溃后不会留下半个 `project.json`。
- HTTP 只绑 `127.0.0.1`；设置 token 后除 `/health` 外全部要求 `Authorization: Bearer`。

## 安全边界

- 所有路径经 `util::inp/outp/check_writable`，默认沙箱在工作区根目录，写越界返回 `PATH_DENIED`；
  读越界需要文件真实存在或显式 `AGENTCANVAS_OPEN_FS=1`。
- 导入 SVG 在编译阶段消毒：剥掉 `<script>`、`on*` 属性和外部 `href`，不执行素材内任何脚本。
- 解码前查尺寸与体积（60 MP / 200 MB），解压炸弹和超大图在入口就被拒。
- 字体只用系统已安装字体，交付包不含任何不可分发字体文件。

## 性能

- 布局求解是增量定点迭代（≤8 轮），只影响必要子树，手工微调不会随机漂移。
- 预览默认 `sc=0.5`；素材缩略图落盘缓存；同一 SVG 字符串在导出多格式时复用。
- 栅格处理用 rayon 并行像素循环；`image.batch` 里每个文件独立成功/失败。
- 具体耗时见 `tests/` 输出与交付报告，文档里不编造毫秒数。

## 为什么不拆成两个产品

交接文档要求"本质是两个软件"——指的是**两个入口**，不是两套实现。
拆成两套会立刻违反不变量 1 和 4：GUI 与后台各自维护状态、各自实现渲染，
于是"智能体做的和人做的不一样"成为结构性缺陷。这里的做法是：
一个 Rust crate（`agentcanvas-core`），两个可执行入口（`agentcanvas-app` 窗口 / `agentcanvas` 无界面），
零重复业务代码。
