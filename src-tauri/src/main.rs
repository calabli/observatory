#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
use observatory_usage_core::api::UsageBackend;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

/// The statistics engine singleton. The OnceCell guarantees it is initialized
/// only once: when the startup warm-up and the first frontend request race,
/// the later one waits for the same initialization, so the database is never
/// opened twice (the second open would hit the instance lock error) and the
/// multi-second synchronous initialization is never performed while holding the
/// lock, which would block the async thread.
struct BackendState(tokio::sync::OnceCell<Arc<UsageBackend>>);

async fn backend(state: &BackendState) -> Result<Arc<UsageBackend>, String> {
    state
        .0
        .get_or_try_init(|| async {
            let backend = tauri::async_runtime::spawn_blocking(UsageBackend::open)
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            backend.start_background();
            Ok::<_, String>(Arc::new(backend))
        })
        .await
        .cloned()
}
struct TrayReady(AtomicBool);
#[cfg(target_os = "macos")]
struct PendingFullscreenHide(AtomicBool);

/// The menu bar / notification area icon is the bare glyph, without the app
/// icon's background tile. macOS tints a template image to match the menu
/// bar; Windows does not, so it gets a light glyph on dark themes and a dark
/// glyph on light ones.
#[cfg(target_os = "macos")]
fn tray_image(_dark: bool) -> tauri::Result<tauri::image::Image<'static>> {
    tauri::image::Image::from_bytes(include_bytes!("../icons/tray-template.png"))
}
#[cfg(not(target_os = "macos"))]
fn tray_image(dark: bool) -> tauri::Result<tauri::image::Image<'static>> {
    tauri::image::Image::from_bytes(if dark {
        include_bytes!("../icons/tray-light.png").as_slice()
    } else {
        include_bytes!("../icons/tray-dark.png").as_slice()
    })
}
#[cfg(target_os = "windows")]
fn tray_is_dark(_app: &tauri::AppHandle) -> bool {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};

    // Windows lets the taskbar and apps use separate color modes.
    let light_theme = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|key| key.get_value::<u32, _>("SystemUsesLightTheme"));
    light_theme.unwrap_or(0) == 0
}

#[cfg(target_os = "macos")]
fn tray_is_dark(_app: &tauri::AppHandle) -> bool {
    false // Template images are tinted by macOS.
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn tray_is_dark(app: &tauri::AppHandle) -> bool {
    app.get_webview_window("main")
        .and_then(|window| window.theme().ok())
        .is_some_and(|theme| theme == tauri::Theme::Dark)
}

fn show_window(app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    app.state::<PendingFullscreenHide>()
        .0
        .store(false, Ordering::Release);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// On macOS, calling `window.hide()` directly while a window is in a native
/// fullscreen Space leaves the user stranded on an empty black Space. Exit
/// fullscreen first, wait for AppKit's Space-exit animation (~600-700ms) to
/// finish (AppKit ignores `orderOut:` mid-transition even after `is_fullscreen`
/// flips to false), and then hide.
#[cfg(target_os = "macos")]
fn hide_window_safely(window: &tauri::Window) {
    if !window.is_fullscreen().unwrap_or(false) {
        let _ = window.hide();
        return;
    }
    let pending = window.state::<PendingFullscreenHide>();
    if pending.0.swap(true, Ordering::AcqRel) {
        return;
    }
    let started = std::time::Instant::now();
    let _ = window.set_fullscreen(false);
    let win = window.clone();
    std::thread::spawn(move || {
        // 1. Wait until fullscreen flag clears and the exit animation has had time to settle.
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(80));
            if !win
                .state::<PendingFullscreenHide>()
                .0
                .load(Ordering::Acquire)
            {
                return;
            }
            if !win.is_fullscreen().unwrap_or(false)
                && started.elapsed() >= std::time::Duration::from_millis(750)
            {
                break;
            }
        }
        // 2. Hide once settled; retry briefly in case AppKit's Space transition is still finishing.
        for _ in 0..6 {
            if !win
                .state::<PendingFullscreenHide>()
                .0
                .load(Ordering::Acquire)
            {
                return;
            }
            let _ = win.hide();
            std::thread::sleep(std::time::Duration::from_millis(120));
            if !win.is_visible().unwrap_or(false) {
                break;
            }
        }
        win.state::<PendingFullscreenHide>()
            .0
            .store(false, Ordering::Release);
    });
}

#[cfg(not(target_os = "macos"))]
fn hide_window_safely(window: &tauri::Window) {
    let _ = window.hide();
}

fn ensure_tray_panel(app: &tauri::AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(window) = app.get_webview_window("tray-panel") {
        return Some(window);
    }
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
    .build()
    .ok()
}

fn toggle_tray_panel(app: &tauri::AppHandle, x: f64, y: f64) {
    let Some(window) = ensure_tray_panel(app) else {
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
    // Create the panel up front so its page has loaded by the first tray click;
    // `ensure_tray_panel` still recreates it on demand if it is ever gone.
    if ensure_tray_panel(app.handle()).is_none() {
        eprintln!("Tray panel could not be created; the tray will open the main window.");
    }
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
    let dark = tray_is_dark(app.handle());
    let tray = TrayIconBuilder::with_id("observatory-tray")
        .icon(tray_image(dark)?)
        .icon_as_template(cfg!(target_os = "macos"))
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
    #[cfg(target_os = "windows")]
    let mut tray_dark = dark;
    let mut last_summary_key: Option<(u64, i64)> = None;
    std::thread::spawn(move || loop {
        // Recheck on the tray's existing refresh cycle, even when only the
        // Windows shell theme changes and no window theme event is emitted.
        #[cfg(target_os = "windows")]
        {
            let dark = tray_is_dark(&handle);
            if dark != tray_dark
                && tray_image(dark)
                    .and_then(|icon| tray.set_icon(Some(icon)))
                    .is_ok()
            {
                tray_dark = dark;
            }
        }
        let backend = handle.state::<BackendState>().0.get().cloned();
        let ready = backend.is_some();
        let cache_key = backend.as_ref().map(|b| b.today_cache_key());
        if ready && cache_key == last_summary_key {
            std::thread::sleep(std::time::Duration::from_secs(30));
            continue;
        }
        match backend.map(|backend| backend.today_summary()) {
            Some(Ok(summary)) => {
                last_summary_key = cache_key;
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
                last_summary_key = None;
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
    backend(&state)
        .await?
        .call(&operation, args)
        .await
        .map_err(|e| e.to_string())
}

/// Whether a window hides instead of closing when it is asked to close.
///
/// The main window stays in the tray, and the quick panel is only hidden: a
/// destroyed panel cannot be opened again in this session, so the tray icon
/// would fall back to the main window and the shortcut would be gone until
/// the next launch.
fn hides_on_close(label: &str, tray_ready: bool) -> bool {
    match label {
        "main" => tray_ready,
        "tray-panel" => true,
        _ => false,
    }
}

fn main() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_window(app);
        }))
        .manage(BackendState(tokio::sync::OnceCell::new()))
        .manage(TrayReady(AtomicBool::new(false)));
    #[cfg(target_os = "macos")]
    let builder = builder.manage(PendingFullscreenHide(AtomicBool::new(false)));
    builder
        .setup(|app| {
            if let Err(e) = setup_tray(app) {
                eprintln!("Tray unavailable: {e}");
                if let Some(panel) = app.get_webview_window("tray-panel") {
                    let _ = panel.destroy();
                }
            }
            // Warm up the statistics engine: the whole chain of
            // UsageBackend::open (instance lock, schema, pricing file replay,
            // possible initial CC Switch import) is a multi-second heavy job,
            // so it runs in the background first; if the first frontend request
            // arrives earlier, it waits for the same initialization to finish.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = backend(&handle.state::<BackendState>()).await {
                    eprintln!("后台初始化统计引擎失败: {e}");
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            if let tauri::WindowEvent::ThemeChanged(theme) = event {
                if let (Some(tray), Ok(image)) = (
                    window.app_handle().tray_by_id("observatory-tray"),
                    tray_image(*theme == tauri::Theme::Dark),
                ) {
                    let _ = tray.set_icon(Some(image));
                }
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if hides_on_close(
                    window.label(),
                    window.state::<TrayReady>().0.load(Ordering::Acquire),
                ) {
                    api.prevent_close();
                    hide_window_safely(window);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![usage_call, tray_action])
        .run(tauri::generate_context!())
        .expect("Unable to start Observatory");
}

#[cfg(test)]
mod tests {
    use super::hides_on_close;

    #[test]
    fn the_quick_panel_is_hidden_and_never_destroyed() {
        // Alt+F4 on the panel sends a close request. Destroying it there
        // would take the shortcut away for the rest of the session.
        assert!(hides_on_close("tray-panel", false));
        assert!(hides_on_close("tray-panel", true));
    }

    #[test]
    fn the_main_window_only_hides_when_the_tray_is_ready() {
        assert!(hides_on_close("main", true));
        // Without the tray there is nowhere to hide to, so the main window
        // has to close for real.
        assert!(!hides_on_close("main", false));
    }

    #[test]
    fn any_other_window_closes_normally() {
        assert!(!hides_on_close("some-other-window", true));
    }
}
