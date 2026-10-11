//! AgentCanvas core.
//!
//! One engine, three doors: the desktop webview, the local HTTP API and the MCP
//! stdio server all call [`rpc::call`], so an agent and a human can never drift
//! apart. The scene graph in [`doc`] is the only truth; SVG ([`svg`]) is the only
//! paint path, and [`render`] rasterises that same SVG for export.

pub mod assets;
pub mod commands;
pub mod doc;
pub mod error;
pub mod geom;
pub mod history;
pub mod http;
pub mod imageops;
pub mod inspect;
pub mod jobs;
pub mod layout;
pub mod pdf;
pub mod project;
pub mod provider;
pub mod render;
pub mod rpc;
pub mod session;
pub mod svg;
pub mod templates;
pub mod text;
pub mod util;

pub use error::{Fail, R};

/// Crate version, reported by `system.version` and `caps`.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Port of the in-process HTTP server; 0 when this process is not serving.
pub fn http_port() -> u16 {
    http::port()
}
