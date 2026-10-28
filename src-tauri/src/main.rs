#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
use observatory_usage_core::api::UsageBackend;
use std::sync::{Arc, Mutex};
use tauri::Manager;

struct BackendState(Mutex<Option<Arc<UsageBackend>>>);

#[tauri::command]
async fn usage_call(
    state: tauri::State<'_, BackendState>,
    operation: String,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let backend = {
        let mut slot = state.0.lock().map_err(|e| e.to_string())?;
        if slot.is_none() {
            let backend = Arc::new(UsageBackend::open().map_err(|e| e.to_string())?);
            backend.start_background();
            *slot = Some(backend);
        }
        slot.as_ref().unwrap().clone()
    };
    backend
        .call(&operation, args)
        .await
        .map_err(|e| e.to_string())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .manage(BackendState(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![usage_call])
        .run(tauri::generate_context!())
        .expect("Unable to start Observatory");
}
