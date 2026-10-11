# Provider 配置（外部模型能力）

产品里所有**确定性**能力——排版、约束求解、图形渲染、图片处理、导出、检查——
不需要任何密钥。只有这几类能力需要模型，并且都做成可插拔 Provider：

| 能力 | 方法 | 没配 Provider 时 |
| --- | --- | --- |
| AI 抠图 | `image.op op:remove_bg args:{mode:"ai"}` | 返回 `PROVIDER_NOT_CONFIGURED`，`hint` 指向确定性的 `mode:"chroma"/"edge"` |
| 智能扩图 | `image.op op:extend args:{mode:"ai"}` | 返回 `PROVIDER_NOT_CONFIGURED`，替代是 `blurpad` / `pad` |
| 以图生图 | `image.generate` | 同上；工程与排版功能完全不受影响 |
| 审美复核 | `inspect.review` | 退化为 `inspect.validate` 的结构化报告（不含主观判断） |
| 内置 Agent 自由对话 | Agent 面板 | 面板走确定性模板生成，并如实列出它调用了哪些工具 |

**返回 `PROVIDER_NOT_CONFIGURED` 不是失败降级，是产品契约**：界面和 CLI 都不会把
"没做"显示成"做了"。

## 配置

密钥**不写进仓库**，也不回显。两种放法：

```powershell
# 密钥只放在环境变量里；引擎默认读 AGENTCANVAS_API_KEY，也可以改名
setx AGENTCANVAS_API_KEY "<your key>"
setx AGENTCANVAS_PROVIDER_BASE_URL "http://127.0.0.1:11434"
```

其余配置走 `system.config`，可写的 provider 键只有四个：`name`、`model`、`baseUrl`、
`apiKeyEnv`。任何读取返回的都是"是否已设置"，不是值：

```json
{"m":"system.config","p":{"set":{"provider":{"name":"ollama","baseUrl":"http://127.0.0.1:11434","model":"llama3","apiKeyEnv":"AGENTCANVAS_API_KEY"}}}}
{"m":"system.config","p":{"set":{"provider":{"apiKey":"sk-..."}}}}
```

第二条会被拒绝（`INVALID_VALUE: unknown provider setting "apiKey"`）。读回来的形状：

```json
{"provider":{"name":"ollama","model":"llama3","baseUrl":"http://127.0.0.1:11434","apiKeyEnv":"AGENTCANVAS_API_KEY","baseUrlSet":true,"apiKeySet":true},"tokenSet":false}
```

## 探测端点（不需要模型也能验证的一格）

```
agentcanvas system.provider.probe                        # 用已保存的 baseUrl
agentcanvas system.provider.probe url=http://127.0.0.1:11434 timeout=2000
```

返回 `{ok,url,status,ms,auth,n_models,models[]}`；没配置给
`PROVIDER_NOT_CONFIGURED`，连不上给 `NETWORK`（带 `status`/`ms` 的 details），
非 http scheme 给 `INVALID_VALUE`。它按地址形状选路由：Ollama 走 `/api/tags`，
带 `/v1` 前缀的走 `/models`，其余走 `/v1/models`。

桌面 App 的设置浮层里这就是「测试连接」按钮；不配 provider 不影响任何确定性能力。

**当前构建的边界**：probe 只证明端点在、能答、有哪些模型；把 prompt/图发出去
（`remove_bg mode:ai`、`image.generate`、`inspect.review`）仍然要求接入方自带模型客户端，
未接入时如实返回 `PROVIDER_NOT_CONFIGURED`，不会把"没做"显示成"做了"。

## 接什么模型

任何**支持工具调用**的 OpenAI 兼容 / Anthropic 兼容端点都可以：内置 Agent 只需要
`messages + tools`，它拿到的工具清单和外部 MCP 客户端拿到的是同一份
（`system.tools`，约 40 个聚合工具）。

- 有视觉能力：Agent 会把 `render.preview` 的 PNG 回传给模型做复核；
- 纯文本模型：改用 `inspect.validate` 的结构化报告，不会假装"看过了"。

## 接入自检

```powershell
agentcanvas doctor      # 看 provider 是否 detected、字体/端口/沙箱状态
agentcanvas caps        # 看哪些能力在当前配置下可用
```

`doctor` 的 `warnings` 会明确列出"AI 抠图不可用，将使用色度键"这类事实。
