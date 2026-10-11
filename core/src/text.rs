//! Font registry, text measurement and CJK-aware wrapping.
//!
//! Three responsibilities, one goal: the desktop preview (a browser), the HTTP/MCP
//! surface and the SVG/raster export must agree on where every glyph lands. That
//! only happens if *the engine* decides the line breaks, and if measurement comes
//! from the very same font file the browser is handed.
//!
//! Design notes
//! - [`Registry::shared`] is built once per process (`OnceLock`) by scanning the
//!   system font directories with `std::fs::read_dir` (`C:\Windows\Fonts` first,
//!   then the per-user font folder, then the POSIX ones). Family/style/weight
//!   metadata comes from `resvg::usvg::fontdb`, which resvg already vendors, so no
//!   dependency is added. Glyph *metrics* come from a small self-contained
//!   TrueType/OpenType table reader in this module (`head`/`hhea`/`maxp`/`hmtx`/
//!   `cmap`/`OS/2`/`post`), because `ttf-parser` is *not* re-exported through
//!   resvg and `Cargo.toml` is off limits.
//! - Table parsing is lazy and cached per face in a `OnceLock`: scanning 150
//!   families costs almost nothing, and a 15 MB `msyh.ttc` is only read by the
//!   faces that are actually measured.
//! - Measurement is advance-width only (no shaping, no kerning, no hinting).
//!   It is reproducible on every machine, which is what a solver needs.
//! - Any code point the chosen face cannot draw gets a documented answer: a
//!   full-width character (Han/kana/Hangul/fullwidth forms) costs exactly one em,
//!   a combining mark costs zero, a space costs the face's space, anything else
//!   costs the `.notdef` advance. That is what lets a Chinese poster measure
//!   correctly on a machine with no Chinese font installed, and it is reported
//!   through `TextMetrics::fell_back`.
//! - Letter spacing is added after *every* character, including the last one (the
//!   CSS model, which is also what usvg/resvg applies), so preview and export
//!   agree to the pixel.

use crate::doc::TextSpec;
use crate::error::{Fail, R};
use resvg::usvg::fontdb::{Database, FaceInfo, Source, Style};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock, RwLockReadGuard};

// ------------------------------------------------------------------ public data

/// One face as the registry sees it: what a font picker lists and what
/// `GET /font/<family>` serves.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct FontInfo {
    pub family: String,
    /// `Regular` | `Bold` | `Italic` | `Bold Italic` | `Oblique` | name-table value.
    pub style: String,
    pub weight: u16,
    pub postscript: String,
    pub path: Option<String>,
    /// Index inside a font collection (`.ttc`); 0 for a single-face file.
    pub index: u32,
}

/// Family used when a document asks for something unavailable and no system font
/// can be found at all.
pub const DEFAULT_FAMILY: &str = "sans-serif";

/// Families tried, in order, when a document asks for a family that is not
/// installed. First hit wins, so measurement is stable on any machine that has a
/// reasonable UI font.
const PREFER_LATIN: &[&str] = &[
    "Segoe UI",
    "Inter",
    "Helvetica Neue",
    "Helvetica",
    "Arial",
    "Noto Sans",
    "DejaVu Sans",
    "Verdana",
    "Tahoma",
];

/// Families that can draw Han, best-for-print first.
const PREFER_CJK: &[&str] = &[
    "Microsoft YaHei",
    "微软雅黑",
    "Noto Sans SC",
    "Source Han Sans SC",
    "Noto Sans CJK SC",
    "PingFang SC",
    "Hiragino Sans GB",
    "SimHei",
    "黑体",
    "SimSun",
    "宋体",
    "Yu Gothic",
    "Malgun Gothic",
    "Arial Unicode MS",
    "WenQuanYi Micro Hei",
];

// ------------------------------------------------------------------ face

/// Where the bytes of a face live.
#[derive(Clone)]
enum Blob {
    File(PathBuf),
    Mem(Arc<Vec<u8>>),
}

/// A loaded face plus its lazily parsed glyph metrics.
pub struct Face {
    /// Canonical (English) family name, e.g. `Microsoft YaHei`.
    pub family: String,
    /// Every name this face answers to, lowercased: declared families (including
    /// localised ones such as `微软雅黑`) plus the PostScript name.
    pub names: Vec<String>,
    pub style: String,
    pub weight: u16,
    pub italic: bool,
    pub postscript: String,
    pub path: Option<String>,
    pub index: u32,
    pub monospaced: bool,
    blob: Blob,
    metrics: OnceLock<Option<Arc<Metrics>>>,
}

impl std::fmt::Debug for Face {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Face")
            .field("family", &self.family)
            .field("style", &self.style)
            .field("weight", &self.weight)
            .field("italic", &self.italic)
            .field("postscript", &self.postscript)
            .field("path", &self.path)
            .field("index", &self.index)
            .field("metrics_cached", &self.metrics.get().is_some())
            .finish()
    }
}

impl Face {
    fn info(&self) -> FontInfo {
        FontInfo {
            family: self.family.clone(),
            style: self.style.clone(),
            weight: self.weight,
            postscript: self.postscript.clone(),
            path: self.path.clone(),
            index: self.index,
        }
    }

    /// Parsed tables for this face, cached for the process lifetime. `None` when
    /// the file is unreadable or is not an sfnt we can parse.
    pub fn metrics(&self) -> Option<&Metrics> {
        self.metrics
            .get_or_init(|| load_metrics(&self.blob, self.index).map(Arc::new))
            .as_ref()
            .map(|a| &**a)
    }

    fn bytes(&self) -> Option<Vec<u8>> {
        match &self.blob {
            Blob::File(p) => std::fs::read(p).ok(),
            Blob::Mem(b) => Some(b.as_ref().clone()),
        }
    }

    /// Does this face have a glyph for `ch`?
    pub fn has(&self, ch: char) -> bool {
        self.metrics().and_then(|m| m.glyph_of(ch)).is_some()
    }

    /// Advance of `ch` in ems.
    pub fn advance_em(&self, ch: char) -> Option<f64> {
        self.metrics().and_then(|m| m.advance_em(ch))
    }
}

fn load_metrics(blob: &Blob, index: u32) -> Option<Metrics> {
    let bytes = match blob {
        Blob::File(p) => std::fs::read(p).ok()?,
        Blob::Mem(b) => b.as_ref().clone(),
    };
    Metrics::parse(&bytes, index)
}

// ------------------------------------------------------------------ registry

#[derive(Default)]
struct Inner {
    faces: Vec<Arc<Face>>,
    /// lowercased name -> indices into `faces`, in registration order
    by_name: HashMap<String, Vec<usize>>,
    /// canonical file path -> the faces it contributed (makes registration idempotent)
    files: HashMap<PathBuf, Vec<usize>>,
}

/// Process-wide font registry. Querying never holds a lock across file IO: faces
/// are cloned out as `Arc<Face>` and their tables parsed afterwards.
pub struct Registry {
    inner: RwLock<Inner>,
}

static SHARED: OnceLock<Registry> = OnceLock::new();

impl Registry {
    /// The shared registry; system fonts are scanned on first use.
    pub fn shared() -> &'static Registry {
        SHARED.get_or_init(|| {
            let reg = Registry { inner: RwLock::new(Inner::default()) };
            for dir in font_dirs() {
                reg.load_dir(&dir);
            }
            reg
        })
    }

    /// A registry that knows nothing but what you register by hand. Tests use it
    /// so they cannot be broken by the host's font set.
    pub fn empty() -> Registry {
        Registry { inner: RwLock::new(Inner::default()) }
    }

    /// Sorted, deduplicated family names.
    pub fn families(&self) -> Vec<String> {
        let g = guard(&self.inner);
        let mut seen = HashSet::new();
        let mut out: Vec<String> = g
            .faces
            .iter()
            .filter(|f| seen.insert(f.family.to_lowercase()))
            .map(|f| f.family.clone())
            .collect();
        out.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(b)));
        out
    }

    /// Every face, sorted by family, weight, style, collection index.
    pub fn fonts(&self) -> Vec<FontInfo> {
        let g = guard(&self.inner);
        let mut out: Vec<FontInfo> = g.faces.iter().map(|f| f.info()).collect();
        out.sort_by(|a, b| {
            a.family
                .to_lowercase()
                .cmp(&b.family.to_lowercase())
                .then(a.weight.cmp(&b.weight))
                .then(a.style.cmp(&b.style))
                .then(a.index.cmp(&b.index))
                .then(a.postscript.cmp(&b.postscript))
        });
        out
    }

    pub fn len(&self) -> usize {
        guard(&self.inner).faces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Load every font file under `path` (recursive, depth-limited). Never fails:
    /// one broken system font must not break a render.
    fn load_dir(&self, path: &Path) {
        let rd = match std::fs::read_dir(path) {
            Ok(rd) => rd,
            Err(_) => return,
        };
        let mut files: Vec<PathBuf> = vec![];
        collect_font_files(rd, 0, &mut files);
        // Sorted: which file wins a family/weight tie must not depend on
        // directory enumeration order.
        files.sort();
        files.dedup();
        for f in files {
            let _ = self.register_file(&f);
        }
    }

    /// Register one font file. Idempotent: the same path twice returns the same
    /// family and adds nothing.
    pub fn register_file(&self, path: &Path) -> R<String> {
        let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(fam) = self.family_of_file(&canon) {
            return Ok(fam);
        }
        if !path.is_file() {
            return Err(Fail::new("NOT_FOUND", format!("no such font file: {}", path.display()))
                .hint("pass an existing .ttf/.otf/.ttc file"));
        }
        let mut db = Database::new();
        db.load_font_file(path).map_err(|e| {
            Fail::new("FONT_MISSING", format!("{}: {e}", path.display()))
                .hint("only sfnt fonts (.ttf/.otf/.ttc) are supported, not .woff")
        })?;
        let infos: Vec<FaceInfo> = db.faces().cloned().collect();
        drop(db);
        self.snapshot(infos, Blob::File(canon.clone()), Some(canon), path)
    }

    /// Every font file this registry knows about. The rasteriser uses this to
    /// mirror the measurement engine, so a font added with `text.register` after
    /// startup is still the font `render.export` draws with.
    pub fn known_files(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self
            .inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .files
            .keys()
            .cloned()
            .collect();
        v.sort();
        v
    }

    /// Register font bytes (an uploaded font, or one embedded in a bundle).
    pub fn register_bytes(&self, bytes: Vec<u8>) -> R<String> {
        if bytes.is_empty() {
            return Err(Fail::new("INVALID_VALUE", "empty font bytes"));
        }
        let arc = Arc::new(bytes);
        let mut db = Database::new();
        db.load_font_data(arc.as_ref().clone());
        let infos: Vec<FaceInfo> = db.faces().cloned().collect();
        drop(db);
        self.snapshot(infos, Blob::Mem(arc), None, Path::new("<memory>"))
    }

    fn family_of_file(&self, canon: &Path) -> Option<String> {
        let g = guard(&self.inner);
        let ix = g.files.get(canon)?;
        ix.first().and_then(|i| g.faces.get(*i)).map(|f| f.family.clone())
    }

    fn snapshot(&self, infos: Vec<FaceInfo>, blob: Blob, canon: Option<PathBuf>, shown: &Path) -> R<String> {
        let mut added: Vec<Arc<Face>> = vec![];
        for info in infos {
            let family = match info.families.first() {
                Some((n, _)) => n.clone(),
                None => continue,
            };
            if family.trim().is_empty() {
                continue;
            }
            let italic = matches!(info.style, Style::Italic | Style::Oblique);
            let path = match &info.source {
                Source::File(p) => Some(p.to_string_lossy().replace('\\', "/")),
                _ => canon.as_ref().map(|p| p.to_string_lossy().replace('\\', "/")),
            };
            let mut names: Vec<String> = info.families.iter().map(|(n, _)| n.to_lowercase()).collect();
            names.push(family.to_lowercase());
            names.push(info.post_script_name.to_lowercase());
            names.sort();
            names.dedup();
            added.push(Arc::new(Face {
                family,
                names,
                style: style_name(info.style, info.weight.0),
                weight: info.weight.0,
                italic,
                postscript: info.post_script_name.clone(),
                path,
                index: info.index,
                monospaced: info.monospaced,
                blob: blob.clone(),
                metrics: OnceLock::new(),
            }));
        }
        if added.is_empty() {
            return Err(Fail::new(
                "FONT_MISSING",
                format!("no usable font face in {}", shown.display()),
            )
            .hint("convert the font to .ttf/.otf; .woff is not supported"));
        }

        let mut g = self.inner.write().unwrap_or_else(|e| e.into_inner());
        // Re-check under the write lock: a concurrent caller may have won the race.
        if let Some(c) = &canon {
            if let Some(ix) = g.files.get(c) {
                if let Some(f) = ix.first().and_then(|i| g.faces.get(*i)) {
                    return Ok(f.family.clone());
                }
            }
        }
        let mut mine: Vec<usize> = vec![];
        let first_family = added[0].family.clone();
        for f in added {
            let ix = g.faces.len();
            for n in &f.names {
                g.by_name.entry(n.clone()).or_default().push(ix);
            }
            g.faces.push(f);
            mine.push(ix);
        }
        if let Some(c) = canon {
            g.files.insert(c, mine);
        }
        Ok(first_family)
    }

    // ------------------------------------------------------------------ lookup

    fn faces_named(&self, name: &str) -> Vec<Arc<Face>> {
        let key = name.trim().trim_matches('"').trim_matches('\'').trim().to_lowercase();
        if key.is_empty() {
            return vec![];
        }
        let g = guard(&self.inner);
        let hit = match g.by_name.get(&key) {
            Some(h) => h,
            None => {
                let bare = key.replace(' ', "");
                match g
                    .by_name
                    .iter()
                    .find(|(k, _)| !bare.is_empty() && k.replace(' ', "") == bare)
                    .map(|(_, v)| v)
                {
                    Some(h) => h,
                    None => return vec![],
                }
            }
        };
        hit.iter().filter_map(|i| g.faces.get(*i).cloned()).collect()
    }

    /// Nearest-face matching: honour italic/oblique when the family has such a
    /// face, then take the closest weight (preferring the lighter one on a tie at
    /// or below 400, which is what CSS does).
    fn pick_from(cands: &[Arc<Face>], weight: u16, italic: bool) -> Option<Arc<Face>> {
        if cands.is_empty() {
            return None;
        }
        let same: Vec<&Arc<Face>> = cands.iter().filter(|f| f.italic == italic).collect();
        let pool: Vec<&Arc<Face>> = if same.is_empty() { cands.iter().collect() } else { same };
        let mut best: Option<(i64, usize, Arc<Face>)> = None;
        for (n, f) in pool.iter().enumerate() {
            let diff = f.weight as i64 - weight as i64;
            let mut d = diff.abs() * 2;
            if d == 0 {
                d = 1;
            } else if weight <= 400 && diff < 0 {
                d -= 1;
            }
            if best.as_ref().map(|(bd, _, _)| d < *bd).unwrap_or(true) {
                best = Some((d, n, (*f).clone()));
            }
        }
        best.map(|(_, _, f)| f)
    }

    /// The face that will be used for `family`/`weight`/`italic`.
    pub fn face(&self, family: &str, weight: u16, italic: bool) -> Option<Arc<Face>> {
        for want in family_list(family) {
            let cands = self.faces_named(&want);
            if let Some(f) = Registry::pick_from(&cands, weight, italic) {
                return Some(f);
            }
        }
        let lower = family.trim().to_lowercase();
        let generic = [
            ("sans-serif", PREFER_LATIN[0]),
            ("ui-sans-serif", PREFER_LATIN[0]),
            ("system-ui", PREFER_LATIN[0]),
            ("serif", "Times New Roman"),
            ("monospace", "Courier New"),
            ("cursive", "Comic Sans MS"),
            ("fantasy", "Impact"),
        ];
        for (name, prefer) in generic {
            if lower == name || lower.contains(name) {
                let cands = self.faces_named(prefer);
                if let Some(f) = Registry::pick_from(&cands, weight, italic) {
                    return Some(f);
                }
            }
        }
        let d = self.default_family();
        let cands = self.faces_named(&d);
        Registry::pick_from(&cands, weight, italic).or_else(|| guard(&self.inner).faces.first().cloned())
    }

    /// Raw bytes of the chosen face. For a collection this is the whole `.ttc`,
    /// which is exactly the point: the browser receives the same file resvg reads,
    /// so glyph shapes cannot drift between preview and export.
    pub fn face_bytes(&self, family: &str, weight: u16, italic: bool) -> Option<Vec<u8>> {
        self.face(family, weight, italic).and_then(|f| f.bytes())
    }

    /// Metadata of the chosen face (path and collection index included).
    pub fn face_info(&self, family: &str, weight: u16, italic: bool) -> Option<FontInfo> {
        self.face(family, weight, italic).map(|f| f.info())
    }

    /// Is any family from this (comma-separated) list installed?
    pub fn has(&self, family: &str) -> bool {
        family_list(family).iter().any(|f| !self.faces_named(f).is_empty())
    }

    /// First family from `want` (a comma list) that exists, else the default.
    /// Returns the canonical spelling, so `resolve("arial")` gives `Arial` and
    /// `resolve("微软雅黑")` gives `Microsoft YaHei`.
    pub fn resolve(&self, want: &str) -> String {
        for w in family_list(want) {
            if let Some(f) = self.faces_named(&w).first() {
                return f.family.clone();
            }
        }
        self.default_family()
    }

    /// The family used when nothing else matches.
    pub fn default_family(&self) -> String {
        for p in PREFER_LATIN {
            if !self.faces_named(p).is_empty() {
                return (*p).to_string();
            }
        }
        self.families().first().cloned().unwrap_or_else(|| DEFAULT_FAMILY.to_string())
    }

    /// First installed family that can actually draw Han — the CJK fallback.
    pub fn cjk_family(&self) -> String {
        for p in PREFER_CJK {
            let cands = self.faces_named(p);
            if let Some(f) = cands.iter().find(|f| f.has('中')) {
                return f.family.clone();
            }
        }
        let g = guard(&self.inner);
        if let Some(f) = g.faces.iter().find(|f| f.has('中')) {
            return f.family.clone();
        }
        self.default_family()
    }

    // ------------------------------------------------------------------ measure

    /// Measurement context for a request: the primary face plus ordered fallbacks.
    pub fn ctx(&self, family: &str, weight: u16, italic: bool) -> Ctx {
        let primary = self.face(family, weight, italic);
        let mut fallbacks: Vec<Arc<Face>> = vec![];
        for name in [self.cjk_family(), self.default_family()] {
            if let Some(f) = self.face(&name, weight, italic) {
                let dup = primary.as_ref().map(|p| same_face(p, &f)).unwrap_or(false)
                    || fallbacks.iter().any(|x| same_face(x, &f));
                if !dup {
                    fallbacks.push(f);
                }
            }
        }
        Ctx { primary, fallbacks }
    }

    /// Advance width of `s` at `size` px with `letter_spacing`, in px.
    /// Deterministic: no shaping state, no locale, no cache-order influence.
    pub fn measure(
        &self,
        s: &str,
        family: &str,
        weight: u16,
        italic: bool,
        size: f64,
        letter_spacing: f64,
    ) -> f64 {
        self.ctx(family, weight, italic).width(s, size, letter_spacing).0
    }

    /// Like [`Registry::measure`] but also reports whether a fallback face answered
    /// and which family was used.
    pub fn measure_ex(
        &self,
        s: &str,
        family: &str,
        weight: u16,
        italic: bool,
        size: f64,
        letter_spacing: f64,
    ) -> (f64, bool, String) {
        let ctx = self.ctx(family, weight, italic);
        let (w, fell) = ctx.width(s, size, letter_spacing);
        let used = ctx.primary.as_ref().map(|f| f.family.clone()).unwrap_or_else(|| self.resolve(family));
        (w, fell, used)
    }

    /// `true` when the chosen face has a glyph for every visible character of `s`
    /// — i.e. no tofu. (`tofu_risk` is the same test the other way round.)
    pub fn covers(&self, s: &str, family: &str, weight: u16) -> bool {
        self.missing_glyph(s, family, weight).is_none()
    }

    /// `true` when at least one character would render as tofu in `family`. This
    /// is what the inspector uses to warn about a missing Chinese font.
    pub fn tofu_risk(&self, s: &str, family: &str, weight: u16) -> bool {
        self.missing_glyph(s, family, weight).is_some()
    }

    /// The first character the face cannot draw. Spaces, newlines and combining
    /// marks never count.
    pub fn missing_glyph(&self, s: &str, family: &str, weight: u16) -> Option<char> {
        let face = self.face(family, weight, false)?;
        for ch in s.chars() {
            if ch == '\n' || ch == '\r' || ch.is_whitespace() || is_combining(ch) {
                continue;
            }
            if !face.has(ch) {
                return Some(ch);
            }
        }
        None
    }

    /// `true` when some installed family can draw every character of `s`.
    pub fn covers_somewhere(&self, s: &str) -> bool {
        for ch in s.chars() {
            if ch == '\n' || ch == '\r' || ch.is_whitespace() || is_combining(ch) {
                continue;
            }
            let g = guard(&self.inner);
            if !g.faces.iter().any(|f| f.has(ch)) {
                return false;
            }
        }
        true
    }

    /// Load every known face into a `fontdb::Database` so resvg rasterises with
    /// exactly the same faces the solver measured with. Call once and reuse.
    pub fn populate_fontdb(&self, db: &mut Database) {
        let g = guard(&self.inner);
        let mut seen: HashSet<String> = HashSet::new();
        for f in &g.faces {
            match &f.blob {
                Blob::File(p) => {
                    if seen.insert(p.to_string_lossy().to_lowercase()) {
                        let _ = db.load_font_file(p);
                    }
                }
                Blob::Mem(b) => {
                    if seen.insert(util_ptr_key(b)) {
                        db.load_font_data(b.as_ref().clone());
                    }
                }
            }
        }
    }

    /// Measure straight off a `TextSpec` (convenience for `text.measure`).
    pub fn measure_spec(&self, s: &str, spec: &TextSpec) -> f64 {
        let (m, _mode, _h, _min) = opts_for_spec(spec, f64::INFINITY, f64::INFINITY);
        self.measure(s, &m.family, m.weight, m.italic, m.size, m.ls)
    }
}

fn util_ptr_key(b: &Arc<Vec<u8>>) -> String {
    format!("{:?}", Arc::as_ptr(b))
}

fn guard<T>(l: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(|e| e.into_inner())
}

fn same_face(a: &Face, b: &Face) -> bool {
    a.postscript == b.postscript && a.index == b.index && a.path == b.path && a.family == b.family
}

fn style_name(style: Style, weight: u16) -> String {
    match (style, weight) {
        (Style::Italic, w) if w >= 600 => "Bold Italic".into(),
        (Style::Italic, _) => "Italic".into(),
        (Style::Oblique, w) if w >= 600 => "Bold Oblique".into(),
        (Style::Oblique, _) => "Oblique".into(),
        (_, 100) => "Thin".into(),
        (_, 200) => "Extra Light".into(),
        (_, 300) => "Light".into(),
        (_, 500) => "Medium".into(),
        (_, 600) => "Semi Bold".into(),
        (_, 700) => "Bold".into(),
        (_, 800) => "Extra Bold".into(),
        (_, 900) => "Black".into(),
        _ => "Regular".into(),
    }
}

// ------------------------------------------------------------------ measuring context

/// A primary face plus the ordered fallbacks used for characters it cannot draw.
#[derive(Clone, Debug)]
pub struct Ctx {
    primary: Option<Arc<Face>>,
    fallbacks: Vec<Arc<Face>>,
}

impl Ctx {
    pub fn has(&self, ch: char) -> bool {
        if self.primary.as_ref().map(|p| p.has(ch)).unwrap_or(false) {
            return true;
        }
        self.fallbacks.iter().any(|f| f.has(ch))
    }

    /// The family the primary face belongs to (`None` when nothing was resolved).
    pub fn family(&self) -> Option<String> {
        self.primary.as_ref().map(|f| f.family.clone())
    }

    /// Advance of one character in ems, and whether a fallback face answered.
    fn em(&self, ch: char) -> (f64, bool) {
        if is_combining(ch) {
            return (0.0, false);
        }
        if let Some(e) = self.primary.as_ref().and_then(|p| p.advance_em(ch)) {
            return (e, false);
        }
        for f in &self.fallbacks {
            if let Some(e) = f.advance_em(ch) {
                return (e, true);
            }
        }
        // Nothing on this machine has the glyph: font-independent documented rules.
        if ch == '\u{3000}' {
            return (1.0, true);
        }
        if ch == ' ' || ch == '\t' {
            let sp = self.primary.as_ref().and_then(|p| p.advance_em(' ')).unwrap_or(0.25);
            return (sp, true);
        }
        if is_wide(ch) {
            return (1.0, true);
        }
        let nd = self.primary.as_ref().and_then(|p| p.metrics()).map(|m| m.notdef_em()).unwrap_or(0.5);
        (nd.max(0.0), true)
    }

    /// Width of `s` in px: sum of advances plus letter-spacing after every char.
    pub fn width(&self, s: &str, size: f64, ls: f64) -> (f64, bool) {
        let mut w = 0.0;
        let mut fell = false;
        for ch in s.chars() {
            if ch == '\n' || ch == '\r' {
                continue;
            }
            let (e, f) = self.em(ch);
            fell |= f;
            w += e * size + ls;
        }
        (w.max(0.0), fell)
    }

    /// Per-character px advances, used when a word must be split mid-flight.
    fn chars(&self, s: &str, size: f64, ls: f64) -> Vec<(char, f64)> {
        let mut out = Vec::with_capacity(s.chars().count());
        for ch in s.chars() {
            if ch == '\n' || ch == '\r' {
                continue;
            }
            let (e, _f) = self.em(ch);
            out.push((ch, (e * size + ls).max(0.0)));
        }
        out
    }

    fn ascent_descent(&self, size: f64) -> (f64, f64) {
        if let Some(m) = self.primary.as_ref().and_then(|p| p.metrics()) {
            return (m.ascender_em() * size, m.descender_em() * size);
        }
        (0.8 * size, -0.2 * size)
    }
}

fn family_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim().trim_matches('"').trim_matches('\'').trim().to_string())
        .filter(|x| !x.is_empty() && !is_generic_family(x))
        .collect()
}

fn is_generic_family(s: &str) -> bool {
    matches!(
        s.to_lowercase().as_str(),
        "sans-serif" | "serif" | "monospace" | "cursive" | "fantasy" | "system-ui" | "ui-sans-serif" | "text" | "emoji"
    )
}

// ------------------------------------------------------------------ wrap API

/// Everything the wrapper needs about the font and the box, in one bundle.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MeasureOpts {
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    pub size: f64,
    /// letter-spacing in px, added after every character.
    pub ls: f64,
    /// line height in px; `<= 0` means `size * 1.35`.
    pub line_h: f64,
    /// content width in px; `<= 0` or infinite means "do not wrap".
    pub max_w: f64,
    /// `left` | `center` | `right`
    pub align: String,
    /// `top` | `middle` | `bottom`
    pub valign: String,
}

impl Default for MeasureOpts {
    fn default() -> Self {
        Self {
            family: DEFAULT_FAMILY.to_string(),
            weight: 400,
            italic: false,
            size: 16.0,
            ls: 0.0,
            line_h: 0.0,
            max_w: f64::INFINITY,
            align: "left".into(),
            valign: "top".into(),
        }
    }
}

impl MeasureOpts {
    /// Box-constrained options at `size` px.
    pub fn boxed(family: &str, weight: u16, size: f64, max_w: f64) -> Self {
        Self { family: family.to_string(), weight, size, max_w, ..Default::default() }
    }

    /// Effective line height in px at `size`.
    pub fn eff_line_h(&self, size: f64) -> f64 {
        if self.line_h > 0.0 && self.line_h.is_finite() {
            self.line_h
        } else {
            size * 1.35
        }
    }

    /// Options re-scaled to `size`, so shrinking keeps proportions.
    pub fn scaled(&self, size: f64) -> Self {
        let ratio = if self.size > 0.0 { size / self.size } else { 1.0 };
        let mut o = self.clone();
        o.size = size;
        if o.line_h > 0.0 && o.line_h.is_finite() {
            o.line_h = self.line_h * ratio;
        }
        o.ls = self.ls * ratio;
        o
    }
}

/// The solved result of laying text out in a box.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct TextMetrics {
    pub lines: Vec<String>,
    pub width: f64,
    pub height: f64,
    pub line_height: f64,
    pub ascender: f64,
    pub descender: f64,
    pub overflow: bool,
    pub truncated: bool,
    pub font: String,
    pub fell_back: bool,
}

impl TextMetrics {
    pub fn n_lines(&self) -> usize {
        self.lines.len()
    }
}

/// What the `overflow` / `mode` string means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overflow {
    /// Break long words so everything lands on some line (the default).
    Wrap,
    /// Never break a long word; content outside the box is clipped away.
    Clip,
    /// Binary-search the font size down until the text fits.
    Shrink,
    /// Shrink, from the `doc.rs` vocabulary.
    Scale,
    /// Wrap, and simply report the overflow.
    Overflow,
}

impl Overflow {
    pub fn parse(s: &str) -> Overflow {
        match s.trim().to_lowercase().as_str() {
            "clip" | "hide" | "hidden" | "cut" => Overflow::Clip,
            "shrink" | "fit" | "auto-shrink" | "shrink-to-fit" => Overflow::Shrink,
            "scale" | "scale-down" => Overflow::Scale,
            "overflow" | "visible" | "none" | "" => Overflow::Overflow,
            _ => Overflow::Wrap,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Overflow::Wrap => "wrap",
            Overflow::Clip => "clip",
            Overflow::Shrink => "shrink",
            Overflow::Scale => "scale",
            Overflow::Overflow => "overflow",
        }
    }
}

/// Wrap `s` for `m`; `mode` is the overflow behaviour (`clip`, `shrink`, `scale`,
/// `overflow`, anything else = `wrap`).
///
/// The box height is unconstrained here because `MeasureOpts` only carries a
/// width, and shrinking searches `[size*0.25, size]`. Use [`layout_text_in`] when
/// a height is known, or [`layout_for_spec`] to take the box and the size bounds
/// straight from a `TextSpec`.
pub fn layout_text(s: &str, m: &MeasureOpts, mode: &str) -> TextMetrics {
    let max = m.size.max(1.0);
    let min = (max * 0.25).max(1.0);
    layout_text_with(Registry::shared(), s, m, mode, f64::INFINITY, min, max)
}

/// As [`layout_text`] with a known content height.
pub fn layout_text_in(s: &str, m: &MeasureOpts, mode: &str, box_h: f64) -> TextMetrics {
    let max = m.size.max(1.0);
    let min = (max * 0.25).max(1.0);
    layout_text_with(Registry::shared(), s, m, mode, box_h, min, max)
}

/// Full control, shared registry: wrap then binary-search the size between
/// `min_size` and `max_size` until the result fits `m.max_w` x `box_h`.
pub fn layout_text_fit(
    s: &str,
    m: &MeasureOpts,
    mode: &str,
    box_h: f64,
    min_size: f64,
    max_size: f64,
) -> TextMetrics {
    layout_text_with(Registry::shared(), s, m, mode, box_h, min_size, max_size)
}

/// The same search against an explicit registry.
pub fn layout_text_with(
    reg: &Registry,
    s: &str,
    m: &MeasureOpts,
    mode: &str,
    box_h: f64,
    min_size: f64,
    max_size: f64,
) -> TextMetrics {
    let ovf = Overflow::parse(mode);
    let want = family_list(&m.family)
        .into_iter()
        .next()
        .unwrap_or_else(|| m.family.trim().to_string());
    let ctx = reg.ctx(&m.family, m.weight, m.italic);
    let font = reg.resolve(&m.family);
    // Family-level fallback: the request named a specific family that is simply
    // not installed here.
    let family_fell = !want.is_empty()
        && !is_generic_family(&want)
        && !reg.faces_named(&want).iter().any(|f| f.family.to_lowercase() == want.to_lowercase());

    let max = max_size.max(1.0);
    let min = min_size.clamp(1.0, max);
    let mut best = run(s, &ctx, &m.scaled(max), ovf, box_h, &font, family_fell);

    if matches!(ovf, Overflow::Shrink | Overflow::Scale) && best.overflow && max - min > 0.01 {
        let (mut lo, mut hi) = (min, max);
        let mut fitted: Option<TextMetrics> = None;
        // Fixed iteration count: same input, same answer, every machine.
        for _ in 0..24 {
            if hi - lo <= 0.05 {
                break;
            }
            let mid = (lo + hi) / 2.0;
            let cand = run(s, &ctx, &m.scaled(mid), ovf, box_h, &font, family_fell);
            if cand.overflow {
                hi = mid;
            } else {
                fitted = Some(cand);
                lo = mid;
            }
        }
        match fitted {
            Some(f) => best = f,
            None => {
                best = run(s, &ctx, &m.scaled(min), ovf, box_h, &font, family_fell);
                best.truncated = true;
            }
        }
    }
    best
}

/// Options + box straight from a `TextSpec` and a node box, then solve.
/// `TextSpec::padding` is inset on all four sides; `box_mode`/`overflow`/
/// `min_font_size`/`max_font_size`/`uppercase` are honoured.
pub fn layout_for_spec(s: &str, spec: &TextSpec, w: f64, h: f64) -> TextMetrics {
    let (m, mode, box_h, min) = opts_for_spec(spec, w, h);
    let text = if spec.uppercase.unwrap_or(false) { s.to_uppercase() } else { s.to_string() };
    let hi = spec.max_font_size.unwrap_or(m.size).max(min);
    layout_text_with(Registry::shared(), &text, &m, &mode, box_h, min, hi)
}

/// `(options, overflow mode, content height, min font size)` for a `TextSpec`.
pub fn opts_for_spec(spec: &TextSpec, w: f64, h: f64) -> (MeasureOpts, String, f64, f64) {
    let pad = spec.padding.unwrap_or(0.0);
    let size = spec.size.unwrap_or(16.0);
    let m = MeasureOpts {
        family: spec
            .family
            .clone()
            .or_else(|| spec.font_ref.clone())
            .or_else(|| spec.fallback.clone())
            .unwrap_or_else(|| DEFAULT_FAMILY.to_string()),
        weight: spec.weight.unwrap_or(400),
        italic: spec.style_italic.unwrap_or(false),
        size,
        ls: spec.letter_spacing.unwrap_or(0.0),
        line_h: spec.line_h(),
        max_w: if w.is_finite() { (w - pad * 2.0).max(0.0) } else { f64::INFINITY },
        align: spec.align.clone().unwrap_or_else(|| "left".into()),
        valign: spec.valign.clone().unwrap_or_else(|| "top".into()),
    };
    (
        m,
        spec.overflow.clone().unwrap_or_else(|| "wrap".into()),
        if h.is_finite() { (h - pad * 2.0).max(0.0) } else { f64::INFINITY },
        spec.min_font_size.unwrap_or((size * 0.25).max(1.0)).max(1.0),
    )
}

/// `text.measure` for the RPC layer.
pub fn metrics_for(s: &str, m: &MeasureOpts, mode: &str) -> TextMetrics {
    layout_text(s, m, mode)
}

// ------------------------------------------------------------------ the wrapper

const EPS: f64 = 0.05;

/// One indivisible piece of text, with the whitespace run that precedes it.
struct Unit {
    text: String,
    ws: String,
    w_ws: f64,
    w: f64,
}

/// Wrap + measure at one fixed size.
fn run(s: &str, ctx: &Ctx, m: &MeasureOpts, ovf: Overflow, box_h: f64, font: &str, family_fell: bool) -> TextMetrics {
    let size = m.size.max(0.5);
    let max_w = if m.max_w > 0.0 && m.max_w.is_finite() { m.max_w } else { f64::INFINITY };
    let may_break = !matches!(ovf, Overflow::Clip | Overflow::Overflow);

    let mut lines: Vec<String> = vec![];
    let mut overflow = false;
    let mut truncated = false;
    let mut fell = family_fell;

    for hard in split_hard(s) {
        if hard.is_empty() {
            lines.push(String::new());
            continue;
        }
        let (units, f) = tokenize(ctx, &hard, size, m.ls);
        fell |= f;
        let mut items: Vec<Unit> = vec![];
        for u in units {
            if max_w.is_finite() && u.w > max_w + EPS && may_break {
                let pieces = split_unit(ctx, &u.text, max_w, size, m.ls);
                for (i, (t, tw)) in pieces.into_iter().enumerate() {
                    items.push(Unit {
                        text: t,
                        ws: if i == 0 { u.ws.clone() } else { String::new() },
                        w_ws: if i == 0 { u.w_ws } else { 0.0 },
                        w: tw,
                    });
                }
            } else {
                items.push(u);
            }
        }
        lines.extend(fill(items, max_w));
    }

    // CJK typography: no line may start with closing punctuation.
    fix_orphans(&mut lines);

    if lines.is_empty() {
        lines.push(String::new());
    }

    let mut width = 0.0;
    for l in &lines {
        let (w, f) = ctx.width(l, size, m.ls);
        fell |= f;
        let w = w.max(0.0);
        if w > width {
            width = w;
        }
        if max_w.is_finite() && w > max_w + EPS {
            overflow = true;
            truncated |= matches!(ovf, Overflow::Clip);
        }
    }

    let line_h = m.eff_line_h(size);
    let (asc, desc) = ctx.ascent_descent(size);
    let total_h = lines.len() as f64 * line_h;
    let mut out_lines = lines;
    if box_h.is_finite() && box_h > 0.0 && total_h > box_h + EPS {
        overflow = true;
        if matches!(ovf, Overflow::Clip) {
            let keep = (box_h / line_h).floor().max(0.0) as usize;
            if keep < out_lines.len() {
                out_lines.truncate(keep);
                truncated = true;
            }
        }
    }

    let height = out_lines.len().max(1) as f64 * line_h;
    TextMetrics {
        lines: out_lines,
        width,
        height,
        line_height: line_h,
        ascender: asc,
        descender: desc,
        overflow,
        truncated,
        font: font.to_string(),
        fell_back: fell,
    }
}

/// Hard breaks first: `\n`, `\r\n`, `\r`, and the Unicode line/paragraph
/// separators that mean the same thing to a reader.
fn split_hard(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\n' || c == '\u{2028}' || c == '\u{2029}' || c == '\u{000B}' || c == '\u{000C}' {
            out.push(std::mem::take(&mut cur));
        } else if c == '\r' {
            if it.peek() == Some(&'\n') {
                it.next();
            }
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

/// Is a line break allowed between the non-space neighbours `a` and `b`?
fn breakable(a: char, b: char) -> bool {
    if is_close_punct(b) || is_cjk_small(b) {
        return false; // never start a line with a closer
    }
    if is_open_punct(a) || is_prefix_punct(a) {
        return false; // never end a line with an opener, a currency sign or a quote
    }
    if is_hyphen(a) {
        return true; // break after a hyphen
    }
    is_cjk(a) || is_cjk(b) // CJK body text breaks between characters
}

/// Split one hard line into unbreakable units.
fn tokenize(ctx: &Ctx, line: &str, size: f64, ls: f64) -> (Vec<Unit>, bool) {
    let mut units: Vec<Unit> = vec![];
    let mut fell = false;
    let mut cur = String::new();
    let mut cur_w = 0.0f64;
    let mut ws = String::new();
    let mut ws_w = 0.0f64;
    let mut prev: Option<char> = None;

    for ch in line.chars() {
        let (e, f) = ctx.em(ch);
        fell |= f;
        let adv = (e * size + ls).max(0.0);

        if is_space_ch(ch) {
            close(&mut cur, &mut cur_w, &mut ws, &mut ws_w, &mut units);
            ws.push(ch);
            ws_w += adv;
            prev = Some(ch);
            continue;
        }
        let start_new = match prev {
            None => false,
            Some(p) => !is_space_ch(p) && breakable(p, ch),
        };
        if start_new {
            close(&mut cur, &mut cur_w, &mut ws, &mut ws_w, &mut units);
        }
        cur.push(ch);
        cur_w += adv;
        prev = Some(ch);
    }
    close(&mut cur, &mut cur_w, &mut ws, &mut ws_w, &mut units);

    if units.is_empty() && !line.is_empty() {
        let w = ctx.width(line, size, ls).0;
        units.push(Unit { text: line.to_string(), ws: String::new(), w_ws: 0.0, w });
    }
    (units, fell)
}

fn close(cur: &mut String, cur_w: &mut f64, ws: &mut String, ws_w: &mut f64, units: &mut Vec<Unit>) {
    if !cur.is_empty() {
        units.push(Unit {
            text: std::mem::take(cur),
            ws: std::mem::take(ws),
            w_ws: std::mem::take(ws_w),
            w: std::mem::take(cur_w),
        });
    }
}

fn is_space_ch(ch: char) -> bool {
    ch == ' ' || ch == '\t' || (ch.is_whitespace() && ch != '\u{00A0}' && ch != '\u{202F}')
}

/// Greedy line filling. The whitespace in front of the first unit of a hard line
/// is kept (it is an indent); everywhere else it is a dropped break cost.
fn fill(items: Vec<Unit>, max_w: f64) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut line = String::new();
    let mut line_w = 0.0f64;

    for u in items {
        let add_ws = if line.is_empty() {
            if out.is_empty() {
                u.w_ws
            } else {
                0.0
            }
        } else {
            u.w_ws
        };
        if max_w.is_finite() && !line.is_empty() && line_w + add_ws + u.w > max_w + EPS {
            out.push(std::mem::take(&mut line));
            line_w = 0.0;
            line.push_str(&u.text);
            line_w += u.w;
        } else {
            if add_ws > 0.0 {
                line.push_str(&u.ws);
            }
            line.push_str(&u.text);
            line_w += add_ws + u.w;
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Break an over-long unit into pieces that each fit `max_w`, never leaving a
/// closing punctuation character at the start of a piece.
fn split_unit(ctx: &Ctx, s: &str, max_w: f64, size: f64, ls: f64) -> Vec<(String, f64)> {
    let chars = ctx.chars(s, size, ls);
    let mut out: Vec<(String, f64)> = vec![];
    let mut buf = String::new();
    let mut w = 0.0f64;
    let mut i = 0usize;
    while i < chars.len() {
        let (ch, adv) = chars[i];
        if adv > max_w && buf.is_empty() {
            // a single character wider than the box: it gets a line to itself
            out.push((ch.to_string(), adv));
            i += 1;
            continue;
        }
        if w + adv > max_w && !buf.is_empty() {
            // pull forbidden leaders up into this piece, even if it overshoots
            let mut j = i;
            while j < chars.len() && (is_close_punct(chars[j].0) || is_cjk_small(chars[j].0)) {
                j += 1;
            }
            for k in i..j {
                buf.push(chars[k].0);
                w += chars[k].1;
            }
            out.push((std::mem::take(&mut buf), w));
            w = 0.0;
            i = j;
            continue;
        }
        if buf.is_empty() && !out.is_empty() && (is_close_punct(ch) || is_cjk_small(ch)) {
            if let Some(last) = out.last_mut() {
                last.0.push(ch);
                last.1 += adv;
                i += 1;
                continue;
            }
        }
        buf.push(ch);
        w += adv;
        i += 1;
    }
    if !buf.is_empty() {
        out.push((buf, w));
    }
    if out.is_empty() {
        out.push((s.to_string(), ctx.width(s, size, ls).0));
    }
    out
}

/// Move closing punctuation that ended up leading a line back up to the previous
/// line (real CJK typography; the product is Chinese-first, so this matters).
fn fix_orphans(lines: &mut Vec<String>) {
    let mut i = 1usize;
    while i < lines.len() {
        let mut guard = 0usize;
        while i < lines.len() && guard < 128 {
            guard += 1;
            let first = match lines[i].chars().next() {
                Some(c) => c,
                None => break,
            };
            let bad = is_close_punct(first) || is_cjk_small(first);
            if bad && !lines[i - 1].is_empty() {
                let rest: String = lines[i].chars().skip(1).collect();
                lines[i - 1].push(first);
                if rest.is_empty() {
                    lines.remove(i);
                } else {
                    lines[i] = rest;
                }
            } else {
                break;
            }
        }
        i += 1;
    }
}

// ------------------------------------------------------------------ character tables

/// Combining marks and format characters that occupy no horizontal space.
fn is_combining(ch: char) -> bool {
    const R: &[(u32, u32)] = &[
        (0x0300, 0x036F), (0x0483, 0x0489), (0x0591, 0x05BD), (0x05BF, 0x05BF), (0x05C1, 0x05C2),
        (0x05C4, 0x05C5), (0x05C7, 0x05C7), (0x0610, 0x061A), (0x064B, 0x065F), (0x0670, 0x0670),
        (0x06D6, 0x06DC), (0x06DF, 0x06E4), (0x06E7, 0x06E8), (0x06EA, 0x06ED), (0x0711, 0x0711),
        (0x0730, 0x074A), (0x07A6, 0x07B0), (0x07EB, 0x07F3), (0x0816, 0x0819), (0x081B, 0x0823),
        (0x0825, 0x0827), (0x0829, 0x082D), (0x0859, 0x085B), (0x08D3, 0x08FF), (0x093A, 0x093A),
        (0x093C, 0x093C), (0x0941, 0x0948), (0x094D, 0x094D), (0x0951, 0x0957), (0x0962, 0x0963),
        (0x0981, 0x0981), (0x09BC, 0x09BC), (0x09C1, 0x09C4), (0x09CD, 0x09CD), (0x09E2, 0x09E3),
        (0x0A01, 0x0A02), (0x0A3C, 0x0A3C), (0x0A41, 0x0A42), (0x0A47, 0x0A48), (0x0A4B, 0x0A4D),
        (0x0A70, 0x0A71), (0x0B01, 0x0B01), (0x0B3C, 0x0B3C), (0x0B3F, 0x0B3F), (0x0B41, 0x0B44),
        (0x0B4D, 0x0B4D), (0x0B82, 0x0B82), (0x0BC0, 0x0BC0), (0x0BCD, 0x0BCD), (0x0C00, 0x0C00),
        (0x0C3E, 0x0C40), (0x0C46, 0x0C48), (0x0C4A, 0x0C4D), (0x0CBC, 0x0CBC), (0x0CBF, 0x0CBF),
        (0x0CC6, 0x0CC6), (0x0CCC, 0x0CCD), (0x0D01, 0x0D01), (0x0D41, 0x0D44), (0x0D4D, 0x0D4D),
        (0x0DCA, 0x0DCA), (0x0DD2, 0x0DD4), (0x0DD6, 0x0DD6), (0x0E31, 0x0E31), (0x0E34, 0x0E3A),
        (0x0E47, 0x0E4E), (0x0EB1, 0x0EB1), (0x0EB4, 0x0EBC), (0x0EC8, 0x0ECD), (0x0F18, 0x0F19),
        (0x0F35, 0x0F35), (0x0F37, 0x0F37), (0x0F39, 0x0F39), (0x0F71, 0x0F7E), (0x0F80, 0x0F84),
        (0x0F86, 0x0F87), (0x0F8D, 0x0F97), (0x0F99, 0x0FBC), (0x0FC6, 0x0FC6), (0x102D, 0x1030),
        (0x1032, 0x1037), (0x1039, 0x103A), (0x103D, 0x103E), (0x1058, 0x1059), (0x105E, 0x1060),
        (0x1071, 0x1074), (0x1082, 0x1082), (0x1085, 0x1086), (0x108D, 0x108D), (0x135D, 0x135F),
        (0x1712, 0x1714), (0x1732, 0x1734), (0x1752, 0x1753), (0x1772, 0x1773), (0x17B4, 0x17B5),
        (0x17B7, 0x17BD), (0x17C6, 0x17C6), (0x17C9, 0x17D3), (0x17DD, 0x17DD), (0x180B, 0x180D),
        (0x18A9, 0x18A9), (0x1920, 0x1922), (0x1927, 0x1928), (0x1932, 0x1932), (0x1939, 0x193B),
        (0x1A17, 0x1A18), (0x1A1B, 0x1A1B), (0x1A56, 0x1A56), (0x1A58, 0x1A5E), (0x1A60, 0x1A60),
        (0x1A62, 0x1A62), (0x1A65, 0x1A6C), (0x1A73, 0x1A7C), (0x1A7F, 0x1A7F), (0x1AB0, 0x1AFF),
        (0x1B00, 0x1B03), (0x1B34, 0x1B34), (0x1B36, 0x1B3A), (0x1B3C, 0x1B3C), (0x1B42, 0x1B42),
        (0x1B6B, 0x1B73), (0x1B80, 0x1B81), (0x1BA2, 0x1BA5), (0x1BA8, 0x1BA9), (0x1BAB, 0x1BAD),
        (0x1BE6, 0x1BE6), (0x1BE8, 0x1BE9), (0x1BED, 0x1BED), (0x1BEF, 0x1BF1), (0x1C2C, 0x1C33),
        (0x1C36, 0x1C37), (0x1CD0, 0x1CD2), (0x1CD4, 0x1CE0), (0x1CE2, 0x1CE8), (0x1CED, 0x1CED),
        (0x1CF4, 0x1CF4), (0x1DC0, 0x1DFF), (0x20D0, 0x20F0), (0x2CEF, 0x2CF1), (0x2D7F, 0x2D7F),
        (0x2DE0, 0x2DFF), (0x302A, 0x302D), (0x3099, 0x309C), (0xA66F, 0xA672), (0xA674, 0xA67D),
        (0xA69E, 0xA69F), (0xA6F0, 0xA6F1), (0xA802, 0xA802), (0xA806, 0xA806), (0xA80B, 0xA80B),
        (0xA823, 0xA827), (0xA880, 0xA881), (0xA8B4, 0xA8C4), (0xA8E0, 0xA8F1), (0xA926, 0xA92D),
        (0xA947, 0xA951), (0xA980, 0xA982), (0xA9B3, 0xA9B3), (0xA9B6, 0xA9B9), (0xA9BC, 0xA9BD),
        (0xA9E5, 0xA9E5), (0xAA29, 0xAA2E), (0xAA31, 0xAA32), (0xAA35, 0xAA36), (0xAA43, 0xAA43),
        (0xAA4C, 0xAA4C), (0xAA7C, 0xAA7C), (0xAAB0, 0xAAB0), (0xAAB2, 0xAAB4), (0xAAB7, 0xAAB8),
        (0xAABE, 0xAABF), (0xAAC1, 0xAAC1), (0xAAEC, 0xAAED), (0xAAF6, 0xAAF6), (0xABE5, 0xABE5),
        (0xABE8, 0xABE8), (0xABED, 0xABED), (0xFB1E, 0xFB1E), (0xFE00, 0xFE0F), (0xFE20, 0xFE2F),
        (0x101FD, 0x101FD), (0x102E0, 0x102E0), (0x10376, 0x1037A), (0x10A01, 0x10A0F),
        (0x10A38, 0x10A3F), (0x10AE5, 0x10AE6), (0x11001, 0x11001), (0x11038, 0x11046),
        (0x1107F, 0x11081), (0x110B3, 0x110B6), (0x110B9, 0x110BA), (0x11100, 0x11102),
        (0x11127, 0x1112B), (0x1112D, 0x11134), (0x11173, 0x11173), (0x11180, 0x11181),
        (0x111B6, 0x111BE), (0x1122F, 0x11231), (0x11234, 0x11234), (0x11236, 0x11237),
        (0x112DF, 0x112DF), (0x112E3, 0x112E8), (0x114B3, 0x114B8), (0x114BA, 0x114BA),
        (0x114BF, 0x114C0), (0x114C2, 0x114C3), (0x115B2, 0x115B5), (0x115BC, 0x115BD),
        (0x115BF, 0x115C0), (0x11630, 0x11632), (0x1163E, 0x1163E), (0x116D7, 0x116D8),
        (0x11725, 0x11729), (0x16AF0, 0x16AF4), (0x16B30, 0x16B36), (0x1B000, 0x1B000),
        (0x1BC9D, 0x1BC9E), (0x1CF00, 0x1CF46), (0x1D167, 0x1D169), (0x1D173, 0x1D182),
        (0x1D185, 0x1D18B), (0x1D1AA, 0x1D1AD), (0x1D242, 0x1D244), (0x1DA00, 0x1DA36),
        (0x1DA3B, 0x1DA6C), (0x1DA9B, 0x1DAAF), (0x1E8D0, 0x1E8D6), (0xE0100, 0xE01EF),
    ];
    range_hit(R, ch)
}

fn range_hit(ranges: &[(u32, u32)], ch: char) -> bool {
    let c = ch as u32;
    let mut lo = 0usize;
    let mut hi = ranges.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        let (a, b) = ranges[mid];
        if c < a {
            hi = mid;
        } else if c > b {
            lo = mid + 1;
        } else {
            return true;
        }
    }
    false
}

/// Full-width characters: Han, kana, Hangul, CJK symbols, fullwidth forms.
fn is_wide(ch: char) -> bool {
    const R: &[(u32, u32)] = &[
        (0x1100, 0x115F), (0x2E80, 0x303E), (0x3041, 0x33FF), (0x3400, 0x4DBF), (0x4E00, 0x9FFF),
        (0xA000, 0xA4CF), (0xA960, 0xA97F), (0xAC00, 0xD7A3), (0xF900, 0xFAFF), (0xFE10, 0xFE19),
        (0xFE30, 0xFE6F), (0xFF01, 0xFF60), (0xFFE0, 0xFFE6), (0x1B000, 0x1B16F), (0x1F200, 0x1F2FF),
        (0x20000, 0x3134F),
    ];
    range_hit(R, ch)
}

/// Characters that begin their own break unit (CJK body text).
fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x3041..=0x30FF | 0x31F0..=0x31FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF |
        0x20000..=0x2FA1F | 0xAC00..=0xD7A3 | 0x1100..=0x11FF | 0x3130..=0x318F)
}

/// Small kana / iteration marks: may never lead a line.
fn is_cjk_small(ch: char) -> bool {
    matches!(ch,
        'ぁ' | 'ぃ' | 'ぅ' | 'ぇ' | 'ぉ' | 'っ' | 'ゃ' | 'ゅ' | 'ょ' | 'ゕ' | 'ゖ' |
        'ァ' | 'ィ' | 'ゥ' | 'ェ' | 'ォ' | 'ッ' | 'ャ' | 'ュ' | 'ョ' | 'ヶ' |
        'ー' | 'ヽ' | 'ヾ' | '々' | '〆' | '・')
}

/// Closing punctuation: never the first character of a line. Includes the set the
/// contract names — `)]}，。！？、;:》” …` — plus the usual western and fullwidth
/// equivalents.
fn is_close_punct(ch: char) -> bool {
    // `「` deliberately does NOT appear here: it is an opener (see `is_open_punct`)
    // and listing it in both sets made every line break before a quotation mark illegal.
    const CLOSE: &str = "\
        !.,:;?)]}%\"'\u{2013}\u{2014}\u{2019}\u{201D}\u{2021}\u{2026}\u{2027}\u{203A}\u{203C}\u{2047}\u{2048}\u{2049}\u{2030}\
        \u{3001}\u{3002}\u{3003}\u{3005}\u{3006}\u{3007}\u{3009}\u{300B}\u{300D}\u{300F}\u{3011}\u{3015}\u{3017}\u{3019}\u{303F}\u{30FB}\u{30FC}\u{30FD}\u{30FE}\
        \u{FE50}\u{FE52}\u{FE54}\u{FE55}\u{FE56}\u{FE57}\u{FE5B}\u{FE5E}\u{FE60}\u{FE61}\u{FE63}\
        \u{FF01}\u{FF02}\u{FF05}\u{FF07}\u{FF09}\u{FF0C}\u{FF0E}\u{FF1A}\u{FF1B}\u{FF1F}\u{FF20}\u{FF3D}\u{FF5D}\u{FF62}\u{FF64}\u{FF65}";
    CLOSE.contains(ch)
}

/// Prefix punctuation: a line may not end with these, because the number or quote
/// they introduce belongs with them (UAX #14 LB25 / PR × ID).
fn is_prefix_punct(ch: char) -> bool {
    matches!(
        ch,
        '$' | '\u{00A5}' | '\u{20AC}' | '\u{00A3}' | '\u{FFE5}' | '\u{FF04}' | '\u{FFE5}' | '#' | '\u{2039}'
    ) || matches!(ch, '\u{300C}' | '\u{3008}' | '\u{300A}' | '\u{300E}' | '\u{3010}' | '\u{3014}')
}

/// Opening punctuation: never the last character of a line.
fn is_open_punct(ch: char) -> bool {
    const OPEN: &str =
        "([{\u{2018}\u{201C}\u{3008}\u{300A}\u{300C}\u{300E}\u{3010}\u{3014}\u{3016}\u{3018}\u{FE5F}\u{FF08}\u{FF3B}\u{FF5B}\u{FF5F}\u{FF61}";
    OPEN.contains(ch)
}

fn is_hyphen(ch: char) -> bool {
    matches!(ch, '-' | '\u{2010}' | '\u{2013}' | '\u{00AD}')
}

// ------------------------------------------------------------------ xml

/// Escape for text content and attribute values. Deterministic, never panics.
pub fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\r' => {}
            c if (c as u32) < 0x20 && c != '\n' && c != '\t' => out.push_str(&format!("&#x{:x};", c as u32)),
            c => out.push(c),
        }
    }
    out
}

// ------------------------------------------------------------------ font dirs

fn font_dirs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = vec![];
    if let Some(root) = std::env::var_os("SYSTEMROOT").or_else(|| std::env::var_os("WINDIR")) {
        v.push(Path::new(&root).join("Fonts"));
    }
    v.push(PathBuf::from("C:\\Windows\\Fonts"));
    if let Some(d) = std::env::var_os("LOCALAPPDATA") {
        v.push(Path::new(&d).join("Microsoft\\Windows\\Fonts"));
    }
    if let Some(d) = std::env::var_os("USERPROFILE") {
        v.push(Path::new(&d).join("AppData\\Local\\Microsoft\\Windows\\Fonts"));
        v.push(Path::new(&d).join("AppData\\Roaming\\Microsoft\\Windows\\Fonts"));
    }
    if let Some(d) = std::env::var_os("AGENTCANVAS_FONT_DIR") {
        v.extend(std::env::split_paths(&d));
    }
    v.push(PathBuf::from("/usr/share/fonts"));
    v.push(PathBuf::from("/usr/local/share/fonts"));
    if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
        v.push(Path::new(&d).join("fonts"));
    }
    if let Some(d) = std::env::var_os("HOME") {
        v.push(Path::new(&d).join(".fonts"));
        v.push(Path::new(&d).join("Library/Fonts"));
    }
    v.push(PathBuf::from("/Library/Fonts"));
    v.push(PathBuf::from("/System/Library/Fonts"));

    let mut seen = HashSet::new();
    v.retain(|p| seen.insert(p.to_string_lossy().to_lowercase()));
    v
}

fn collect_font_files(rd: std::fs::ReadDir, depth: u32, out: &mut Vec<PathBuf>) {
    for entry in rd.flatten() {
        let path = entry.path();
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if ft.is_dir() {
            if depth < 4 {
                if let Ok(sub) = std::fs::read_dir(&path) {
                    collect_font_files(sub, depth + 1, out);
                }
            }
            continue;
        }
        if !ft.is_file() {
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        if matches!(ext.as_str(), "ttf" | "otf" | "ttc" | "otc") {
            out.push(path);
        }
    }
}

// ------------------------------------------------------------------ true-type tables

#[inline]
fn b_u16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
#[inline]
fn b_i16(b: &[u8], at: usize) -> Option<i16> {
    Some(i16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
#[inline]
fn b_u32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
#[inline]
fn b_i32(b: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn table_bytes<'a>(data: &'a [u8], start: usize, tag: &[u8; 4]) -> Option<&'a [u8]> {
    let num = b_u16(data, start.checked_add(4)?)? as usize;
    for i in 0..num {
        let rec = start.checked_add(12)?.checked_add(i.checked_mul(16)?)?;
        if data.get(rec..rec + 4)? == *tag {
            let off = b_u32(data, rec + 8)? as usize;
            let len = b_u32(data, rec + 12)? as usize;
            return data.get(off..off.checked_add(len)?);
        }
    }
    None
}

/// Offset of face `index` inside a font file; handles `ttcf` collections.
fn face_start(data: &[u8], index: u32) -> Option<usize> {
    let tag = data.get(0..4)?;
    if tag == b"ttcf" {
        let n = b_u32(data, 8)? as usize;
        let i = index as usize;
        if i >= n {
            return None;
        }
        b_u32(data, 12 + i.checked_mul(4)?).map(|o| o as usize)
    } else if index == 0 {
        Some(0)
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug)]
struct CmapRange {
    lo: u32,
    hi: u32,
    first: u32,
    linear: bool,
}

/// Glyph metrics for one face, read straight out of the font file.
#[derive(Clone, Debug)]
pub struct Metrics {
    pub units_per_em: f64,
    /// ascent in font units (positive)
    pub ascender: f64,
    /// descent in font units (normally negative)
    pub descender: f64,
    pub line_gap: f64,
    pub cap_height: f64,
    pub x_height: f64,
    pub italic_angle: f64,
    pub num_glyphs: u32,
    ranges: Vec<CmapRange>,
    sparse: Vec<(u32, u32)>,
    advances: Vec<f64>,
}

impl Metrics {
    /// Parse `head`, `hhea`, `maxp`, `hmtx`, `cmap`, `OS/2`, `post`.
    pub fn parse(data: &[u8], index: u32) -> Option<Metrics> {
        let start = face_start(data, index)?;
        let head = table_bytes(data, start, b"head")?;
        let hhea = table_bytes(data, start, b"hhea")?;
        let maxp = table_bytes(data, start, b"maxp")?;
        let cmap = table_bytes(data, start, b"cmap")?;
        let hmtx = table_bytes(data, start, b"hmtx")?;
        let os2 = table_bytes(data, start, b"OS/2");
        let post = table_bytes(data, start, b"post");

        let upem_raw = b_u16(head, 18)?;
        let units_per_em = if upem_raw == 0 { 1000.0 } else { upem_raw as f64 };
        let num_glyphs = b_u16(maxp, 4)? as u32;
        let num_h = (b_u16(hhea, 34)? as u32).clamp(1, num_glyphs.max(1));

        let mut ascender = b_i16(hhea, 4)? as f64;
        let mut descender = b_i16(hhea, 6)? as f64;
        let mut line_gap = b_i16(hhea, 8)?.max(0) as f64;
        let mut cap_height = 0.0;
        let mut x_height = 0.0;
        if let Some(o) = os2 {
            let v = b_u16(o, 0).unwrap_or(0);
            let ta = b_i16(o, 68).unwrap_or(0) as f64;
            let td = b_i16(o, 70).unwrap_or(0) as f64;
            let tg = b_i16(o, 72).unwrap_or(0).max(0) as f64;
            if ta > 0.0 && td < 0.0 {
                ascender = ta;
                descender = td;
                line_gap = tg;
            } else if ta > 0.0 && ascender <= 0.0 {
                ascender = ta;
            }
            if v >= 2 {
                cap_height = b_i16(o, 88).unwrap_or(0) as f64;
                x_height = b_i16(o, 86).unwrap_or(0) as f64;
            }
        }
        let italic_angle = post.map(|p| b_i32(p, 4).unwrap_or(0) as f64 / 65536.0).unwrap_or(0.0);

        let mut advances: Vec<f64> = Vec::with_capacity(num_glyphs as usize);
        let last_h = b_u16(hmtx, (num_h as usize - 1) * 4).unwrap_or(0) as f64;
        for gid in 0..num_glyphs {
            let adv = if gid < num_h { b_u16(hmtx, gid as usize * 4).unwrap_or(0) as f64 } else { last_h };
            advances.push(adv);
        }

        let (ranges, sparse) = parse_cmap(cmap)?;

        Some(Metrics {
            units_per_em,
            ascender,
            descender,
            line_gap,
            cap_height,
            x_height,
            italic_angle,
            num_glyphs,
            ranges,
            sparse,
            advances,
        })
    }

    /// `ch` -> glyph id, when this face draws it.
    pub fn glyph_of(&self, ch: char) -> Option<u32> {
        let c = ch as u32;
        let mut lo = 0usize;
        let mut hi = self.ranges.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            let r = self.ranges[mid];
            if c < r.lo {
                hi = mid;
            } else if c > r.hi {
                lo = mid + 1;
            } else {
                let gid = if r.linear { r.first + (c - r.lo) } else { r.first };
                return if gid == 0 || gid >= self.num_glyphs { None } else { Some(gid) };
            }
        }
        let mut lo = 0usize;
        let mut hi = self.sparse.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            let (k, g) = self.sparse[mid];
            if c < k {
                hi = mid;
            } else if c > k {
                lo = mid + 1;
            } else {
                return if g == 0 || g >= self.num_glyphs { None } else { Some(g) };
            }
        }
        None
    }

    /// Advance of `ch` in ems, `None` when there is no glyph.
    pub fn advance_em(&self, ch: char) -> Option<f64> {
        if is_combining(ch) {
            return Some(0.0);
        }
        let gid = self.glyph_of(ch)?;
        Some(self.advances.get(gid as usize).copied().unwrap_or(0.0) / self.units_per_em)
    }

    /// The `.notdef` box in ems — what an unmapped character costs.
    pub fn notdef_em(&self) -> f64 {
        self.advances.first().copied().unwrap_or(0.0) / self.units_per_em
    }

    pub fn advance_px(&self, ch: char, size: f64) -> Option<f64> {
        self.advance_em(ch).map(|e| e * size)
    }
    pub fn ascender_em(&self) -> f64 {
        self.ascender / self.units_per_em
    }
    pub fn descender_em(&self) -> f64 {
        self.descender / self.units_per_em
    }
    pub fn line_gap_em(&self) -> f64 {
        self.line_gap / self.units_per_em
    }
    pub fn cap_height_em(&self) -> f64 {
        if self.cap_height > 0.0 { self.cap_height / self.units_per_em } else { 0.7 }
    }
    pub fn x_height_em(&self) -> f64 {
        if self.x_height > 0.0 { self.x_height / self.units_per_em } else { 0.5 }
    }
    /// `true` when the face can draw Han — the missing-Chinese-font check.
    pub fn has_cjk(&self) -> bool {
        self.glyph_of('中').is_some()
    }
}

/// Pick the best `cmap` subtable and expand it into sorted, non-shadowed ranges.
fn parse_cmap(t: &[u8]) -> Option<(Vec<CmapRange>, Vec<(u32, u32)>)> {
    let num = b_u16(t, 2)? as usize;
    let mut best: Option<(u32, usize)> = None;
    for i in 0..num.min(64) {
        let rec = 4 + i.checked_mul(8)?;
        let pid = b_u16(t, rec)?;
        let eid = b_u16(t, rec + 2)?;
        let off = b_u32(t, rec + 4)? as usize;
        let score = match (pid, eid) {
            (3, 10) => 100,
            (0, 6) | (0, 4) => 95,
            (3, 1) => 90,
            (0, 3) | (0, 2) | (0, 1) | (0, 0) => 80,
            (3, 0) => 70,
            (1, 0) | (1, 1) | (1, 3) => 60,
            (2, 0) | (2, 1) => 50,
            _ => 10,
        };
        if off + 4 <= t.len() && best.map(|(s, _)| score > s).unwrap_or(true) {
            best = Some((score, off));
        }
    }
    let off = match best {
        Some((_, o)) => o,
        None => return None,
    };
    let fmt = b_u16(t, off)?;
    let mut ranges: Vec<CmapRange> = vec![];
    let mut sparse: Vec<(u32, u32)> = vec![];
    match fmt {
        0 => {
            let n = (b_u16(t, off + 6).unwrap_or(256) as usize).min(256);
            for c in 0..n {
                let g = *t.get(off + 8 + c)? as u32;
                if g != 0 {
                    sparse.push((c as u32, g));
                }
            }
        }
        6 => {
            let first = b_u16(t, off + 6)? as u32;
            let cnt = (b_u16(t, off + 8)? as usize).min(65536);
            for i in 0..cnt {
                let g = b_u16(t, off + 10 + i * 2)? as u32;
                if g != 0 {
                    sparse.push((first + i as u32, g));
                }
            }
        }
        4 => {
            let seg2 = b_u16(t, off + 6)? as usize;
            let seg = seg2 / 2;
            let ends = off + 8;
            let starts = ends + seg2 + 2;
            let deltas = starts + seg2;
            let roff = deltas + seg2;
            for i in 0..seg.min(1024) {
                let e = b_u16(t, ends + i * 2)? as u32;
                let st = b_u16(t, starts + i * 2)? as u32;
                let d = b_i16(t, deltas + i * 2)? as i32;
                let ro = b_u16(t, roff + i * 2)? as usize;
                if st == 0xFFFF {
                    continue;
                }
                if ro == 0 {
                    let first = ((st as i32 + d) & 0xFFFF) as u32;
                    if first != 0 {
                        ranges.push(CmapRange { lo: st, hi: e, first, linear: true });
                    }
                } else {
                    let here = roff + i * 2;
                    for c in st..=e.min(st + 8192) {
                        let addr = here + ro + ((c - st) as usize) * 2;
                        let g = b_u16(t, addr).unwrap_or(0) as u32;
                        if g == 0 {
                            continue;
                        }
                        sparse.push((c, ((g as i32 + d) & 0xFFFF) as u32));
                    }
                }
            }
        }
        12 | 13 => {
            let n = (b_u32(t, off + 12)? as usize).min(200_000);
            for i in 0..n {
                let at = off + 16 + i * 12;
                let s = b_u32(t, at)?;
                let e = b_u32(t, at + 4)?;
                let g = b_u32(t, at + 8)?;
                if e < s || g == 0 {
                    continue;
                }
                ranges.push(CmapRange { lo: s, hi: e, first: g, linear: fmt != 13 });
            }
        }
        _ => return None,
    }
    ranges.sort_by(|a, b| a.lo.cmp(&b.lo).then(a.hi.cmp(&b.hi)));
    // A range fully inside an earlier one is shadowed; drop it so lookup is
    // unambiguous and binary search stays valid.
    let mut kept: Vec<CmapRange> = vec![];
    for r in ranges {
        if kept.iter().any(|k| k.lo <= r.lo && r.hi <= k.hi) {
            continue;
        }
        kept.push(r);
    }
    sparse.sort_unstable();
    sparse.dedup_by(|a, b| a.0 == b.0);
    if kept.is_empty() && sparse.is_empty() {
        return None;
    }
    Some((kept, sparse))
}
