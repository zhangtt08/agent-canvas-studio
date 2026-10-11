# MCP 接入

## Claude Desktop / Claude Code

```json
{
  "mcpServers": {
    "agentcanvas": {
      "command": "C:\\Users\\<you>\\Documents\\agentcanvas\\dist\\bin\\agentcanvas.exe",
      "args": ["mcp"],
      "env": { "AGENTCANVAS_HOME": "C:\\Users\\<you>\\AgentCanvas" }
    }
  }
}
```

`agentcanvas caps` 的 `paths.exe` 返回本机真实路径，直接复制即可。

## 任意支持工具调用的客户端

传输是 **stdio 上的换行分隔 JSON-RPC 2.0**。握手：

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"my-agent","version":"1"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"poster","arguments":{"title":"新品上市","image":"C:\\pics\\a.png","preset":"poster"}}}
```

实现的方法：`initialize` `ping` `tools/list` `tools/call` `resources/list` `resources/read`
`prompts/list` `prompts/get`，以及 `notifications/*`。

## 工具粒度是刻意做的

`tools/list` 只返回约 40 个粗粒度工具（`project_*`、`canvas_*`、`layout_*`、`inspect_*`、
`render_*`、`template_*`、`image_*`、`poster`、`batch`、`agentcanvas`）。
每个工具的 schema 都要进模型上下文，把 100 个细方法摊出去会让每次请求多花几 KB。

需要细粒度时走通用工具：

```json
{"name":"agentcanvas","arguments":{"m":"layout.explain","p":{"p":"proj_x","id":"n_img"}}}
```

`m` 可以是 `docs/METHODS.md` 里的**任何**方法，所以工具列表不需要为了覆盖率而膨胀。

## 资源

| URI | 内容 |
| --- | --- |
| `agentcanvas://caps` | 全部自描述（方法、op、intent、限额、错误码） |
| `agentcanvas://project/<id>` | 该工程的完整文档 JSON |

## 返回值

`tools/call` 同时给出：
- `content[0].text` —— 紧凑单行 JSON（智能体读这个）；
- `structuredContent` —— 同一个对象的结构化版本（支持 schema 的客户端读这个）；
- 当调用产出了 PNG（`render.preview`、`poster`、`render.export`）时，
  `content` 里额外带一个 `type:"image"` 项 —— 视觉模型可以直接看自己排出来的结果。

失败时 `isError:true`，文本仍是 `{e, code, hint?, details?, retryable?}`。

## 建议的智能体回路

```
caps → poster | project.create → asset.import → canvas.add(+intent)
     → layout.resolve → inspect.validate → inspect.fix(dry=1)
     → render.preview → image.compare → render.export
```

带 `rev` 写、失败看 `code`、改完自己 `render.preview` + `image.compare` 复核——
这条回路是端到端测试 `tests/mcp-e2e.mjs` 逐条断言过的。

## 无界面运行

MCP 服务不依赖桌面 App、不依赖浏览器、不依赖模型密钥。
`agentcanvas mcp` 是独立进程；桌面 App 关掉之后它照样能建工程、排版、导出真文件。
