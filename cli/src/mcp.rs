//! MCP over stdio: JSON-RPC 2.0, newline-delimited, one message per line.
//!
//! The tool list is `rpc::mcp_tools()` — deliberately aggregated to ~40 coarse
//! tools rather than one per method, because every tool schema is re-sent in the
//! model context. `tools/call` forwards to the same `rpc::call` the desktop uses,
//! so an agent and the human in the window cannot diverge.

use agentcanvas_core::rpc::{self, Ctx};
use agentcanvas_core::util;
use serde_json::{json, Value};
use std::io::{BufRead, Write};

const PROTOCOL: &str = "2025-06-18";

pub fn serve(ctx: &Ctx) {
    // The history trail and the call log answer "who changed this?", so the door
    // has to be recorded, not inherited from the process that hosts the server.
    let ctx = Ctx { root: ctx.root.clone(), origin: "mcp".into() };
    let ctx = &ctx;
    let stdin = std::io::stdin();
    let out = std::io::stdout();
    let mut out = out.lock();
    let mut line = String::new();
    let mut batched = 0usize;
    loop {
        line.clear();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        let msg = line.trim();
        if msg.is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(msg) {
            Ok(v) => v,
            Err(_) => {
                let _ = writeln!(out, "{}", err(Value::Null, -32700, "parse error"));
                let _ = out.flush();
                continue;
            }
        };
        // One read may contain several requests (batch or concatenated).
        let items: Vec<Value> = match v {
            Value::Array(a) => {
                batched = a.len();
                a
            }
            other => vec![other],
        };
        for it in items {
            let id = it.get("id").cloned().unwrap_or(Value::Null);
            let method = it.get("method").and_then(|m| m.as_str()).unwrap_or("").to_string();
            let params = it.get("params").cloned().unwrap_or_else(|| json!({}));
            let is_notification = it.get("id").is_none();
            let resp = handle(&method, &params, ctx, &id);
            if !is_notification {
                if let Some(r) = resp {
                    let _ = writeln!(out, "{r}");
                }
            }
        }
        let _ = out.flush();
        if batched > 0 {
            batched = 0;
        }
    }
}

fn handle(method: &str, params: &Value, ctx: &Ctx, id: &Value) -> Option<String> {
    let r: Value = match method {
        "initialize" => json!({
            "protocolVersion": std::env::var("AGENTCANVAS_MCP_PROTOCOL").unwrap_or_else(|_| PROTOCOL.into()),
            "capabilities": {"tools": {"listChanged": false}, "resources": {"listChanged": false}, "logging": {}},
            "serverInfo": {"name": "agentcanvas", "title": "AgentCanvas", "version": agentcanvas_core::version()},
            "instructions": INSTRUCTIONS,
        }),
        "ping" | "notifications/initialized" | "initialized" => json!({}),
        "tools/list" => json!({"tools": tools()}),
        "tools/call" => {
            let name = params.get("name").and_then(|x| x.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let want_inline = util::opt_b(&args, "inline", false);
            match call_tool(name, args, ctx) {
                Ok(v) => {
                    let text = serde_json::to_string(&v).unwrap_or_else(|_| "{}".into());
                    let mut content = vec![json!({"type": "text", "text": text})];
                    // A produced image is advertised as a resource link, not inlined:
                    // base64 costs ~1.37 characters per PNG byte and then sits in the
                    // model's context for the rest of the conversation. `inline:1`
                    // opts into the bytes for clients that cannot fetch.
                    if let Some(link) = resource_link(&v) {
                        content.push(link);
                    }
                    if want_inline {
                        if let Some(img) = image_content(&v) {
                            content.push(img);
                        }
                    }
                    let mut out = json!({"content": content, "isError": false});
                    // Only echo the parsed copy while it is cheap; past that the text
                    // block already carries it and duplicating doubles the token bill.
                    if text.len() <= 2048 {
                        out["structuredContent"] = v;
                    }
                    out
                }
                Err(e) => json!({"content":[{"type":"text","text": serde_json::to_string(&e.to_value()).unwrap_or_default()}], "isError": true}),
            }
        }
        "resources/list" => json!({"resources": resources(ctx)}),
        "resources/read" => {
            let uri = params.get("uri").and_then(|x| x.as_str()).unwrap_or("");
            match read_resource(uri, ctx) {
                Ok(v) => v,
                Err(e) => return Some(err(id.clone(), -32002, e.msg)),
            }
        }
        "prompts/list" => json!({"prompts": prompts()}),
        "prompts/get" => json!({"description":"AgentCanvas poster prompt","messages":[{"role":"user","content":{"type":"text","text":INSTRUCTIONS}}]}),
        "" => return Some(err(id.clone(), -32600, "empty request")),
        other if other.starts_with("notifications/") => return None,
        other => return Some(err(id.clone(), -32601, format!("method not found: {other}"))),
    };
    Some(json!({"jsonrpc":"2.0","id":id,"result":r}).to_string())
}

/// Coarse tools forward to fine methods. `agentcanvas` itself is also exposed so
/// a client can reach any method without waiting for a schema update.
fn call_tool(name: &str, mut args: Value, ctx: &Ctx) -> agentcanvas_core::R<Value> {
    if matches!(name, "agentcanvas" | "rpc" | "call") {
        let m = args.get("m").or_else(|| args.get("method")).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let p = args.get("p").or_else(|| args.get("params")).cloned().unwrap_or_else(|| json!({}));
        return rpc::call(&m, &p, ctx);
    }
    // Resolve through the advertised table so a tool name and the method it calls
    // cannot drift apart.
    let list = rpc::mcp_tools();
    let method = list
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|t| t.get("name").and_then(|n| n.as_str()) == Some(name))
                .and_then(|t| t.get("method").and_then(|m| m.as_str()))
                .map(String::from)
        })
        .ok_or_else(|| {
            agentcanvas_core::Fail::new("INVALID_SCHEMA", format!("unknown tool \"{name}\""))
                .hint("call the agentcanvas tool with {m:\"caps\"} to list everything")
        })?;
    if let Some(o) = args.as_object_mut() {
        o.remove("m");
        o.remove("method");
    }
    rpc::call(&method, &args, ctx)
}

fn tools() -> Value {
    let list = rpc::mcp_tools();
    Value::Array(
        list.as_array()
            .map(|a| {
                a.iter()
                    .map(|t| {
                        let mut o = t.as_object().cloned().unwrap_or_default();
                        o.remove("method");
                        json!({"name": o.get("name"), "description": o.get("description"), "inputSchema": o.get("inputSchema"),
                               "annotations": {"readOnlyHint": is_read_only(t.get("name").and_then(|n| n.as_str()).unwrap_or("")),
                                               "destructiveHint": is_destructive(t.get("name").and_then(|n| n.as_str()).unwrap_or("")),
                                               "idempotentHint": is_idempotent(t.get("name").and_then(|n| n.as_str()).unwrap_or("")),
                                               "openWorldHint": false}})
                    })
                    .collect()
            })
            .unwrap_or_default(),
    )
}

fn is_read_only(name: &str) -> bool {
    matches!(
        name,
        "agentcanvas_caps" | "project_get" | "project_list" | "artboard_list" | "asset_list" | "asset_inspect"
            | "layout_explain" | "inspect_validate" | "inspect_diff" | "inspect_geometry" | "inspect_text"
            | "history_list" | "image_probe" | "image_compare" | "image_ops" | "image_presets" | "text_measure"
            | "system_caps" | "system_doctor" | "system_tools" | "render_svg" | "render_preview" | "template_builtin"
    )
}

/// `destructiveHint` is what tells a client to ask before firing the tool, so the
/// list has to be honest: deleting a board and rolling back history really are
/// destructive, while `render_export` only ever writes new files.
fn is_destructive(name: &str) -> bool {
    matches!(
        name,
        "canvas_delete" | "artboard_delete" | "asset_remove" | "history_rollback" | "project_close"
    )
}

fn is_idempotent(name: &str) -> bool {
    matches!(
        name,
        "project_get" | "project_list" | "caps" | "system_doctor" | "system_tools" | "image_probe" | "text_measure"
            | "render_export" | "render_preview" | "image_op" | "image_chain"
    )
}

fn resources(ctx: &Ctx) -> Vec<Value> {
    let mut out = vec![json!({
        "uri": "agentcanvas://caps", "name": "capabilities",
        "description": "methods, image ops, layout intents, limits, error codes",
        "mimeType": "application/json"
    })];
    if let Ok(Value::Array(list)) = rpc::call("project.list", &json!({}), ctx) {
        for pr in list.iter().take(50) {
            if let Some(id) = pr.get("p").and_then(|x| x.as_str()) {
                out.push(json!({"uri": format!("agentcanvas://project/{id}"), "name": pr.get("name").cloned().unwrap_or(json!(id)),
                                "description": "design document (scene graph)", "mimeType": "application/json"}));
            }
        }
    }
    out
}

fn read_resource(uri: &str, ctx: &Ctx) -> agentcanvas_core::R<Value> {
    if uri.ends_with("/caps") {
        let v = rpc::call("system.caps", &json!({}), ctx)?;
        return Ok(json!({"contents":[{"uri":uri,"mimeType":"application/json","text":serde_json::to_string(&v).unwrap_or_default()}]}));
    }
    if let Some(rel) = uri.strip_prefix("agentcanvas://file/") {
        let full = if std::path::Path::new(rel).is_absolute() {
            std::path::PathBuf::from(rel)
        } else {
            util::root(ctx.root.as_deref()).join(rel)
        };
        let bytes = std::fs::read(&full).map_err(|e| agentcanvas_core::Fail::new("NOT_FOUND", format!("{rel}: {e}")))?;
        use base64::Engine;
        return Ok(json!({"contents":[{"uri":uri,"mimeType":mime_of(rel),"blob": base64::engine::general_purpose::STANDARD.encode(&bytes)}]}));
    }
    if let Some(id) = uri.strip_prefix("agentcanvas://project/") {
        let v = rpc::call("project.get", &json!({"p": id}), ctx)?;
        return Ok(json!({"contents":[{"uri":uri,"mimeType":"application/json","text":serde_json::to_string(&v).unwrap_or_default()}]}));
    }
    Err(agentcanvas_core::Fail::new("NOT_FOUND", format!("unknown resource {uri}")))
}

fn prompts() -> Value {
    json!([
        {"name":"poster","title":"Make a poster","description":"assets + Chinese brief -> exported poster",
         "arguments":[{"name":"brief","required":true,"description":"what the poster is for"},{"name":"images","required":false,"description":"comma separated paths"}]},
        {"name":"series","title":"Make a 3-poster series","description":"one layout, three boards, different copy and accent",
         "arguments":[{"name":"title","required":true},{"name":"count","required":false}]}
    ])
}

/// When a call produced an image, advertise it as a fetchable resource instead of
/// pasting its bytes into the transcript.
fn resource_link(v: &Value) -> Option<Value> {
    let path = produced_file(v)?;
    if !is_image_path(&path) {
        return None;
    }
    Some(json!({
        "type": "resource_link",
        "uri": format!("agentcanvas://file/{}", path.replace('\\', "/")),
        "name": path.rsplit(['/', '\\']).next().unwrap_or(&path),
        "mimeType": mime_of(&path),
    }))
}

fn produced_file(v: &Value) -> Option<String> {
    v.get("path")
        .and_then(|p| p.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            v.get("files")
                .and_then(|f| f.as_array())
                .and_then(|a| a.first())
                .and_then(|x| x.get("path"))
                .and_then(|p| p.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            v.get("out")
                .and_then(|p| p.as_str())
                .map(|s| s.to_string())
        })
}

fn is_image_path(p: &str) -> bool {
    matches!(p.to_lowercase().rsplit('.').next().unwrap_or(""), "png" | "jpg" | "jpeg" | "webp")
}

fn mime_of(p: &str) -> &'static str {
    match p.to_lowercase().rsplit('.').next().unwrap_or("") {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

fn image_content(v: &Value) -> Option<Value> {
    let path = produced_file(v)?;
    let full = if std::path::Path::new(&path).is_absolute() { std::path::PathBuf::from(&path) } else { util::root(None).join(&path) };
    let bytes = std::fs::read(&full).ok()?;
    if bytes.len() > 4_000_000 {
        return None;
    }
    use base64::Engine;
    Some(json!({"type":"image","data": base64::engine::general_purpose::STANDARD.encode(&bytes), "mimeType":mime_of(&path)}))
}

fn err(id: Value, code: i64, msg: impl Into<String>) -> String {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":msg.into()}}).to_string()
}

const INSTRUCTIONS: &str = "AgentCanvas is a design/poster and image-processing engine. Call the agentcanvas_caps tool once: it lists every method, the image ops, the semantic layout intents and the error codes. Prefer `poster` for a whole deliverable; use canvas_add + layout_apply for step-by-step work; call inspect_validate after edits and render_preview to look at the result; render_export writes real files and returns sha256. Mutating calls accept rev=<expectedRevision> and return {ok,rev,changed}; a stale rev fails with REVISION_CONFLICT and the current rev, so re-read and retry. Locked layers are never moved without force. Pass paths relative to the configured root, or set root with --root.";
