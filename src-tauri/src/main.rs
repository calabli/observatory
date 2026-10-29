#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
use observatory_usage_core::api::UsageBackend;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

struct BackendState(Mutex<Option<Arc<UsageBackend>>>);
struct TrayReady(AtomicBool);

fn show_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn toggle_tray_panel(app: &tauri::AppHandle, x: f64, y: f64) {
    let Some(window) = app.get_webview_window("tray-panel") else {
        show_window(app);
        return;
    };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        return;
    }
    if let Ok(Some(monitor)) = window.monitor_from_point(x, y) {
        let area = monitor.work_area();
        let factor = monitor.scale_factor();
        let width = (420.0 * factor).min(area.size.width as f64);
        let height = (720.0 * factor).min(area.size.height as f64);
        let _ = window.set_size(tauri::PhysicalSize::new(width as u32, height as u32));
        let left = area.position.x as f64;
        let top = area.position.y as f64;
        let right = left + area.size.width as f64;
        let bottom = top + area.size.height as f64;
        let px = (x - width / 2.0).clamp(left, (right - width).max(left));
        let py = if y < top + area.size.height as f64 / 2.0 {
            y + 12.0
        } else {
            y - height - 12.0
        }
        .clamp(top, (bottom - height).max(top));
        let _ = window.set_position(tauri::PhysicalPosition::new(px as i32, py as i32));
    }
    let _ = window.show();
    let _ = window.set_focus();
}

#[tauri::command]
fn tray_action(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    action: &str,
) -> Result<(), String> {
    if window.label() != "tray-panel" {
        return Err("不允许的窗口".into());
    }
    match action {
        "open" => {
            let _ = window.hide();
            show_window(&app);
        }
        "close" => {
            let _ = window.hide();
        }
        "quit" => app.exit(0),
        _ => return Err("未知操作".into()),
    }
    Ok(())
}

fn setup_tray(app: &mut tauri::App) -> tauri::Result<()> {
    WebviewWindowBuilder::new(
        app,
        "tray-panel",
        WebviewUrl::App("index.html?surface=tray".into()),
    )
    .title("观测台快捷面板")
    .inner_size(420.0, 720.0)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(false)
    .build()?;
    let open = MenuItem::with_id(app, "open", "打开观测台", true, None::<&str>)?;
    let heading = MenuItem::with_id(app, "heading", "本机 · 今日用量", false, None::<&str>)?;
    let cost = MenuItem::with_id(app, "cost", "今日估算费用：读取中…", false, None::<&str>)?;
    let requests = MenuItem::with_id(app, "requests", "今日请求：读取中…", false, None::<&str>)?;
    let tokens = MenuItem::with_id(app, "tokens", "今日 Token：读取中…", false, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出观测台", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open, &sep1, &heading, &cost, &requests, &tokens, &sep2, &quit,
        ],
    )?;
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("app icon".into()))?;
    let tray = TrayIconBuilder::with_id("observatory-tray")
        .icon(icon)
        .tooltip("观测台 · 本机今日用量")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                position,
                ..
            } = event
            {
                toggle_tray_panel(tray.app_handle(), position.x, position.y);
            }
        })
        .build(app)?;
    app.state::<TrayReady>().0.store(true, Ordering::Release);
    let handle = app.handle().clone();
    std::thread::spawn(move || loop {
        let backend = handle
            .state::<BackendState>()
            .0
            .lock()
            .ok()
            .and_then(|state| state.clone());
        let ready = backend.is_some();
        match backend.map(|backend| backend.today_summary()) {
            Some(Ok(summary)) => {
                let usd = summary.total_cost.parse::<f64>().ok();
                let amount = usd
                    .map(|v| format!("${v:.2}"))
                    .unwrap_or_else(|| "—".into());
                let _ = cost.set_text(format!("今日估算费用：{amount}"));
                let _ = requests.set_text(format!("今日请求：{}", summary.total_requests));
                let _ = tokens.set_text(format!("今日 Token：{}", summary.real_total_tokens));
                let _ = tray.set_tooltip(Some(format!(
                    "观测台 · 今日 {amount} · {} 次请求",
                    summary.total_requests
                )));
            }
            Some(Err(_)) => {
                let _ = cost.set_text("今日用量读取失败，请打开观测台查看");
                let _ = requests.set_text("今日请求：—");
                let _ = tokens.set_text("今日 Token：—");
                let _ = tray.set_tooltip(Some("观测台 · 今日用量读取失败"));
            }
            None => {
                let _ = cost.set_text("今日用量未就绪，请打开观测台查看");
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(if ready { 30 } else { 3 }));
    });
    Ok(())
}

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
            show_window(app);
        }))
        .manage(BackendState(Mutex::new(None)))
        .manage(TrayReady(AtomicBool::new(false)))
        .setup(|app| {
            if let Err(e) = setup_tray(app) {
                eprintln!("Tray unavailable: {e}");
                if let Some(panel) = app.get_webview_window("tray-panel") {
                    let _ = panel.destroy();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" && window.state::<TrayReady>().0.load(Ordering::Acquire)
                {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![usage_call, tray_action])
        .run(tauri::generate_context!())
        .expect("Unable to start Observatory");
}
