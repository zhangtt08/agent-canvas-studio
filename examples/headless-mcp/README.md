# 无界面调用

见 `tests/mcp-e2e.mjs`：它不启动任何窗口，只用 MCP stdio 建工程、排版、检查、导出。

```powershell
agentcanvas mcp    # stdio 服务
node tests/mcp-e2e.mjs
```
