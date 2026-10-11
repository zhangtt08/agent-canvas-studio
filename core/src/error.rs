//! Errors are machine-readable first: every failure carries a stable `code` and a
//! `hint` that is usually enough to fix the call on the next attempt, so an agent
//! does not spend a round-trip asking what went wrong.

use serde_json::{json, Value};

pub type R<T> = Result<T, Fail>;

#[derive(Debug, Clone)]
pub struct Fail {
    pub code: &'static str,
    pub msg: String,
    pub hint: Option<String>,
    pub details: Option<Value>,
    pub retryable: bool,
}

impl Fail {
    pub fn new(code: &'static str, msg: impl Into<String>) -> Self {
        Self { code, msg: msg.into(), hint: None, details: None, retryable: false }
    }
    pub fn hint(mut self, h: impl Into<String>) -> Self {
        self.hint = Some(h.into());
        self
    }
    pub fn details(mut self, d: Value) -> Self {
        self.details = Some(d);
        self
    }
    pub fn retry(mut self) -> Self {
        self.retryable = true;
        self
    }
    pub fn to_value(&self) -> Value {
        let mut v = json!({ "e": self.msg, "code": self.code });
        if let Some(h) = &self.hint {
            v["hint"] = json!(h);
        }
        if let Some(d) = &self.details {
            v["details"] = d.clone();
        }
        if self.retryable {
            v["retryable"] = json!(true);
        }
        v
    }
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.msg)
    }
}

impl std::error::Error for Fail {}

impl From<std::io::Error> for Fail {
    fn from(e: std::io::Error) -> Self {
        Fail::new("io", e.to_string())
    }
}

/// `serde_json::to_value`/`from_value` show up all over the RPC layer; a JSON
/// shape problem is always a schema problem, so one conversion keeps the call
/// sites free of `.map_err(...)`.
impl From<serde_json::Error> for Fail {
    fn from(e: serde_json::Error) -> Self {
        Fail::new("INVALID_SCHEMA", format!("json: {e}"))
    }
}

/// Stable codes. Keep this list aligned with AGENTS.md; agents switch on it.
pub const CODES: &[(&str, &str)] = &[
    ("INVALID_SCHEMA", "argument missing or of the wrong shape"),
    ("INVALID_VALUE", "argument present but out of range"),
    ("PROJECT_NOT_FOUND", "no project with that id"),
    ("ARTBOARD_NOT_FOUND", "no artboard with that id"),
    ("NODE_NOT_FOUND", "no node with that id"),
    ("ASSET_NOT_FOUND", "asset id or path unresolved"),
    ("ASSET_MISSING", "asset file is gone from disk"),
    ("FONT_MISSING", "requested font is not installed"),
    ("LAYOUT_CONFLICT", "constraints cannot all be satisfied"),
    ("LOCKED", "node is locked and was not modified"),
    ("REVISION_CONFLICT", "expectedRevision is stale, re-read then retry"),
    ("PATH_DENIED", "path is outside the allowed root"),
    ("PERMISSION_DENIED", "operation not allowed"),
    ("UNSUPPORTED", "format or capability not available in this build"),
    ("RENDER_FAILED", "rasteriser failed"),
    ("DECODE_FAILED", "image bytes could not be decoded"),
    ("TOO_LARGE", "input exceeds configured limits"),
    ("JOB_NOT_FOUND", "no such job id"),
    ("JOB_TIMEOUT", "job exceeded its time budget"),
    ("CANCELLED", "job was cancelled"),
    ("PROVIDER_NOT_CONFIGURED", "an AI provider is required but none is configured"),
    ("IO", "filesystem error"),
    ("NOT_FOUND", "nothing at that path"),
    ("INTERNAL", "unexpected engine error"),
];
