//! LAN usage aggregation: every device exports its local statistics for the apps the user
//! selected. Only usage metrics and model/session identifiers are transferred, never the
//! conversation text, the request content or the keys.
//! Devices are discovered over UDP, the local records are pulled over HTTP; remote copies
//! are not forwarded any more.

pub mod client;
pub mod discovery;
pub mod identity;
mod interfaces;
pub mod pairing;
pub mod projection;
pub mod public_api;
pub mod server;
pub mod storage;

use serde::{Deserialize, Serialize};
use std::sync::{Arc, LazyLock, Mutex};

/// Only one LAN service runs at a time. A repeated start stops the old one first.
static SERVER_HANDLE: LazyLock<Mutex<Option<server::LanServerHandle>>> =
    LazyLock::new(|| Mutex::new(None));

/// The port the service currently uses (None when it is not running).
///
/// The guard only lives inside this function and is never held across an await point -- a
/// `std::sync::MutexGuard` is not Send, and holding it across an await would turn the whole
/// future into a non-Send one, which would not compile in a spawn.
fn running_port() -> Option<u16> {
    SERVER_HANDLE
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|handle| handle.port))
}

/// Take the running service out (whether it is stopped is up to the caller).
fn take_running() -> Option<server::LanServerHandle> {
    SERVER_HANDLE.lock().ok().and_then(|mut guard| guard.take())
}

/// Put the newly started service into the global slot. If the slot already holds a service, the old one is stopped.
fn install_running(handle: server::LanServerHandle) -> Result<u16, crate::error::AppError> {
    let port = handle.port;
    let previous = SERVER_HANDLE
        .lock()
        .ok()
        .and_then(|mut guard| guard.replace(handle));
    if let Some(previous) = previous {
        previous.stop();
    }
    Ok(port)
}

/// Make sure the LAN service is running according to the current settings.
///
/// Start it when it is enabled, stop it when it is not. Calling it repeatedly is safe: the
/// service is not restarted when neither the port nor the settings changed.
pub async fn ensure_server(
    db: Arc<crate::database::Database>,
    scan_lock: Arc<tokio::sync::Mutex<()>>,
) -> Result<bool, crate::error::AppError> {
    let settings = identity::share_settings(&db);

    if !settings.enabled {
        if let Some(handle) = take_running() {
            handle.stop();
            log::info!("局域网用量服务已停止");
        }
        return Ok(false);
    }

    if running_port() == Some(settings.port) {
        return Ok(true);
    }

    // The port changed or the service is not running: stop the old one first, then start the new one.
    if let Some(handle) = take_running() {
        handle.stop();
    }
    let handle = server::start_with_lock(db.clone(), settings.port, scan_lock).await?;
    let port = install_running(handle)?;
    log::info!("局域网用量服务已启动，端口 {port}");
    Ok(true)
}

/// Stop the LAN service (called when the application exits).
pub fn stop_server() {
    if let Ok(mut guard) = SERVER_HANDLE.lock() {
        if let Some(handle) = guard.take() {
            handle.stop();
        }
    }
}

/// Called at application startup: start the sharing service according to the settings and spin up the scheduled sync.
pub fn start_background(
    db: Arc<crate::database::Database>,
    scan_lock: Arc<tokio::sync::Mutex<()>>,
) {
    tokio::spawn(async move {
        match ensure_server(db.clone(), scan_lock).await {
            Ok(true) => log::info!("局域网用量共享已开启"),
            Ok(false) => {}
            Err(e) => log::warn!("启动局域网用量服务失败: {e}"),
        }
        spawn_auto_sync(db).await;
    });
}

/// Pull the usage of the registered devices back on a schedule.
async fn spawn_auto_sync(db: Arc<crate::database::Database>) {
    tokio::spawn(async move {
        let mut last_sync = tokio::time::Instant::now();
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(10));
        // One sync round (multiple devices x a timeout of up to 120s) often exceeds the 10s tick:
        // the ticks that piled up are dropped with Skip instead of being fired in a burst after the
        // sync has finished.
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Reading the sharing settings costs six primary key lookups and grabs the connection lock
        // on every tick; cache them for 60 seconds -- a settings change (turning sharing off, changing
        // the interval) takes effect within a minute at the latest, far less than the sync interval
        // itself.
        let mut cached_settings: Option<(tokio::time::Instant, LanShareSettings)> = None;
        loop {
            ticker.tick().await;
            let settings = match &cached_settings {
                Some((at, settings)) if at.elapsed() < std::time::Duration::from_secs(60) => {
                    settings.clone()
                }
                _ => {
                    let settings = identity::share_settings(&db);
                    cached_settings = Some((tokio::time::Instant::now(), settings.clone()));
                    settings
                }
            };
            if !settings.auto_sync
                || last_sync.elapsed().as_secs() < settings.auto_sync_interval_secs.max(60)
            {
                continue;
            }
            last_sync = tokio::time::Instant::now();
            let results = client::sync_all(&db).await;
            for (device_id, result) in results {
                match result {
                    Ok(n) if n > 0 => log::info!("局域网用量同步：{device_id} 拉回 {n} 条"),
                    Ok(_) => {}
                    Err(e) => log::warn!("局域网用量同步失败 {device_id}: {e}"),
                }
            }
        }
    });
}

/// Wire defaults remain compatible with CC Switch. Observatory persists port 17891 for its own service.
pub const DEFAULT_PORT: u16 = 17890;
pub const DISCOVERY_MAGIC: &str = "CC_SWITCH_LAN_DISCOVER_V1";

/// All supported usage apps; sharing exports metrics, never request/response bodies.
pub const SHARED_APP_TYPES: [&str; 14] = [
    "claude",
    "claude-desktop",
    "codex",
    "gemini",
    "grokbuild",
    "opencode",
    "openclaw",
    "hermes",
    "pi",
    "mcode",
    "kimi",
    "agy",
    "dsh",
    "zcode",
];

pub fn default_shared_app_types() -> Vec<String> {
    SHARED_APP_TYPES
        .iter()
        .map(|app| (*app).to_string())
        .collect()
}

/// Keep the allowlist ordered and unique. Unknown sources never enter export SQL.
pub fn normalize_shared_app_types(selected: &[String]) -> Vec<String> {
    SHARED_APP_TYPES
        .iter()
        .filter(|app| selected.iter().any(|value| value == **app))
        .map(|app| (*app).to_string())
        .collect()
}

/// The identity of this machine. It tells "who is who" on the LAN.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_name: String,
    pub app_version: String,
    pub platform: String,
    /// The process start time (unix seconds), used to tell whether a peer has just restarted.
    pub started_at: i64,
}

/// Statistical metadata only. Never includes provider settings, keys or message content.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UsageMetadata {
    pub provider_id: String,
    pub provider_name: String,
    pub request_model: Option<String>,
    pub pricing_model: Option<String>,
    pub first_token_ms: Option<i64>,
    pub duration_ms: Option<i64>,
    pub is_streaming: bool,
    pub cost_multiplier: Option<String>,
}

/// One usage detail record (a request level record the peer parsed out of its local files).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEventRow {
    #[serde(flatten)]
    pub metadata: UsageMetadata,
    pub request_id: String,
    pub data_source: String,
    pub app_type: String,
    pub model: String,
    pub session_id: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    /// The semantics marker of the input tokens (0=legacy 1=total 2=fresh); it has to be passed
    /// along during aggregation, otherwise the cache hits of the devices are miscounted.
    pub input_token_semantics: i64,
    pub input_cost_usd: String,
    pub output_cost_usd: String,
    pub cache_read_cost_usd: String,
    pub cache_creation_cost_usd: String,
    pub total_cost_usd: String,
    pub status_code: i64,
    pub latency_ms: i64,
    /// The time the event happened (unix seconds).
    pub created_at: i64,
}

/// One daily rollup row (history whose peer detail records were already rolled up).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyRollupRow {
    #[serde(flatten)]
    pub metadata: UsageMetadata,
    pub date: String,
    pub app_type: String,
    pub model: String,
    pub request_count: i64,
    pub success_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub input_token_semantics: i64,
    pub total_cost_usd: String,
    pub avg_latency_ms: i64,
}

/// A whole usage export of a peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageExport {
    #[serde(default)]
    pub snapshot_version: u32,
    pub device: DeviceInfo,
    /// None means an older peer: preserve its previously imported sources.
    /// Some([]) explicitly revokes every shared source on the next successful sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_app_types: Option<Vec<String>>,
    /// The request level records that are still in the detail table.
    pub events: Vec<UsageEventRow>,
    /// The historical records that were already rolled up into the per-day table.
    pub daily: Vec<DailyRollupRow>,
}

/// The connection state of one device.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanDeviceStatus {
    pub device_id: String,
    pub device_name: String,
    pub host: String,
    pub port: u16,
    /// Whether the device is registered in the local database (only registered devices are pulled).
    pub registered: bool,
    pub enabled: bool,
    pub paired: bool,
    pub last_seen_at: Option<i64>,
    pub last_sync_at: Option<i64>,
    pub last_error: Option<String>,
    /// How many rows the last sync pulled in.
    pub last_imported: i64,
}

/// The LAN sharing settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanShareSettings {
    #[serde(default = "default_shared_app_types")]
    pub shared_app_types: Vec<String>,
    /// Whether the local usage is offered to others (turning it off hides everything from the LAN).
    pub enabled: bool,
    pub port: u16,
    /// The local name exposed to others, the host name by default.
    pub device_name: String,
    /// Pull the usage of the registered devices back on a schedule.
    pub auto_sync: bool,
    pub auto_sync_interval_secs: u64,
}

impl Default for LanShareSettings {
    fn default() -> Self {
        Self {
            shared_app_types: default_shared_app_types(),
            enabled: false,
            port: DEFAULT_PORT,
            device_name: String::new(),
            auto_sync: true,
            auto_sync_interval_secs: 300,
        }
    }
}

/// Scan the LAN for peer devices.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredDevice {
    pub device_id: String,
    pub device_name: String,
    pub app_version: String,
    pub host: String,
    pub port: u16,
}

/// Build the SQL IN list from a list of sources.
pub fn sql_in_list(values: &[&str]) -> String {
    values
        .iter()
        .map(|v| format!("'{}'", v.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Produce a local date string (`YYYY-MM-DD`), the same format as the date column of the rollup table.
pub fn local_date_string(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_else(|| "1970-01-01".to_string())
}
