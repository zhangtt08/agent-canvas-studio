# AgentCanvas internal module contract

This file is the coordination contract between modules. Signatures listed here are
**fixed**; anything unlisted is the module author's choice. All modules live in
`core/src/`. Rust 2021, no `unsafe`, no `anyhow` — errors are `crate::error::Fail`.

Common aliases: `use crate::error::{Fail, R};` where `R<T> = Result<T, Fail>`.

## Layering (a module may only depend on modules above it)

```
layer 0  error, util, geom, doc
layer 1  text, assets, imageops, session, jobs
layer 2  layout, svg, pdf
layer 3  render, inspect, templates, project, history, commands
layer 4  rpc
layer 5  http            (and cli/, app/ crates)
```

## text.rs — font registry + measurement + wrapping

```rust
pub struct FontInfo { pub family: String, pub style: String, pub weight: u16,
                     pub postscript: String, pub path: Option<String>, pub index: u32 }
pub struct Registry { /* fontdb::Database + fallback list, built once per process */ }
impl Registry {
    pub fn shared() -> &'static Registry;              // OnceLock, loads system fonts
    pub fn families(&self) -> Vec<String>;              // sorted, deduped
    pub fn fonts(&self) -> Vec<FontInfo>;
    pub fn register_file(&self, path: &Path) -> R<String>;   // returns family; idempotent
    pub fn register_bytes(&self, bytes: Vec<u8>) -> R<String>;
    /// Resolve a family+weight to the bytes of the chosen face, for @font-face serving.
    pub fn face_bytes(&self, family: &str, weight: u16, italic: bool) -> Option<Vec<u8>>;
    pub fn has(&self, family: &str) -> bool;
    /// First family from `want` (comma list) that exists, else the default.
    pub fn resolve(&self, want: &str) -> String;
    /// Advance width of `s` at `size` px with `letter_spacing`. Deterministic.
    pub fn measure(&self, s: &str, family: &str, weight: u16, italic: bool,
                   size: f64, letter_spacing: f64) -> f64;
    /// True when the face has no glyph for any char (tofu risk).
    pub fn covers(&self, s: &str, family: &str, weight: u16) -> bool;
}
pub struct MeasureOpts { pub family: String, pub weight: u16, pub italic: bool,
                          pub size: f64, pub ls: f64, pub line_h: f64, pub max_w: f64,
                          pub align: String, pub valign: String }
pub struct TextMetrics { pub lines: Vec<String>, pub width: f64, pub height: f64,
                         pub line_height: f64, pub ascender: f64, pub descender: f64,
                         pub overflow: bool, pub truncated: bool, pub font: String,
                         pub fell_back: bool }
/// Wrap with CJK-aware break opportunities (no space needed between Han), plus
/// `\n` hard breaks, and shrink-to-fit when `overflow == "shrink"`.
pub fn layout_text(s: &str, m: &MeasureOpts, mode: &str) -> TextMetrics;
pub fn escape_xml(s: &str) -> String;
```

## layout.rs — semantic constraint solver

```rust
pub struct SolveReport { pub changed: Vec<String>, pub conflicts: Vec<Conflict>,
                         pub passes: u32 }
pub struct Conflict { pub node: String, pub intent: String, pub with: Option<String>,
                      pub why: String }
/// Solve one artboard (or the whole doc when `board == None`). Deterministic and
/// incremental: `mode == manual` or `pinned == Some(true)` nodes are never moved.
pub fn solve(doc: &mut Document, board: Option<&str>) -> R<SolveReport>;
/// Align / distribute selected ids inside their artboard or a common parent.
pub fn align(doc: &mut Document, ids: &[String], axis: &str, how: &str) -> R<Vec<String>>;
pub fn distribute(doc: &mut Document, ids: &[String], axis: &str) -> R<Vec<String>>;
/// Why is this node where it is? Returns intent keys in application order.
pub fn explain(doc: &Document, id: &str) -> R<Value>;
/// Reflow a board to a new size, scaling intents (not just pixels).
pub fn reflow(doc: &mut Document, board: &str, w: f64, h: f64, mode: &str) -> R<SolveReport>;
```

Application order inside `solve` (documented, tested): `size` → `anchor`/`margin*` →
`below|above|left_of|right_of` → `align_to` → `align_x|align_y` → `center_in` →
`safe_margin` → `distribute|equal_gap` → flex/grid container pass. Cycles are
reported as `LAYOUT_CONFLICT` with the node ids on the cycle, never as NaN boxes.

## svg.rs — the one paint path

```rust
pub struct SvgOpts {
    pub boards: Vec<String>,      // which artboards to emit; empty = all visible
    pub inline_assets: bool,      // base64 data: URIs (export) vs /asset/<id> (preview)
    pub asset_url: Option<String>,// base URL used in preview mode
    pub font_url: Option<String>, // base URL for @font-face in preview mode
    pub embed_fonts: bool,        // true for standalone .svg export
    pub show_guides: bool,
    pub omit_ids: Vec<String>,    // hidden / excluded layers
    pub scale: f64,
}
/// Compile a document to a standalone, deterministic SVG string. The desktop
/// preview and every raster export come from this exact function.
pub fn compile(doc: &Document, o: &SvgOpts) -> R<String>;
/// One artboard only, as a fragment (used by `render.preview` of a single board).
pub fn compile_board(doc: &Document, board: &str, o: &SvgOpts) -> R<String>;
pub fn board_size(doc: &Document, o: &SvgOpts) -> (f64, f64);
```

Rules: z-order = `paint_order()`; rotation via `transform="rotate(a cx cy)"`;
rounded rects via `rx` or a path when radii differ; text is emitted as one
`<text>` per solved line with explicit `x`/`y` (never relying on the renderer's
own wrapping); effects become `<filter>` defs; `<defs>` ids are namespaced per
board so multi-board documents stay valid.

## render.rs — raster + format export

```rust
pub struct RenderOpts { pub scale: f64, pub format: Format, pub quality: u8,
                        pub background: Option<String>, pub width: Option<u32>,
                        pub height: Option<u32> }
pub enum Format { Png, Jpeg, Webp, Svg, Pdf }
pub struct Rendered { pub path: Option<PathBuf>, pub bytes: u64, pub width: u32,
                      pub height: u32, pub sha256: String, pub format: &'static str,
                      pub mime: &'static str }
/// SVG string -> RGBA pixmap (resvg + the shared font registry).
pub fn raster(svg: &str, scale: f64) -> R<(u32, u32, Vec<u8>)>;
pub fn encode_png(w: u32, h: u32, rgba: &[u8]) -> R<Vec<u8>>;
pub fn encode(rgba: &[u8], w: u32, h: u32, f: Format, q: u8) -> R<Vec<u8>>;
/// Render a document and write it to disk; returns the artifact summary.
pub fn export(doc: &Document, dir: &Path, name: &str, o: &RenderOpts, boards: &[String]) -> R<Vec<Rendered>>;
pub fn to_bytes(doc: &Document, o: &RenderOpts) -> R<(Vec<u8>, Rendered)>;
/// Perceptual-ish diff of two images: mean abs error per channel + score 0..100.
pub fn diff(a: &Path, b: &Path) -> R<Value>;
pub fn load_rgba(p: &Path) -> R<(u32, u32, Vec<u8>)>;
```

## pdf.rs — minimal, honest PDF writer

```rust
/// One page per board, RGB raster embedded as FlateDecode. Real PDF 1.4, opens in
/// any viewer; `meta` is written into the document info dictionary.
pub fn write_pdf(pages: &[(u32, u32, Vec<u8>)], meta: &Value) -> R<Vec<u8>>;
```

## imageops.rs — raster image processing (the "图片处理" half)

```rust
pub struct Img { pub w: u32, pub h: u32, pub px: Vec<u8> }  // RGBA8, always
pub fn load(p: &Path) -> R<Img>;
pub fn save(img: &Img, p: &Path, q: u8) -> R<()>;           // format from extension
pub fn probe(p: &Path) -> R<Value>;
pub fn op(name: &str, img: Img, args: &Value) -> R<Img>;    // crop|resize|scale|pad|
                                                             // rotate|flip|round|border|
                                                             // adjust|blur|sharpen|noise|
                                                             // gray|invert|threshold|tint|
                                                             // watermark|overlay|grid|
                                                             // background|remove_bg|
                                                             // palette|dhash|strip
pub fn ops() -> Vec<(&'static str, &'static str)>;           // name + one-line description
```

`remove_bg` is deterministic (border-colour / chroma keyed) and says so; the AI
variant is a provider and returns `PROVIDER_NOT_CONFIGURED` when absent.

## assets.rs — import, dedupe, thumbnails, indexing

```rust
pub struct Imported { pub id: String, pub uri: String, pub name: String, pub mime: String,
                      pub w: f64, pub h: f64, pub bytes: u64, pub sha256: String,
                      pub alpha: bool, pub dup: Option<String>, pub palette: Vec<String> }
/// Copy (or reference) into `<project>/assets/`, deduped by sha256.
pub fn import(doc: &mut Document, dir: &Path, src: &Path, opts: &Value) -> R<Imported>;
pub fn thumb(doc: &Document, dir: &Path, asset_id: &str, w: u32) -> R<PathBuf>;
pub fn inspect(doc: &Document, dir: &Path, id: &str) -> R<Value>;
pub fn missing(doc: &Document, dir: &Path) -> R<Vec<Value>>;
pub fn replace(doc: &mut Document, dir: &Path, id: &str, src: &Path) -> R<Imported>;
```

## project.rs — bundles, autosave, recents

```rust
pub struct Project { pub dir: PathBuf, pub doc: Document, pub path: PathBuf }
pub fn create(root: &Path, name: &str, w: f64, h: f64, opts: &Value) -> R<Project>;
pub fn open(root: &Path, id_or_path: &str) -> R<Project>;
pub fn save(p: &mut Project, opts: &Value) -> R<Value>;       // atomic, bumps revision
pub fn close(root: &Path, id: &str) -> R<()>;
pub fn list(root: &Path) -> R<Vec<Value>>;
pub fn recents(root: &Path) -> R<Vec<Value>>;
pub fn snapshot(p: &Project) -> R<String>;                    // version id
pub fn restore(root: &Path, id: &str, snap: &str) -> R<Project>;
pub fn export_bundle(p: &Project, out: &Path) -> R<Value>;    // .acanvas zip-less folder/tar
pub fn import_bundle(root: &Path, src: &Path) -> R<Project>;
pub fn ensure_open(root: &Path, p: &Value) -> R<Project>;     // from {p|project|id}
```

Open projects live in a process-global `Mutex<HashMap<String, Project>>` so the
desktop window and an HTTP request edit the *same* document. `session::with_project`.

## session.rs / jobs.rs

```rust
// session.rs
pub fn workspace() -> PathBuf;         // %USERPROFILE%\AgentCanvas
pub fn log_call(entry: &Value);        // jsonl, ring-capped
pub fn read_log(n: usize) -> Vec<Value>;
pub fn config(root: &Path) -> Value;   // read settings.json
pub fn set_config(root: &Path, patch: &Value) -> R<Value>;
pub fn with_project<T>(id: &str, f: impl FnOnce(&mut Project) -> R<T>) -> R<T>;
// jobs.rs
pub fn start(kind: &str, label: &str) -> String;               // job id
pub struct Handle { /* id, progress + cancel flags */ }
impl Handle { pub fn progress(&self, done: f64, stage: &str);
              pub fn cancelled(&self) -> bool;
              pub fn log(&self, msg: &str); }
pub fn finish(id: &str, ok: bool, result: Value);
pub fn snapshot() -> Vec<Value>;
pub fn cancel(id: &str) -> bool;
pub fn run<T: Send + 'static>(label: &str, f: impl FnOnce(&Handle) -> R<Value> + Send + 'static) -> R<Value>;
```

## history.rs / commands.rs — reversible edits

```rust
// commands.rs — every mutation in the product is one of these, from UI or API.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "t")]
pub enum Cmd {
    NodeAdd { node: Node, board: String, at: Option<usize> },
    NodeDel { id: String },
    NodeSet { id: String, patch: Value },         // deep merge, `null` deletes a key
    NodesSet { ids: Vec<String>, patch: Value },
    NodeMove { id: String, board: String, to: usize, parent: Option<String> },
    BoardAdd { board: Artboard }, BoardDel { id: String }, BoardSet { id: String, patch: Value },
    AssetAdd { asset: Asset }, AssetDel { id: String },
    TokenSet { group: String, name: String, value: Value },
    Text { id: String, content: String },
    Transform { id: String, x: f64, y: f64, w: f64, h: f64 },
    LayoutIntent { id: String, intent: Value, mode: String },
    Style { id: String, style: Value },
    Group { ids: Vec<String>, board: String }, Ungroup { id: String },
    Duplicate { ids: Vec<String> },
    Reflow { board: String, w: f64, h: f64, mode: String },
    Solve { board: Option<String> },
    Composite { cmds: Vec<Cmd> },                 // one transaction, all-or-nothing
}
pub fn apply(doc: &mut Document, cmd: &Cmd, ctx: &ApplyCtx) -> R<Applied>;
pub struct Applied { pub changed: Vec<String>, pub undo: Cmd, pub warnings: Vec<String>,
                     pub created: Vec<String>, pub revision: u64 }
pub struct ApplyCtx { pub dir: PathBuf, pub force: bool, pub origin: String }

// history.rs
pub fn undo(root: &Path, p: &Value) -> R<Value>;
pub fn redo(root: &Path, p: &Value) -> R<Value>;
pub fn checkpoint(root: &Path, p: &Value) -> R<Value>;
pub fn compare(root: &Path, a: &str, b: &str) -> R<Value>;   // structural diff
pub fn rollback(root: &Path, p: &Value) -> R<Value>;
pub fn history(root: &Path, n: usize) -> Vec<Value>;
```

Locked nodes are never mutated by a command: `apply` returns `LOCKED` with the node
id unless `ctx.force` is set. A `Composite` rolls the document back to its pre-state
if any member fails.

## inspect.rs — machine-readable quality report

```rust
pub struct Issue { pub sev: &'static str, pub code: &'static str, pub node: Option<String>,
                   pub board: Option<String>, pub msg: String, pub evidence: Value,
                   pub fix: Option<Value> }
pub fn validate(doc: &Document, dir: &Path, opts: &Value) -> R<Value>;
// codes: offboard, overflow, text-overflow, zero-size, missing-asset, missing-font,
// bad-color, invisible, low-res, contrast, safe-area, conflict, dup-id, unlinked,
// empty-text, tiny-target
pub fn geometry(doc: &Document, ids: &[String]) -> Vec<Value>;
pub fn text_metrics(doc: &Document, id: &str) -> R<Value>;
pub fn conflicts(doc: &Document, board: &str) -> Vec<Value>;
pub fn asset_usage(doc: &Document) -> Vec<Value>;
pub fn diff_docs(a: &Document, b: &Document) -> Value;
pub fn suggest_fixes(issues: &[Issue]) -> Vec<Cmd>;
```

## templates.rs — tokens, templates, variants, batch

```rust
pub fn save_from_board(doc: &Document, dir: &Path, board: &str, name: &str) -> R<Value>;
pub fn list(dir: &Path) -> R<Vec<Value>>;
pub fn instantiate(dir: &Path, tpl: &str, into: &mut Document, opts: &Value) -> R<Vec<String>>;
pub fn apply_variables(doc: &mut Document, vars: &Value) -> R<Vec<String>>;
pub fn create_variants(doc: &mut Document, opts: &Value) -> R<Value>;  // {boards, ids}
pub fn builtin() -> Vec<(&'static str, &'static str)>;                        // name + description
pub fn apply_builtin(doc: &mut Document, dir: &Path, name: &str) -> R<Vec<String>>;
```

## rpc.rs — the single tool surface

`pub fn call(method: &str, params: &Value, ctx: &Ctx) -> R<Value>` with
`pub struct Ctx { pub root: Option<PathBuf>, pub origin: String }`.
Also `pub fn mcp_tools() -> Value`, `pub fn caps() -> R<Value>`,
`pub fn cheatsheet() -> Vec<String>`, `pub fn doctor() -> R<Value>`,
`pub fn methods() -> Vec<(&'static str, &'static str)>`.

Success responses are **compact JSON** with short keys; failures are
`{e, code, hint?, details?, retryable?}`. Every mutating call accepts
`{p: <project>, rev: <expectedRevision>}` and returns `{rev, ok, changed}`.

## http.rs

```rust
pub const DEFAULT_PORT: u16 = 8899;
pub fn start(port: u16, root: Option<PathBuf>) -> R<u16>;
pub fn port() -> u16;
```
Routes: `POST /rpc` `{m,p}`; `GET /rpc?m=&p=`; `POST /v1/projects`,
`POST /v1/projects/:id/commands:batch`, `GET /v1/projects/:id`,
`GET /health`, `GET /caps`, `GET /tools` (MCP tool list), `GET /asset/<id>` and
`GET /font/<family>` for the webview, `GET /file/<rel>` for previews. Loopback only.
