/** Mirrors `core/src/doc.rs` / `core/src/geom.rs` serde shapes.
 *  Field names are exactly what the engine writes: `box` is the serialised name
 *  of `Node::box`, enums are lowercase, `Origin` is kebab-case. */

export type V = Record<string, unknown>;

/* ------------------------------------------------------------------ geom */

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export type Origin =
  | 'top-left'
  | 'top'
  | 'top-right'
  | 'left'
  | 'center'
  | 'right'
  | 'bottom-left'
  | 'bottom'
  | 'bottom-right';

export type AlignX = 'left' | 'center' | 'right';
export type AlignY = 'top' | 'middle' | 'bottom';
export type Fit = 'contain' | 'cover' | 'fill' | 'actual';

/* ------------------------------------------------------------------ doc */

export type NodeType =
  | 'text'
  | 'image'
  | 'rect'
  | 'ellipse'
  | 'line'
  | 'polygon'
  | 'path'
  | 'group'
  | 'frame'
  | 'svg';

export type LayoutMode = 'manual' | 'constraint' | 'flex' | 'grid';

export interface Stop {
  at: number;
  color: string;
}

export type Paint =
  | { t: 'none' }
  | { t: 'solid'; color: string }
  | { t: 'linear'; stops: Stop[]; angle: number }
  | { t: 'radial'; stops: Stop[]; cx: number; cy: number; r: number }
  | { t: 'image'; asset: string; fit: Fit; opacity: number };

export interface Stroke {
  color: string;
  width: number;
  align?: 'inside' | 'center' | 'outside';
  dash?: number[];
  cap?: string;
}

export interface Radius {
  all: number;
  tl?: number;
  tr?: number;
  br?: number;
  bl?: number;
}

export interface Shadow {
  x: number;
  y: number;
  blur: number;
  color: string;
  inset?: boolean;
  spread?: number;
}

export interface Effects {
  shadows?: Shadow[];
  blur?: number;
  backdrop_blur?: number;
  saturate?: number;
  noise?: number;
  inner_glow?: string;
}

export interface Style {
  fill?: Paint;
  stroke?: Stroke;
  radius?: Radius;
  color?: string;
}

export interface TextSpec {
  content: string;
  family?: string;
  fallback?: string;
  size?: number;
  weight?: number;
  style_italic?: boolean;
  color?: string;
  line_height?: number;
  line_ratio?: number;
  letter_spacing?: number;
  align?: string;
  valign?: string;
  /** The engine writes `box_mode` on the wire (see `doc::TextSpec`). */
  box_mode?: string;
  overflow?: string;
  max_font_size?: number;
  min_font_size?: number;
  uppercase?: boolean;
  rotate_each?: number;
  lines?: string[];
  font_ref?: string;
  paint?: Paint;
  stroke?: Stroke;
  bg?: Paint;
  padding?: number;
}

export interface ImageSpec {
  asset: string;
  fit: Fit;
  crop?: Rect;
  brightness?: number;
  contrast?: number;
  saturate?: number;
  hue?: number;
  blur?: number;
  grayscale?: boolean;
  tint?: string;
  position?: string;
  opacity?: number;
  tile?: number;
}

export interface VectorSpec {
  d?: string;
  sides?: number;
  points?: number[][];
  asset?: string;
  inline?: string;
  viewBox?: number[];
  line_end?: number[];
}

export interface Clip {
  kind: string;
  by?: string;
  rect?: Rect;
  radius?: number;
}

export interface Rel {
  of: string;
  gap?: number;
  cross?: string;
}

export interface AlignTo {
  of: string;
  edge: string;
  offset?: number;
}

export interface SizeIntent {
  w?: number;
  h?: number;
  w_ref?: number;
  h_ref?: number;
  ratio?: number;
  mode?: 'fixed' | 'fill' | 'hug' | 'min';
  max_w?: number;
  max_h?: number;
  min_w?: number;
  min_h?: number;
}

export interface EqualGap {
  axis: string;
  between?: string[];
  gap?: number;
}

export interface FlexIntent {
  direction?: string;
  gap?: number;
  row_gap?: number;
  padding?: number;
  pad_top?: number;
  pad_right?: number;
  pad_bottom?: number;
  pad_left?: number;
  justify?: string;
  align?: string;
  wrap?: boolean;
  sizing?: string;
}

export interface GridIntent {
  cols?: number;
  rows?: number;
  gap?: number;
  padding?: number;
}

export interface Intent {
  anchor?: Origin;
  margin_top?: number;
  margin_right?: number;
  margin_bottom?: number;
  margin_left?: number;
  margin?: number;
  above?: Rel;
  below?: Rel;
  left_of?: Rel;
  right_of?: Rel;
  center_in?: string;
  align_x?: AlignX;
  align_y?: AlignY;
  align_to?: AlignTo;
  size?: SizeIntent;
  fit?: Fit;
  safe_margin?: number;
  distribute?: string[];
  equal_gap?: EqualGap;
  keep_aspect_ratio?: boolean;
  flex?: FlexIntent;
  grid?: GridIntent;
  priority?: number;
}

export interface Layout {
  mode: LayoutMode;
  intent?: Intent;
  provenance?: string[];
  pinned?: boolean;
}

export interface Node {
  id: string;
  type: NodeType;
  name: string;
  artboard?: string;
  parent?: string;
  children?: string[];
  box: Rect;
  rotation: number;
  origin: Origin;
  flip_x: boolean;
  flip_y: boolean;
  visible: boolean;
  locked: boolean;
  opacity: number;
  blend?: string;
  role?: string;
  z_role?: string;
  layout: Layout;
  style?: Style;
  text?: TextSpec;
  image?: ImageSpec;
  vector?: VectorSpec;
  clip?: Clip;
  fx?: Effects;
  data?: V;
}

export interface Guide {
  axis: 'v' | 'h';
  pos: number;
  label?: string;
}

export interface Artboard {
  id: string;
  name: string;
  width: number;
  height: number;
  background?: Paint;
  visible: boolean;
  layers: string[];
  safe_margin?: number;
  guides?: Guide[];
  ratio?: string;
  role?: string;
  template_id?: string;
  vars?: V;
}

export interface Asset {
  id: string;
  name: string;
  uri: string;
  mime?: string;
  width: number;
  height: number;
  bytes: number;
  sha256?: string;
  alpha?: boolean;
  kind?: string;
  source?: string;
  tags?: string[];
  thumb?: string;
  palette?: string[];
  orientation?: string;
}

export interface ColorToken {
  name: string;
  value: string;
  note?: string;
}
export interface NumToken {
  name: string;
  value: number;
}
export interface FontToken {
  name: string;
  family: string;
  weight?: number;
  size?: number;
  line_height?: number;
  letter_spacing?: number;
}
export interface Tokens {
  colors?: ColorToken[];
  fonts?: FontToken[];
  spacing?: NumToken[];
  radii?: NumToken[];
}

export interface Meta {
  created_at?: string;
  updated_at?: string;
  author?: string;
  origin?: string;
  description?: string;
  dirty?: boolean;
}

export interface Document {
  schema_version: string;
  project_id: string;
  revision: number;
  name: string;
  tokens: Tokens;
  assets: Asset[];
  artboards: Artboard[];
  nodes: Node[];
  meta: Meta;
}

/* ------------------------------------------------------------ RPC shapes */

/** `project.get` / `project.create` summary (`depth:0`, and every create/open). */
export interface Summary {
  p: string;
  rev: number;
  name: string;
  boards: { b: string; name: string; w: number; h: number; n?: number }[];
  assets?: number;
  nodes?: number;
  dir?: string;
}

/** Result of any mutating method. */
export interface Mut {
  ok: boolean;
  rev: number;
  changed: string[];
  created?: string[];
  warnings: string[];
  id?: string;
  ids?: string[];
  z?: number;
  box?: Rect;
  prov?: string[];
  conflicts?: { n: string; i: string; why: string }[];
  passes?: number;
}

export interface Issue {
  /** wire key `s` — the engine emits compact issues, see `inspect::summarize` */
  s: 'error' | 'warn' | 'info' | string;
  /** wire key `c` */
  c: string;
  /** wire key `n` — node id */
  n?: string | null;
  /** wire key `b` — artboard id */
  b?: string | null;
  /** wire key `m` — human message */
  m: string;
  /** wire key `e` — numeric evidence */
  e?: V;
  /** wire key `f` — a ready-to-run `canvas.batch` command */
  f?: V | null;
}

export interface Validate {
  issues: Issue[];
  ok: boolean;
  errors?: number;
  warns?: number;
  infos?: number;
  boards?: { b: string; nodes: number; ink: number }[];
}

export interface Rendered {
  path: string;
  f?: string;
  w?: number;
  h?: number;
  bytes: number;
  sha: string;
  mime?: string;
}

export interface Preview {
  path: string;
  url: string;
  w: number;
  h: number;
  bytes: number;
  sha: string;
}

/** `system.caps.presets` on the wire is a tuple `[key, w, h, label]`; the UI works
 *  with the object form everywhere. `presetsOf()` in state/app.tsx does the
 *  conversion once, at the boundary, so no component sees the raw tuple. */
export type PresetWire = [string, number, number, string] | Preset;

export interface Preset {
  k: string;
  wh: [number, number];
  d: string;
}

export interface Caps {
  name: string;
  ver: string;
  schema: string;
  methods: string[];
  image_ops: string[];
  image_presets: string[];
  templates: string[];
  presets: Preset[];
  keys: V;
  intents: string[];
  node_types: NodeType[];
  limits: V;
  codes: string[];
  paths: V;
  port: number;
  fonts: number;
  loop?: string;
}

/** `artboard.list` — `n` is the layer count (not `layers`). */
export interface BoardInfo {
  b: string;
  name: string;
  w: number;
  h: number;
  n?: number;
  layers?: number;
}

/** `asset.list` — compact `w`/`h`, no `uri`; join with `doc.assets` for the rest. */
export interface AssetRow {
  id: string;
  name: string;
  w: number;
  h: number;
  bytes?: number;
  kind?: string;
}

/** `system.config` — camelCase, and that is the whole writable surface. */
export interface EngineConfig {
  version?: number;
  workspace?: string;
  exportDir?: string;
  autosaveSecs?: number;
  maxImageMp?: number;
  renderScaleMax?: number;
  logCalls?: boolean;
  logCapLines?: number;
  historyCap?: number;
  provider?: { name?: string; baseUrl?: string; apiKeySet?: boolean; baseUrlSet?: boolean };
}

export interface Health {
  ok: boolean;
  ver: string;
  schema: string;
  port: number;
  jobs: number;
  fonts: number;
  root: string;
}

export interface Doctor {
  ok: boolean;
  root: string;
  writable: boolean;
  fonts: number;
  cjk_font: boolean;
  port: number;
  http: boolean;
  version: string;
  warnings: string[];
}

export interface McpTool {
  name: string;
  description: string;
  method: string;
  inputSchema: {
    type: string;
    properties: Record<string, { type?: string; description?: string; items?: unknown }>;
    required?: string[];
    additionalProperties?: boolean;
  };
}

export interface Job {
  id: string;
  kind: string;
  label: string;
  done: number;
  stage: string;
  ok: boolean | null;
  err?: string;
}

export interface LogEntry {
  /** `t` is the engine's `"YYYY-MM-DD HH:MM"` stamp, not an ISO string. */
  t: string;
  m: string;
  ms?: number;
  ok: boolean;
  /** empty string when the call succeeded, otherwise the error code */
  e?: string;
  /** caller door: `http` | `mcp` | `cli` | `desktop` */
  src?: string;
  p?: string;
  args?: V;
}

/** `template.list` — the engine keys are `name` / `desc`; `builtin` is either the
 *  boolean flag on each row or, on the trailing summary row, the whole built-in
 *  table as `[[name, desc], …]`. */
export interface Template {
  name: string;
  desc?: string;
  builtin?: boolean | [string, string][];
  boards?: number;
  updated?: string;
}

export interface HistoryEntry {
  rev: number;
  /** e.g. `"NodeAdd n_5zuo7a4"` — there is no separate `t` field on the wire. */
  label: string;
  origin: string;
  /** epoch milliseconds */
  at?: number | null;
  changed?: string[];
  undo?: V | null;
  redo?: V | null;
  snap?: string | null;
  warnings?: string[];
}

export interface FontFamilies {
  n: number;
  families: string[];
}

export interface RecentProject {
  p: string;
  name: string;
  /** `project.list` reports the board *count*; `project.get depth:0` reports the
   *  board array with real dimensions. They are different shapes on purpose. */
  boards?: number;
  layers?: number;
  assets?: number;
  bytes?: number;
  open?: boolean;
  rev?: number;
  dir?: string;
  abs?: string;
  updated?: string;
}

export interface ImageProbe {
  url?: string;
  name?: string;
  w: number;
  h: number;
  fmt: string;
  alpha: boolean;
  bytes: number;
  sha: string;
  orientation?: string;
  palette?: string[];
}

export interface ImageOut {
  out: string;
  bytes: number;
  sha: string;
  w?: [number, number];
  url?: string;
}

export interface BatchRow {
  i: number;
  ok: boolean;
  out?: string;
  w?: number;
  h?: number;
  bytes?: number;
  e?: string;
  code?: string;
  url?: string;
}

export interface OpDef {
  op: string;
  d: string;
}

export interface PresetDef {
  name: string;
  d: string;
}

/** One entry of the human-readable intent list rendered in the 布局 tab. */
export interface IntentRow {
  key: keyof Intent | 'gap';
  label: string;
  value: string;
}
