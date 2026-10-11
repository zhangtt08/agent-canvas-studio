//! Provider reachability.
//!
//! The product's own capabilities — layout, typography, compositing, export,
//! inspection — are deterministic and need no key. A provider is only ever an
//! *optional* extra (AI cutout, image generation, the panel's free-form agent), and
//! the contract is that an unconfigured provider answers
//! `PROVIDER_NOT_CONFIGURED` rather than pretending.
//!
//! This module answers the one question that can be settled without a model client:
//! **is the endpoint you configured actually there?** It speaks plain HTTP/1.1 with
//! the standard library only — no TLS stack is vendored, so `https://` is reported as
//! unsupported instead of silently failing. That is enough for the local servers
//! people actually run beside a desktop app (Ollama, llama.cpp, LM Studio, vLLM on
//! loopback), and it keeps the dependency tree inside the image-processing product.
//!
//! Secrets: the key is read from the environment variable *named* by
//! `provider.apiKeyEnv` and is only ever placed in a request header. It is never
//! returned, logged or written to disk.

use crate::error::{Fail, R};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// Where a probe will actually knock.
struct Endpoint {
    host: String,
    port: u16,
    path: String,
    tls: bool,
}

/// `http://host:port/prefix` → the model list path that server really serves.
///
/// Ollama answers `/api/tags`; anything already carrying a `/v1` prefix answers
/// `/models`; a bare host gets the OpenAI-compatible `/v1/models`. Guessing wrong is
/// a 404 that looks like "no provider", so the shape is chosen from the base the user
/// typed rather than from a fixed default.
fn models_path(base: &str, name: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    let lower = name.to_lowercase();
    let without_scheme = trimmed.split_once("://").map(|(_, r)| r.to_string()).unwrap_or_else(|| trimmed.to_string());
    if lower.contains("ollama") || without_scheme.contains(":11434") {
        return format!("{trimmed}/api/tags");
    }
    if without_scheme.ends_with("/v1") {
        return format!("{trimmed}/models");
    }
    format!("{trimmed}/v1/models")
}

fn split(url: &str) -> R<Endpoint> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| Fail::new("INVALID_VALUE", format!("provider.baseUrl \"{url}\" has no scheme")))?;
    let tls = scheme.eq_ignore_ascii_case("https");
    if !tls && !scheme.eq_ignore_ascii_case("http") {
        return Err(Fail::new("INVALID_VALUE", format!("unsupported scheme \"{scheme}\""))
            .hint("provider.baseUrl must be http:// or https://"));
    }
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (h.to_string(), p.parse::<u16>().unwrap_or(80)),
        _ => (authority.to_string(), if tls { 443 } else { 80 }),
    };
    if host.trim().is_empty() {
        return Err(Fail::new("INVALID_VALUE", "provider.baseUrl has an empty host"));
    }
    Ok(Endpoint { host, port, path: format!("/{path}"), tls })
}

/// Ask the configured provider for its model list.
///
/// `cfg` is the merged settings document; `timeout_ms` bounds connect *and* read.
pub fn probe(cfg: &Value, timeout_ms: u64) -> R<Value> {
    let prov = cfg.get("provider").cloned().unwrap_or(json!({}));
    let base = prov
        .get("baseUrl")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            Fail::new("PROVIDER_NOT_CONFIGURED", "provider.baseUrl is empty")
                .hint("set provider.baseUrl to an OpenAI-compatible or Ollama server, e.g. http://127.0.0.1:11434")
        })?;
    let url = models_path(&base, prov.get("name").and_then(|v| v.as_str()).unwrap_or(""));
    if url.starts_with("https://") {
        return Err(Fail::new("NETWORK", "this build probes plain http only; no TLS stack is bundled")
            .hint("point provider.baseUrl at a local http:// endpoint, or front the provider with a local proxy"));
    }
    let ep = split(&url)?;
    let timeout = Duration::from_millis(timeout_ms.max(1));
    let t0 = Instant::now();

    let mut sock = {
        let addrs = (ep.host.as_str(), ep.port)
            .to_socket_addrs()
            .map_err(|e| Fail::new("NETWORK", format!("cannot resolve {}:{} ({e})", ep.host, ep.port)))?;
        let mut last: Option<std::io::Error> = None;
        let mut opened = None;
        for a in addrs {
            match TcpStream::connect_timeout(&a, timeout) {
                Ok(s) => {
                    opened = Some(s);
                    break;
                }
                Err(e) => last = Some(e),
            }
        }
        opened.ok_or_else(|| {
            Fail::new("NETWORK", format!("{}:{} unreachable ({})", ep.host, ep.port, last.map(|e| e.to_string()).unwrap_or_default()))
        })?
    };
    let _ = sock.set_read_timeout(Some(timeout));
    let _ = sock.set_write_timeout(Some(timeout));

    let key_env = prov
        .get("apiKeyEnv")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("AGENTCANVAS_API_KEY")
        .to_string();
    let key = std::env::var(&key_env).ok().filter(|s| !s.trim().is_empty());
    let mut req = format!(
        "GET {} HTTP/1.1\r\nHost: {}:{}\r\nAccept: application/json\r\nUser-Agent: agentcanvas/{}\r\nConnection: close\r\n",
        ep.path,
        ep.host,
        ep.port,
        crate::version()
    );
    if let Some(k) = &key {
        // Header only. The value never appears in any reply or log line.
        req.push_str(&format!("Authorization: Bearer {}\r\n", k.replace(['\r', '\n'], " ")));
    }
    req.push_str("\r\n");
    sock.write_all(req.as_bytes()).map_err(|e| Fail::new("NETWORK", format!("send failed: {e}")))?;

    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = sock.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        // A model list is small; a hostile or misconfigured endpoint must not be able
        // to make this buffer the desktop's memory.
        if buf.len() > 1 << 20 {
            return Err(Fail::new("NETWORK", "provider reply exceeded 1 MiB"));
        }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let (headers, body) = split_reply(&text);
    let status = headers
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let models = model_names(&parsed);
    let ms = (Instant::now() - t0).as_millis() as u64;
    if status < 200 || status > 299 {
        return Err(Fail::new("NETWORK", format!("provider answered {status} for {url}"))
            .details(json!({"status": status, "reachable": true, "ms": ms}))
            .hint("check the base URL prefix: Ollama uses /api/tags, OpenAI-compatible servers use /v1/models"));
    }
    Ok(json!({
        "ok": true,
        "url": url,
        "status": status,
        "ms": ms,
        "auth": if key.is_some() { "header sent" } else { "none" },
        "n_models": models.len(),
        "models": models.iter().take(8).cloned().collect::<Vec<_>>(),
        "parsed": !parsed.is_null(),
    }))
}

/// Split headers from body on the blank line; a body may legitimately contain one.
fn split_reply(raw: &str) -> (String, String) {
    match raw.split_once("\r\n\r\n") {
        Some((h, b)) => (h.to_string(), b.to_string()),
        None => (raw.to_string(), String::new()),
    }
}

/// Both reply shapes: OpenAI's `{data:[{id}]}` and Ollama's `{models:[{name}]}`.
fn model_names(v: &Value) -> Vec<String> {
    let list = v
        .get("data")
        .or_else(|| v.get("models"))
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();
    list.iter()
        .filter_map(|m| {
            m.get("id")
                .or_else(|| m.get("name"))
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider server we control, so the positive path is really exercised.
    fn stub(reply: &'static str) -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for got in l.incoming() {
                let mut s = match got {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let mut buf = [0u8; 1024];
                let n = s.read(&mut buf).unwrap_or(0);
                let asked = String::from_utf8_lossy(&buf[..n]).to_string();
                // The two server families are reached at different paths; serve whichever
                // the request actually named, so a wrong route shows up as a miss.
                let body = if asked.starts_with("GET /api/tags") {
                    r#"{"models":[{"name":"llama3:8b"}]}"#
                } else if asked.starts_with("GET /v1/models") || asked.starts_with("GET /models") {
                    reply
                } else {
                    continue;
                };
                let _ = s.write_all(
                    format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len())
                        .as_bytes(),
                );
                let _ = s.flush();
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
        });
        port
    }

    #[test]
    fn an_openai_compatible_endpoint_reports_its_models() {
        let port = stub(r#"{"object":"list","data":[{"id":"flux"},{"id":"sdxl"}]}"#);
        let cfg = json!({"provider": {"name": "openai", "baseUrl": format!("http://127.0.0.1:{port}")}});
        let r = probe(&cfg, 3000).unwrap();
        assert_eq!(r["status"], 200);
        assert_eq!(r["n_models"], 2, "{r}");
        assert!(r["models"].as_array().unwrap().iter().any(|m| m == "flux"), "{r}");
        assert_eq!(r["url"], format!("http://127.0.0.1:{port}/v1/models"));
        assert!(r["ms"].as_u64().unwrap() < 3000);
    }

    #[test]
    fn ollama_is_asked_for_its_own_route() {
        let port = stub("{}");
        let cfg = json!({"provider": {"name": "ollama", "baseUrl": format!("http://127.0.0.1:{port}")}});
        let r = probe(&cfg, 3000).unwrap();
        assert!(r["url"].as_str().unwrap().ends_with("/api/tags"), "{}", r["url"]);
        assert_eq!(r["n_models"], 1, "{r}");
    }

    #[test]
    fn an_empty_or_unreachable_base_url_is_named_not_guessed() {
        let err = probe(&json!({"provider": {"name": ""}}), 500).unwrap_err();
        assert_eq!(err.code, "PROVIDER_NOT_CONFIGURED");
        let hint = err.hint.clone().unwrap_or_default();
        assert!(hint.contains("baseUrl"), "{hint}");

        // Port 1 is closed on every loopback.
        let cfg = json!({"provider": {"name": "openai", "baseUrl": "http://127.0.0.1:1/v1"}});
        let err = probe(&cfg, 800).unwrap_err();
        assert_eq!(err.code, "NETWORK", "{:?}", err.msg);
        assert!(!err.msg.contains("AGENTCANVAS_API_KEY"), "the env var name must not leak a value");

        let https = probe(&json!({"provider": {"baseUrl": "https://api.openai.com/v1"}}), 500).unwrap_err();
        assert_eq!(https.code, "NETWORK");
        assert!(https.hint.unwrap().contains("http"), "and it says what to do instead");

        let junk = probe(&json!({"provider": {"baseUrl": "ftp://x"}}), 500).unwrap_err();
        assert_eq!(junk.code, "INVALID_VALUE");
    }

    #[test]
    fn the_route_follows_the_shape_of_the_base_url() {
        assert_eq!(models_path("http://h:1/v1", "x"), "http://h:1/v1/models");
        assert_eq!(models_path("http://h:1/", "x"), "http://h:1/v1/models");
        assert_eq!(models_path("http://h:11434", ""), "http://h:11434/api/tags");
        let e = split("https://user:pass@host.example:8443/prefix").unwrap();
        assert_eq!((e.host.as_str(), e.port, e.path.as_str(), e.tls), ("host.example", 8443, "/prefix", true));
    }
}
