#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! The desktop door. It is a thin shell: the window is frameless and the UI draws
//! its own controls, while the engine runs in this process and is reachable both
//! through `invoke("rpc")` and through the same loopback HTTP server an agent uses.
//! Closing the window leaves nothing half-written because every mutation is saved
//! atomically by the engine, not by the UI.

use agentcanvas_core::{http, rpc, session, util};
use serde_json::{json, Value};
use std::path::PathBuf;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

struct AppState {
    root: PathBuf,
    port: std::sync::Mutex<u16>,
}

/// One command carries the entire product: the webview calls the same RPC an
/// agent calls, so there is no second implementation to keep in sync.
#[tauri::command]
async fn rpc_call(state: tauri::State<'_, AppState>, method: String, params: Value) -> Result<Value, Value> {
    let root = state.root.clone();
    let ctx = rpc::Ctx { root: Some(root), origin: "desktop".into() };
    let task = tauri::async_runtime::spawn_blocking(move || rpc::call(&method, &params, &ctx)).await;
    match task {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(e.to_value()),
        Err(e) => Err(json!({"e": e.to_string(), "code": "INTERNAL"})),
    }
}

#[tauri::command]
fn endpoint(state: tauri::State<'_, AppState>) -> Value {
    let port = *state.port.lock().unwrap();
    json!({"port": port, "base": format!("http://127.0.0.1:{port}"), "root": util::short(&state.root, None)})
}

/// Native dialogs are the only thing the webview cannot do for itself. This runs
/// on Tauri's command thread, so blocking on the modal is fine.
#[tauri::command]
fn pick(app: tauri::AppHandle, state: tauri::State<'_, AppState>, kind: Option<String>) -> Result<Value, Value> {
    use tauri_plugin_dialog::DialogExt;
    let dlg = app.dialog().file();
    let chosen = if kind.as_deref() == Some("dir") { dlg.blocking_pick_folder() } else { dlg.blocking_pick_file() };
    match chosen {
        Some(p) => {
            let path = p.as_path().ok_or_else(|| json!({"e":"not a local path","code":"INVALID_VALUE"}))?;
            Ok(json!({"path": util::short(path, Some(&state.root)), "abs": path.to_string_lossy(), "exists": path.exists()}))
        }
        None => Ok(json!({"path": Value::Null, "cancelled": true})),
    }
}

#[tauri::command]
fn reveal(state: tauri::State<'_, AppState>, path: String) -> Value {
    let full = if std::path::Path::new(&path).is_absolute() { PathBuf::from(&path) } else { state.root.join(&path) };
    if !full.exists() {
        return json!({"ok":false,"e":"not found","code":"NOT_FOUND"});
    }
    let is_file = full.is_file();
    let dir = if is_file { full.parent().map(PathBuf::from).unwrap_or_else(|| state.root.clone()) } else { full.clone() };
    let target = dir.to_string_lossy().to_string();
    // `explorer /select,<file>` shows the thing itself; a bare folder path just opens it.
    let mut cmd = std::process::Command::new("explorer");
    if is_file {
        cmd.arg(format!("/select,{}", full.to_string_lossy()));
    } else {
        cmd.arg(&target);
    }
    let _ = cmd.spawn();
    json!({"ok": true, "dir": target})
}

fn main() {
    let root = session::workspace();
    let _ = std::fs::create_dir_all(&root);
    let setup_root = root.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState { root, port: std::sync::Mutex::new(0) })
        .setup(move |app| {
            // The window is built here, not in tauri.conf.json, so the bound port
            // exists before any page script runs: the UI streams assets and fonts
            // over HTTP from the same server agents use.
            let mut bound = 0u16;
            for p in http::DEFAULT_PORT..http::DEFAULT_PORT + 10 {
                if let Ok(got) = http::start(p, Some(setup_root.clone())) {
                    bound = got;
                    break;
                }
            }
            {
                let st = app.state::<AppState>();
                *st.port.lock().unwrap() = bound;
            }
            let init = format!(
                "window.__AC_PORT__={bound};window.__AC_ROOT__={root:?};",
                root = util::short(&setup_root, None)
            );
            let _win = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("AgentCanvas")
                .inner_size(1520.0, 950.0)
                .min_inner_size(1080.0, 700.0)
                .center()
                .resizable(true)
                .maximizable(true)
                .decorations(false)
                // Frameless + transparent is what lets the UI own the corners and
                // the window controls; `shadow(false)` because a rectangular OS
                // shadow would give the rounded slab a square halo.
                .transparent(true)
                .shadow(false)
                .accept_first_mouse(true)
                .initialization_script(&init)
                .build()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![rpc_call, endpoint, pick, reveal])
        .on_window_event(|_win, _e| {})
        .run(tauri::generate_context!())
        .expect("AgentCanvas failed to start");
}
