# 文档 Schema

> 由 `node tools/gen-docs.mjs` 从引擎的 `doc.rs` 类型生成。
> schema 版本 **1.1.0**，引擎 1.0.0。

一个工程就是一份 `Document`：

```json
{
  "artboards": [],
  "assets": [],
  "meta": {},
  "name": "Untitled",
  "nodes": [],
  "project_id": "",
  "revision": 0,
  "schema_version": "1.1.0",
  "tokens": {}
}
```

约定：

- 坐标是 **CSS 像素**，原点在画板左上角，y 向下；`rotation` 是角度、顺时针、绕 `origin`。
- `nodes[].box` 是**求解结果**，`nodes[].layout.intent` 是**作者意图**，两者分开存。
- `layers` 与 `children` 的索引 0 是**最底层**。
- 颜色支持 `#rgb` `#rgba` `#rrggbb` `#rrggbbaa` `rgb()` `rgba()` 和 `$token`。
- `layout.mode` ∈ `manual` `constraint` `flex` `grid`；`manual` 或 `pinned:true` 的节点不参与自动排版。
- 未知字段会被拒绝（不是忽略），这样拼写错误会立刻暴露而不是变成"设置没生效"。

## 节点类型

`text`, `image`, `rect`, `ellipse`, `line`, `polygon`, `path`, `group`, `frame`, `svg`

## 命令

每一次修改都是一个命令（`canvas.batch` 一次提交多条，一个事务）：

```json
[
 {
  "t": "NodeAdd"
 },
 {
  "t": "NodeDel"
 },
 {
  "t": "NodeSet"
 },
 {
  "t": "NodesSet"
 },
 {
  "t": "NodeMove"
 },
 {
  "t": "BoardAdd"
 },
 {
  "t": "BoardDel"
 },
 {
  "t": "BoardSet"
 },
 {
  "t": "AssetAdd"
 },
 {
  "t": "AssetDel"
 },
 {
  "t": "TokenSet"
 },
 {
  "t": "Text"
 },
 {
  "t": "Transform"
 },
 {
  "t": "LayoutIntent"
 },
 {
  "t": "Style"
 },
 {
  "t": "Group"
 },
 {
  "t": "Ungroup"
 },
 {
  "t": "Duplicate"
 },
 {
  "t": "Reflow"
 },
 {
  "t": "Solve"
 },
 {
  "t": "Composite"
 }
]
```

## 错误码

| code | 含义 |
| --- | --- |
| `INVALID_SCHEMA` | argument missing or of the wrong shape |
| `INVALID_VALUE` | argument present but out of range |
| `PROJECT_NOT_FOUND` | no project with that id |
| `ARTBOARD_NOT_FOUND` | no artboard with that id |
| `NODE_NOT_FOUND` | no node with that id |
| `ASSET_NOT_FOUND` | asset id or path unresolved |
| `ASSET_MISSING` | asset file is gone from disk |
| `FONT_MISSING` | requested font is not installed |
| `LAYOUT_CONFLICT` | constraints cannot all be satisfied |
| `LOCKED` | node is locked and was not modified |
| `REVISION_CONFLICT` | expectedRevision is stale, re-read then retry |
| `PATH_DENIED` | path is outside the allowed root |
| `PERMISSION_DENIED` | operation not allowed |
| `UNSUPPORTED` | format or capability not available in this build |
| `RENDER_FAILED` | rasteriser failed |
| `DECODE_FAILED` | image bytes could not be decoded |
| `TOO_LARGE` | input exceeds configured limits |
| `JOB_NOT_FOUND` | no such job id |
| `JOB_TIMEOUT` | job exceeded its time budget |
| `CANCELLED` | job was cancelled |
| `PROVIDER_NOT_CONFIGURED` | an AI provider is required but none is configured |
| `IO` | filesystem error |
| `NOT_FOUND` | nothing at that path |
| `INTERNAL` | unexpected engine error |

## 迁移

`schema_version` 是语义化版本。读入时：

- 同 minor 直接读；
- 低版本走 `doc::migrate`（读旧写新，不丢字段）；
- 高版本拒绝打开并返回 `INVALID_SCHEMA` + 引擎版本，避免旧引擎写坏新工程。
