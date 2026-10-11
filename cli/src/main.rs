//! `agentcanvas` — the headless door. Same engine as the desktop app, no window.
//!
//! ```text
//! agentcanvas caps                     one call that explains the product
//! agentcanvas poster title=标题 image=a.png   end to end, one line
//! agentcanvas <method> k=v k2=v2       one call, compact JSON on stdout
//! agentcanvas rpc '{"m":"caps"}'       JSON in, JSON out
//! agentcanvas batch '[{...},{...}]'    several calls, one round trip
//! agentcanvas mcp                      stdio MCP server for any agent client
//! agentcanvas http [--port 8899]       REST + /asset + /font, loopback only
//! agentcanvas gui                      launch the desktop app if it is installed
//! ```
//!
//! stdout is always exactly one JSON value; logs and progress go to stderr, so a
//! shell pipeline never has to strip decoration.

use agentcanvas_core::{rpc, session, util};
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;

mod mcp;

struct Flags {
    root: Option<PathBuf>,
    pretty: bool,
    fields: Option<String>,
    rest: Vec<String>,
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut f = Flags { root: None, pretty: false, fields: None, rest: vec![] };
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--root" => {
                i += 1;
                f.root = argv.get(i).map(PathBuf::from);
            }
            "--pretty" => f.pretty = true,
            "--fields" => {
                i += 1;
                f.fields = argv.get(i).cloned();
            }
            "-h" | "--help" => {
                println!("{}", help_text().join("\n"));
                return;
            }
            "-V" | "--version" => {
                println!("agentcanvas {}", agentcanvas_core::version());
                return;
            }
            other if other.starts_with("--root=") => f.root = Some(PathBuf::from(other.trim_start_matches("--root="))),
            other if other.starts_with("--fields=") => f.fields = Some(other.trim_start_matches("--fields=").to_string()),
            other => {
                f.pretty |= other == "-p";
                f.rest.push(other.to_string());
            }
        }
        i += 1;
    }

    let ctx = rpc::Ctx { root: f.root.clone(), origin: "cli".into() };
    let out = match f.rest.first().map(|s| s.as_str()) {
        None | Some("") => Ok(json!({"e":"no method given","code":"INVALID_SCHEMA","hint":"agentcanvas caps","methods": rpc::cheatsheet()})),
        Some("mcp") => {
            agentcanvas_mcp_serve(&ctx);
            return;
        }
        Some("http") | Some("serve") => {
            let port = flag_val(&f.rest, "--port").or_else(|| std::env::var("AGENTCANVAS_PORT").ok().and_then(|p| p.parse().ok()))
                .unwrap_or(agentcanvas_core::http::DEFAULT_PORT);
            match agentcanvas_core::http::start(port, f.root.clone()) {
                Ok(bound) => {
                    eprintln!("agentcanvas http listening on http://127.0.0.1:{bound}/rpc  (Ctrl-C to stop)");
                    eprintln!("health: curl http://127.0.0.1:{bound}/health   caps: curl -s -XPOST http://127.0.0.1:{bound}/rpc -d '{{\"m\":\"caps\"}}'");
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(3600));
                    }
                }
                Err(e) => Err(e),
            }
        }
        Some("gui") => spawn_gui(&f.root),
        Some("batch") => {
            let raw = f.rest.get(1).map(|s| s.as_str()).unwrap_or("");
            if raw.starts_with('[') || raw.starts_with('{') || raw.starts_with('@') {
                match read_json_arg(raw) {
                    Ok(v) => rpc::call("batch", &json!({"jobs": v.as_array().cloned().unwrap_or_default()}), &ctx),
                    Err(e) => Err(e),
                }
            } else {
                // `agentcanvas batch jobs=[{...}]` is the shape `system.tools` documents,
                // so it must reach the same handler as the positional form.
                let mut p = json!({});
                merge_cli_args(&mut p, &f.rest[1..]);
                rpc::call("batch", &p, &ctx)
            }
        }
        Some("rpc") => {
            let raw = f.rest.get(1).cloned().unwrap_or_else(read_stdin);
            match read_json_arg(&raw) {
                Ok(v) => {
                    let m = util::ps(&v, "m").or_else(|| util::ps(&v, "method")).unwrap_or("").to_string();
                    let mut p = v.get("p").cloned().or_else(|| v.get("params").cloned()).unwrap_or_else(|| json!({}));
                    merge_cli_args(&mut p, &f.rest[2..]);
                    rpc::call(&m, &p, &ctx)
                }
                Err(e) => Err(e),
            }
        }
        Some(method) => {
            let mut p = json!({});
            merge_cli_args(&mut p, &f.rest[1..]);
            if let Some(pos) = f.rest.get(1) {
                if pos.starts_with('{') || pos.starts_with('[') {
                    match read_json_arg(pos) {
                        Ok(v) => p = v,
                        Err(e) => {
                            emit(Err(e), &f);
                            return;
                        }
                    };
                }
            }
            rpc::call(method, &p, &ctx)
        }
    };
    emit(out, &f);
}

fn emit(out: Result<Value, agentcanvas_core::Fail>, f: &Flags) {
    let v = match out {
        Ok(v) => v,
        Err(e) => {
            let mut o = e.to_value();
            if let Some(fl) = &f.fields {
                let _ = fl;
            }
            o
        }
    };
    let s = if f.pretty {
        serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".into())
    } else {
        serde_json::to_string(&v).unwrap_or_else(|_| "{}".into())
    };
    let mut so = std::io::stdout();
    let _ = so.write_all(s.as_bytes());
    let _ = so.write_all(b"\n");
    let _ = so.flush();
    if v.get("ok").and_then(|x| x.as_bool()) == Some(false) || v.get("code").is_some() {
        std::process::exit(1);
    }
}

/// `k=v` pairs, `k:=json`, `@file` payloads, and bare positionals mapped by the
/// method's usual first argument, so the most common calls stay one word long.
fn merge_cli_args(p: &mut Value, args: &[String]) {
    let Some(obj) = p.as_object_mut() else { return };
    for a in args {
        if a.starts_with("--") {
            continue;
        }
        match a.split_once('=') {
            Some((k, v)) => {
                let key = k.trim_start_matches('-');
                // `k:=json` and `k=:[json]` both mean "parse the value as JSON".
                match key.strip_suffix(':') {
                    Some(short) => {
                        obj.insert(short.to_string(), parse_val(&format!(":{v}")));
                    }
                    None => {
                        obj.insert(key.to_string(), parse_val(v));
                    }
                }
            }
            None => {
                if a.starts_with('@') || std::path::Path::new(a).is_file() {
                    obj.entry("path".to_string()).or_insert_with(|| json!(a.trim_start_matches('@')));
                } else if !obj.contains_key("text") && !obj.contains_key("title") && !a.contains(char::is_whitespace) {
                    obj.entry("arg".to_string()).or_insert_with(|| json!(a));
                }
            }
        }
    }
}

fn parse_val(v: &str) -> Value {
    let t = v.trim();
    if let Some(rest) = t.strip_prefix(':') {
        return serde_json::from_str(rest).unwrap_or_else(|_| json!(rest));
    }
    if (t.starts_with('{') || t.starts_with('[')) && serde_json::from_str::<Value>(t).is_ok() {
        return serde_json::from_str(t).unwrap_or(Value::Null);
    }
    if t == "true" || t == "false" {
        return json!(t == "true");
    }
    if let Ok(n) = t.parse::<f64>() {
        if n.is_finite() {
            // Emit a real JSON integer when the typed value has no fraction: params
            // read through `as_u64`/`as_i64` (board index, count, n) answer "missing"
            // for `0.0`, and `from=0` is the obvious thing for a human to type.
            if n.fract() == 0.0 && n.abs() < 9.0e15 {
                return json!(n as i64);
            }
            return json!(n);
        }
    }
    if t.starts_with('[') || t.starts_with('"') {
        if let Ok(j) = serde_json::from_str::<Value>(t) {
            return j;
        }
    }
    // comma lists are the ergonomic form for `ids=a,b,c`
    if t.contains(',') && !t.contains(' ') {
        return Value::Array(t.split(',').map(|x| json!(x.trim())).collect());
    }
    json!(t)
}

fn read_json_arg(raw: &str) -> Result<Value, agentcanvas_core::Fail> {
    let t = raw.trim();
    let body = if let Some(f) = t.strip_prefix('@') {
        std::fs::read_to_string(f).map_err(|e| agentcanvas_core::Fail::new("NOT_FOUND", format!("{f}: {e}")))?
    } else {
        t.to_string()
    };
    serde_json::from_str::<Value>(&body).map_err(|e| agentcanvas_core::Fail::new("INVALID_SCHEMA", format!("bad JSON: {e}")).hint("expecting {\"m\":..,\"p\":{..}} or a JSON object of params"))
}

fn read_stdin() -> String {
    use std::io::Read;
    let mut s = String::new();
    let _ = std::io::stdin().read_to_string(&mut s);
    s
}

fn flag_val(args: &[String], name: &str) -> Option<u16> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
}

fn spawn_gui(root: &Option<PathBuf>) -> Result<Value, agentcanvas_core::Fail> {
    let exe = std::env::current_exe().ok();
    let cand = exe.as_ref().and_then(|p| p.parent().map(|d| d.join("agentcanvas-app.exe")));
    match cand.filter(|p| p.exists()) {
        Some(p) => {
            let _ = std::process::Command::new(&p).spawn();
            Ok(json!({"ok":true,"started":util::short(&p, root.as_deref())}))
        }
        None => Err(agentcanvas_core::Fail::new("NOT_FOUND", "desktop app binary not found next to the CLI")
            .hint("run `cargo tauri dev`, or build it with scripts/build.mjs")),
    }
}

fn help_text() -> Vec<String> {
    let mut v = rpc::cheatsheet();
    v.push(String::new());
    v.push("flags: --root DIR (sandbox), --pretty, --fields a,b".into());
    v.push("args:  k=v  k:=json  @file.json   (paths resolve against --root)".into());
    v.push(session::workspace().display().to_string() + "  <- project root when --root is absent".into());
    v
}

/// Kept in its own item so the MCP transport can be swapped without touching main.
fn agentcanvas_mcp_serve(ctx: &rpc::Ctx) {
    crate::mcp::serve(ctx);
}
