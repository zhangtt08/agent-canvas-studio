//! The local HTTP surface. It is deliberately hand-rolled: one dependency-free
//! HTTP/1.1 server is easier to audit than a framework, and the desktop webview
//! needs it for the same reason agents do — to fetch `/asset/<id>` and
//! `/font/<family>` bytes that the engine also uses when exporting.
//!
//! Loopback only. Auth is off by default (this is a local tool) and turns on as
//! soon as `settings.token` or `AGENTCANVAS_TOKEN` is set, at which point every
//! route except `/health` requires `Authorization: Bearer <token>`.

use crate::error::{Fail, R};
use crate::rpc::{self, Ctx};
use crate::util;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;

pub const DEFAULT_PORT: u16 = 8899;

static BOUND: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

pub fn port() -> u16 {
    BOUND.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn start(port: u16, root: Option<PathBuf>) -> R<u16> {
    let l = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| Fail::new("bind", format!("127.0.0.1:{port}: {e}")))?;
    let bound = l.local_addr().map(|a| a.port()).unwrap_or(port);
    BOUND.store(bound, std::sync::atomic::Ordering::Relaxed);
    thread::spawn(move || {
        for stream in l.incoming() {
            if let Ok(s) = stream {
                let r = root.clone();
                thread::spawn(move || {
                    let _ = handle(s, r);
                });
            }
        }
    });
    Ok(bound)
}

struct Req {
    method: String,
    path: String,
    query: String,
    auth: String,
    body: Vec<u8>,
}

fn handle(mut s: TcpStream, root: Option<PathBuf>) -> std::io::Result<()> {
    s.set_read_timeout(Some(std::time::Duration::from_secs(120))).ok();
    s.set_write_timeout(Some(std::time::Duration::from_secs(120))).ok();
    let mut buf: Vec<u8> = Vec::with_capacity(8192);
    let mut chunk = [0u8; 8192];
    // Read until the header terminator, then the declared body length.
    let head_end = loop {
        if let Some(i) = find(&buf, b"\r\n\r\n") {
            break i;
        }
        if buf.len() > 64 * 1024 {
            return Ok(());
        }
        match s.read(&mut chunk) {
            Ok(0) => return Ok(()),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => return Ok(()),
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let method = first.next().unwrap_or("GET").to_string();
    let target = first.next().unwrap_or("/").to_string();
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (urldecode(p), q.to_string()),
        None => (urldecode(&target), String::new()),
    };
    let mut content_len = 0usize;
    let mut auth = String::new();
    for line in lines {
        let (k, v) = match line.split_once(':') {
            Some(x) => x,
            None => continue,
        };
        match k.to_lowercase().as_str() {
            "content-length" => content_len = v.trim().parse().unwrap_or(0),
            "authorization" => auth = v.trim().to_string(),
            _ => {}
        }
    }
    let mut body: Vec<u8> = buf[head_end + 4..].to_vec();
    while body.len() < content_len {
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    let req = Req { method, path, query, auth, body };
    route(&mut s, req, root);
    Ok(())
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn route(s: &mut TcpStream, req: Req, root: Option<PathBuf>) {
    let ctx = Ctx { root: root.clone(), origin: "http".into() };
    let default_root = util::home();
    let token = auth_token(root.as_deref().unwrap_or(&default_root));
    let needs_auth = !matches!(req.path.as_str(), "/health" | "/favicon.ico");
    if let Some(t) = &token {
        if needs_auth && req.auth.replace("Bearer ", "").trim() != t.as_str() {
            return json_resp(s, "401 Unauthorized", &err_val("PERMISSION_DENIED", "missing or wrong bearer token", Some("set Authorization: Bearer <settings.token>, or clear the token to allow loopback access")));
        }
    }
    match (req.method.as_str(), req.path.as_str()) {
        ("OPTIONS", _) => write_all(s, "204 No Content", &[("Content-Type", "text/plain")], b""),
        ("POST", "/rpc") | ("GET", "/rpc") => {
            let (m, mut p) = match parse_rpc(&req) {
                Ok(x) => x,
                Err(e) => return json_resp(s, "400 Bad Request", &e.to_value()),
            };
            if let Some(f) = util::ps(&p, "fields").map(|s| s.to_string()) {
                p.as_object_mut().map(|o| o.remove("fields"));
                let v = rpc::call(&m, &p, &ctx);
                return match v {
                    Ok(v) => json_resp(s, "200 OK", &pick(&v, &f)),
                    Err(e) => json_resp(s, "400 Bad Request", &e.to_value()),
                };
            }
            match rpc::call(&m, &p, &ctx) {
                Ok(v) => json_resp(s, "200 OK", &v),
                Err(e) => json_resp(s, "400 Bad Request", &e.to_value()),
            }
        }
        // `pid` lets a caller prove it is answering *this* process, not whatever else
        // happens to be listening on the port.
        ("GET", "/health") => json_resp(s, "200 OK", &json!({"ok":true,"ver":crate::version(),"port":port(),"pid":std::process::id()})),
        ("GET", "/caps") => respond(s, rpc::call("system.caps", &json!({}), &ctx)),
        ("GET", "/tools") => match rpc::call("system.tools", &json!({"full":1}), &ctx) {
            Ok(v) => json_resp(s, "200 OK", &v),
            Err(e) => json_resp(s, "400 Bad Request", &e.to_value()),
        },
        ("POST", "/v1/projects") => {
            let p = body_json(&req).unwrap_or_else(|| json!({}));
            let mut args = json!({});
            if let Some(n) = util::ps(&p, "name") {
                args["name"] = json!(n);
            }
            if let Some(ab) = p.get("artboards") {
                if let Some(first) = ab.as_array().and_then(|a| a.first()) {
                    args["w"] = first.get("width").cloned().unwrap_or(json!(1080));
                    args["h"] = first.get("height").cloned().unwrap_or(json!(1440));
                }
                args["boards"] = ab.clone();
            }
            respond(s, rpc::call("project.create", &args, &ctx));
        }
        ("GET", "/v1/projects") => respond(s, rpc::call("project.list", &json!({}), &ctx)),
        ("GET", "/asset") | ("GET", "/font") | ("GET", "/file") | ("GET", "/preview") => {
            let q = parse_query(&req.query);
            let r = serve_bytes(&q, &req.path, root.as_deref());
            match r {
                Ok((bytes, mime)) => write_all(s, "200 OK", &[("Content-Type", mime)], &bytes),
                Err(e) => json_resp(s, "404 Not Found", &e.to_value()),
            }
        }
        ("GET", "/favicon.ico") => write_all(s, "204 No Content", &[], b""),
        _ => {
            // /v1/projects/<id>[/commands:batch|/nodes|/render]
            let seg: Vec<&str> = req.path.trim_matches('/').split('/').collect();
            if seg.first() == Some(&"v1") {
                return rest(s, &req, &seg, &ctx);
            }
            json_resp(
                s,
                "404 Not Found",
                &err_val(
                    "NOT_FOUND",
                    format!("no route {} {}", req.method, req.path),
                    Some("POST /rpc {\"m\":\"system.caps\"} is the whole API; GET /health is liveness"),
                ),
            )
        }
    }
}

/// The documented REST shape, implemented on top of the same RPC dispatch so
/// there is never a second business logic path to keep in sync.
fn rest(s: &mut TcpStream, req: &Req, seg: &[&str], ctx: &Ctx) {
    let body = body_json(req).unwrap_or_else(|| json!({}));
    let pid = seg.get(2).copied().unwrap_or("");
    let sub = seg.get(3).copied().unwrap_or("");
    let (m, p) = match (req.method.as_str(), sub) {
        ("GET", "") => ("project.get", json!({"p":pid,"depth":1})),
        ("POST", "commands:batch") => ("canvas.batch", json!({"p":pid,"cmds":body.get("commands").cloned().unwrap_or(json!([])),"rev":body.get("expectedRevision").cloned().unwrap_or(Value::Null)})),
        ("POST", "commands") => ("canvas.batch", json!({"p":pid,"cmds":body.get("commands").cloned().unwrap_or_else(|| body.get("cmd").cloned().unwrap_or(json!([]))),"rev":body.get("expectedRevision").cloned().unwrap_or(Value::Null)})),
        ("POST", "render") => ("render.export", merge(json!({"p":pid}), &body)),
        ("POST", "validate") => ("inspect.validate", merge(json!({"p":pid}), &body)),
        ("DELETE", "") => ("project.close", json!({"p":pid})),
        _ => return json_resp(s, "404 Not Found", &err_val("NOT_FOUND", format!("no route {} {}", req.method, req.path), Some("see GET /caps for the method list"))),
    };
    respond(s, rpc::call(m, &p, ctx))
}

fn merge(mut a: Value, b: &Value) -> Value {
    if let (Some(am), Some(bm)) = (a.as_object_mut(), b.as_object()) {
        for (k, v) in bm {
            am.insert(k.clone(), v.clone());
        }
    }
    a
}

fn parse_rpc(req: &Req) -> R<(String, Value)> {
    if req.method == "POST" {
        let v = body_json(req).ok_or_else(|| Fail::new("INVALID_SCHEMA", "body is not JSON"))?;
        let m = util::req_s(&v, "m").or_else(|_| util::req_s(&v, "method")).map(String::from)?;
        let p = v.get("p").cloned().or_else(|| v.get("params").cloned()).unwrap_or_else(|| json!({}));
        return Ok((m, p));
    }
    let q = parse_query(&req.query);
    let m = q.get("m").cloned().ok_or_else(|| Fail::new("INVALID_SCHEMA", "query needs m=<method>"))?;
    let p = q
        .get("p")
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or_else(|| {
            // also accept flat ?m=crop&in=a.png&w=100
            let mut o = serde_json::Map::new();
            for (k, v) in &q {
                if k != "m" && k != "p" {
                    o.insert(k.clone(), coerce(v));
                }
            }
            Value::Object(o)
        });
    Ok((m, p))
}

fn coerce(v: &str) -> Value {
    let t = v.trim();
    if t.starts_with('{') || t.starts_with('[') || t.starts_with('"') {
        if let Ok(j) = serde_json::from_str::<Value>(t) {
            return j;
        }
    }
    match t {
        "true" => json!(true),
        "false" => json!(false),
        _ => t.parse::<f64>().map(|x| json!(x)).unwrap_or_else(|_| json!(v)),
    }
}

fn body_json(req: &Req) -> Option<Value> {
    if req.body.is_empty() {
        return Some(json!({}));
    }
    serde_json::from_slice(&req.body).ok()
}

fn serve_bytes(q: &std::collections::HashMap<String, String>, path: &str, root: Option<&Path>) -> R<(Vec<u8>, &'static str)> {
    let dir = util::root(root);
    match path {
        "/asset" | "/preview" | "/file" => {
            let id = q.get("id").cloned().unwrap_or_default();
            let rel = q.get("path").cloned().unwrap_or_else(|| id.clone());
            if rel.is_empty() {
                return Err(Fail::new("INVALID_SCHEMA", "missing ?id= or ?path="));
            }
            // `?id=<asset_id>` is what the webview and the resource links use, and
            // an asset id is not a file name — resolve it through the documents.
            if let Some(found) = asset_by_id(&dir, &id).or_else(|| {
                if q.get("id").is_some() { None } else { None }
            }) {
                let b = util::read(&found)?;
                return Ok((b, mime(&found)));
            }
            let cand = if std::path::Path::new(&rel).is_absolute() {
                PathBuf::from(&rel)
            } else {
                find_under(&dir, &rel)?
            };
            if !util::clean(&cand).starts_with(util::clean(&dir)) && std::env::var("AGENTCANVAS_OPEN_FS").is_err() {
                return Err(Fail::new("PATH_DENIED", "outside root"));
            }
            let b = util::read(&cand)?;
            Ok((b, mime(&cand)))
        }
        "/font" => {
            let fam = q.get("family").cloned().unwrap_or_default();
            let weight = q.get("weight").and_then(|w| w.parse().ok()).unwrap_or(400u16);
            // Strict on purpose: silently handing back a substitute face is how a
            // preview ends up disagreeing with the export.
            if !crate::text::Registry::shared().has(&fam) {
                return Err(Fail::new("FONT_MISSING", format!("no installed family \"{fam}\""))
                    .hint("text.fonts lists what is installed, text.register adds a file"));
            }
            let bytes = crate::text::Registry::shared()
                .face_bytes(&fam, weight, q.get("style").map(|s| s.as_str()) == Some("italic"))
                .ok_or_else(|| Fail::new("FONT_MISSING", format!("no face for {fam}/{weight}")))?;
            Ok((bytes, "font/ttf"))
        }
        _ => Err(Fail::new("NOT_FOUND", path.to_string())),
    }
}

/// Assets live inside project directories; accept a bare file name and find it.
fn asset_by_id(dir: &Path, id: &str) -> Option<PathBuf> {
    if id.is_empty() {
        return None;
    }
    let projects = dir.join("projects");
    let rd = std::fs::read_dir(&projects).ok()?;
    for entry in rd.flatten() {
        let pd = entry.path();
        let Ok(txt) = std::fs::read_to_string(pd.join(crate::project::FILE)) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&txt) else { continue };
        let Some(uri) = v
            .get("assets")
            .and_then(|a| a.as_array())
            .and_then(|a| a.iter().find(|x| x.get("id").and_then(|i| i.as_str()) == Some(id)))
            .and_then(|x| x.get("uri"))
            .and_then(|u| u.as_str())
        else {
            continue;
        };
        let p = util::clean(&pd.join(uri));
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn find_under(dir: &Path, rel: &str) -> R<PathBuf> {
    let direct = dir.join(rel);
    if direct.is_file() {
        return Ok(util::clean(&direct));
    }
    let name = std::path::Path::new(rel).file_name().and_then(|s| s.to_str()).unwrap_or(rel);
    let mut stack = vec![dir.to_path_buf()];
    let mut depth = 0;
    while let Some(d) = stack.pop() {
        if depth > 6 {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().and_then(|s| s.to_str()) == Some("node_modules") {
                    continue;
                }
                stack.push(p);
            } else if p.file_name().and_then(|s| s.to_str()) == Some(name) {
                return Ok(util::clean(&p));
            }
        }
        depth += 1;
    }
    Err(Fail::new("NOT_FOUND", format!("no asset \"{rel}\" under {}", dir.display())))
}

fn mime(p: &Path) -> &'static str {
    match util::ext(p).as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "json" => "application/json",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff2" => "font/woff2",
        "css" => "text/css",
        "js" => "text/javascript",
        "html" => "text/html",
        _ => "application/octet-stream",
    }
}

fn parse_query(q: &str) -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    for pair in q.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        m.insert(urldecode(k), urldecode(v));
    }
    m
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = vec![];
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn auth_token(root: &Path) -> Option<String> {
    // The only accessor allowed to see the value. Nothing that lists settings —
    // system.config, caps, the call log — can return it.
    crate::session::access_token(root)
}

fn pick(v: &Value, fields: &str) -> Value {
    let want: Vec<&str> = fields.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    match v {
        Value::Object(m) => Value::Object(
            want.iter().filter_map(|k| m.get(*k).map(|val| ((*k).to_string(), val.clone()))).collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(|x| pick(x, fields)).collect()),
        other => other.clone(),
    }
}

fn respond(s: &mut TcpStream, r: R<Value>) {
    match r {
        Ok(v) => json_resp(s, "200 OK", &v),
        Err(e) => json_resp(s, "400 Bad Request", &e.to_value()),
    }
}

fn err_val(code: &str, msg: impl Into<String>, hint: Option<&str>) -> Value {
    let mut v = json!({"e": msg.into(), "code": code});
    if let Some(h) = hint {
        v["hint"] = json!(h);
    }
    v
}

fn json_resp(s: &mut TcpStream, status: &str, v: &Value) {
    let body = serde_json::to_vec(v).unwrap_or_else(|_| b"{}".to_vec());
    write_all(s, status, &[("Content-Type", "application/json")], &body);
}

fn write_all(s: &mut TcpStream, status: &str, headers: &[(&str, &str)], body: &[u8]) {
    let mut h = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n", body.len());
    h.push_str("Access-Control-Allow-Origin: *\r\n");
    h.push_str("Access-Control-Allow-Headers: Content-Type, Authorization\r\n");
    h.push_str("Access-Control-Allow-Methods: GET, POST, DELETE, OPTIONS\r\n");
    h.push_str("Cache-Control: no-store\r\n");
    h.push_str("Connection: close\r\n");
    for (k, v) in headers {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    h.push_str("\r\n");
    let _ = s.write_all(h.as_bytes());
    let _ = s.write_all(body);
    let _ = s.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_and_pick() {
        let q = parse_query("m=crop&in=a.png&w=100&x=%2Ftmp%2Fb");
        assert_eq!(q.get("m").map(|s| s.as_str()), Some("crop"));
        assert_eq!(q.get("x").map(|s| s.as_str()), Some("/tmp/b"));
        assert_eq!(coerce("100"), json!(100.0));
        let v = json!({"a":1,"b":2,"c":3});
        assert_eq!(pick(&v, "a,c"), json!({"a":1,"c":3}));
    }

    #[test]
    fn mime_is_real() {
        assert_eq!(mime(Path::new("x.PNG")), "image/png");
        assert_eq!(mime(Path::new("f.svg")), "image/svg+xml");
    }
}
