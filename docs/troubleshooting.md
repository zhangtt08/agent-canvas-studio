# 常见问题与异常恢复

## 打不开 / 起不来

**双击 `AgentCanvas.exe` 没反应。**
桌面 exe 依赖同目录的 `WebView2Loader.dll`。从 `target/release/` 单独复制 exe 会丢掉它——
用 `node scripts/build.mjs`，它会把 dll 一起放进 `dist/`。
另外 Windows 需要装过 WebView2 Runtime（Win11 自带，Win10 一般随 Office 装了）。

**窗口是黑的 / 什么都没有。**
开发模式下前端由 Vite 提供（`http://localhost:5173`）。先 `npm --prefix ui run dev`，
或在 `dist/` 里跑打包版。

**`cargo build` 报 `linking with 'link.exe' failed`。**
这台机器没有 Windows SDK，而且 Git Bash 的 `link` 会冒充 MSVC 的链接器。
仓库里的 `rust-toolchain.toml` + `.cargo/config.toml` 已经把构建固定到 GNU 工具链；
如果你从别处调用 cargo，请在工作区根目录执行，或
`export PATH="$PATH:/c/Users/EDY/tools/mingw64/bin"`。

## 端口

桌面 App 会依次尝试 `8899…8908`。端口全被占时 HTTP 不启动，但**窗口和所有功能照常**——
此时 `caps.port` 返回 0，界面上的接口状态灯会变灰，这是真实状态而不是坏了。
手动指定：`agentcanvas http --port 9001`。

## 工程与素材

**素材显示"缺失"（`ASSET_MISSING`）。**
工程目录里的 `assets/` 被移动或删掉了。用 `asset.replace {p,id,path}` 重新指一份，
图层引用不会断（id 不变）。批量看：`asset.missing {p}`。

**字体显示回退（`missing-font` / `tofu-risk`）。**
文档里记的是字体**名字**，不是字体文件。换机器后如果没装同名字体会回退。
两条路：装那个字体，或 `text.register {path:"C:\\Fonts\\X.ttf"}` 临时注册。
`agentcanvas doctor` 的 `cjk_font` 字段能提前告诉你这台机器有没有中文字体。

**工程打不开（`INVALID_SCHEMA`）。**
`project.json` 手改坏了。`versions/` 里有快照：
`agentcanvas project.restore p=<id> snap=<snap>`，或回退到上一次自动保存
（`history.list` 能看到每一步是谁做的）。

**崩溃后。**
所有写入都是"临时文件 + rename"，所以磁盘上不会留半个 `project.json`。
重开工程即可；`.tmp` 残留文件可以直接删。

## 排版

**自动排版把我手工调好的位置改回去了？**
不会——手工拖动会把节点标成 `manual`/`pinned`，求解器跳过它。
如果你确实想让它重新参与排版，在右侧"布局"里把模式切回 `constraint`，或
`layout.lock {pinned:false}`。

**`layout.resolve` 返回 `LAYOUT_CONFLICT`。**
`details.cycle` 里是成环的节点（例如 A `below` B 且 B `below` A）。
删掉一条约束即可。求解器不会因此产生 NaN 坐标。

**导出图和预览位置差一点。**
先确认不是缩放取整：用 `sc=1` 导出再比。若仍有差异，以 `render.export` 的文件为准——
浏览器和 resvg 在极端字距下有亚像素差异，这是当前实现的诚实边界。

## 智能体侧

**`REVISION_CONFLICT`。**
有人（可能是你自己）在你读取之后改过。用响应里的 `details.rev` 重新提交，
或干脆不传 `rev`（表示"我知道我在覆盖"）。

**`PATH_DENIED`。**
写操作默认沙箱在工作区根目录内。用相对路径，或 `--root D:\work`，
或显式 `AGENTCANVAS_OPEN_FS=1`（只放开读，写仍然要落在根内）。

**`PROVIDER_NOT_CONFIGURED`。**
见 `docs/provider-setup.md`。确定性替代方案在 `hint` 里。

**MCP 客户端连不上。**
`tools/list` 之前必须先 `initialize`，再发 `notifications/initialized`。
stdio 是**换行分隔**的 JSON-RPC，不要按 HTTP 分块读。
排查时直接：`echo {"jsonrpc":"2.0","id":1,"method":"tools/list"} | agentcanvas mcp`。

## 性能

**大图卡顿。** 上限是 60 MP / 200 MB，超了会 `TOO_LARGE`。
预览用 `render.preview {sc:0.5}`，别反复 `project.get` 整个文档——
`inspect.document` 或 `fields` 参数便宜得多。

**批量任务想停。** `jobs` 看进度，`cancel {id}` 会设置取消标志，
协作式检查点会尽快退出并返回 `CANCELLED`（已经写完的文件不会回滚）。

## 卸载

删掉两处即可：程序目录，和 `%USERPROFILE%\AgentCanvas\`（工程、日志、缓存、模板）。
没有注册表项、没有服务、没有后台驻留进程。
