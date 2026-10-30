//! 局域网用量聚合：各设备按用户勾选的应用导出本机统计数据。
//! 只传用量指标和模型/会话标识，不传对话正文、请求内容或密钥。
//! UDP 发现设备，HTTP 拉取本机记录；远端副本不再转发。

pub mod aggregate;
pub mod client;
pub mod discovery;
pub mod identity;
pub mod pairing;
mod interfaces;
pub mod projection;
pub mod server;
pub mod storage;

use serde::{Deserialize, Serialize};
use std::sync::{Arc, LazyLock, Mutex};

/// 全局只跑一个局域网服务。重复启动时先停掉旧的。
static SERVER_HANDLE: LazyLock<Mutex<Option<server::LanServerHandle>>> =
    LazyLock::new(|| Mutex::new(None));

/// 当前服务占用的端口（没在跑就是 None）。
///
/// 守卫只在这个函数里活着，不会跨 await 点被持有——`std::sync::MutexGuard`
/// 不是 Send，跨 await 持有会让整个 future 变成非 Send，spawn 就编译不过。
fn running_port() -> Option<u16> {
    SERVER_HANDLE
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|handle| handle.port))
}

/// 取下正在跑的服务（停不停由调用方决定）。
fn take_running() -> Option<server::LanServerHandle> {
    SERVER_HANDLE.lock().ok().and_then(|mut guard| guard.take())
}

/// 把新启的服务装进全局槽位。槽位里已有服务时旧的那个会被停掉。
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

/// 按当前设置确保局域网服务处于运行状态。
///
/// 开着就启动，没开就停掉。重复调用是安全的：端口或设置没变时不会重启服务。
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

    // 端口变了或服务没在跑：先把旧的停掉，再起新的。
    if let Some(handle) = take_running() {
        handle.stop();
    }
    let handle = server::start_with_lock(db.clone(), settings.port, scan_lock).await?;
    let port = install_running(handle)?;
    log::info!("局域网用量服务已启动，端口 {port}");
    Ok(true)
}

/// 停掉局域网服务（应用退出时调用）。
pub fn stop_server() {
    if let Ok(mut guard) = SERVER_HANDLE.lock() {
        if let Some(handle) = guard.take() {
            handle.stop();
        }
    }
}

/// 应用启动时调用：按设置启动共享服务，并拉起定时同步。
pub fn start_background(db: Arc<crate::database::Database>, scan_lock: Arc<tokio::sync::Mutex<()>>) {
    tokio::spawn(async move {
        match ensure_server(db.clone(), scan_lock).await {
            Ok(true) => log::info!("局域网用量共享已开启"),
            Ok(false) => {}
            Err(e) => log::warn!("启动局域网用量服务失败: {e}"),
        }
        spawn_auto_sync(db).await;
    });
}

/// 定时把已登记设备的用量拉回来。
async fn spawn_auto_sync(db: Arc<crate::database::Database>) {
    tokio::spawn(async move {
        let mut last_sync = tokio::time::Instant::now();
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(10));
        loop {
            ticker.tick().await;
            let settings = identity::share_settings(&db);
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

/// 本机身份信息。局域网里用它区分「谁是谁」。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_name: String,
    pub app_version: String,
    pub platform: String,
    /// 进程启动时间（unix 秒），用来判断对端是不是刚重启过。
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

/// 一条用量明细（对端从本地文件解析出来的请求级记录）。
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
    /// 输入 token 的口径标记（0=legacy 1=total 2=fresh），聚合时必须跟着一起传，
    /// 否则各设备的缓存命中会被算错。
    pub input_token_semantics: i64,
    pub input_cost_usd: String,
    pub output_cost_usd: String,
    pub cache_read_cost_usd: String,
    pub cache_creation_cost_usd: String,
    pub total_cost_usd: String,
    pub status_code: i64,
    pub latency_ms: i64,
    /// 事件发生时间（unix 秒）。
    pub created_at: i64,
}

/// 一行按天汇总（对端明细已经被收进汇总表的历史数据）。
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

/// 对端导出的一整份用量。
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
    /// 还在明细表里的请求级记录。
    pub events: Vec<UsageEventRow>,
    /// 已收进按天汇总表的历史记录。
    pub daily: Vec<DailyRollupRow>,
}

/// 一个用量汇总块。字段和本机 Dashboard 的 `UsageSummary` 保持一致，前端可以直接
/// 复用同一套格式化逻辑。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanUsageSummary {
    pub total_requests: u64,
    pub total_cost: String,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_creation_tokens: u64,
    pub total_cache_read_tokens: u64,
    /// 成功（2xx）的请求数。
    pub success_count: u64,
    pub success_rate: f32,
    /// input + output + cache_creation + cache_read：模型真正处理的 token 总量。
    pub real_total_tokens: u64,
    /// cache_read / (input + cache_creation + cache_read)，0~1。
    pub cache_hit_rate: f64,
}

impl LanUsageSummary {
    /// 把两个汇总相加（用于「本机 + 远端 = 合计」）。
    pub fn add(&self, other: &LanUsageSummary) -> LanUsageSummary {
        let total_cost = self.total_cost.parse::<f64>().unwrap_or(0.0)
            + other.total_cost.parse::<f64>().unwrap_or(0.0);
        let mut merged = LanUsageSummary {
            total_requests: self.total_requests.saturating_add(other.total_requests),
            total_cost: format!("{total_cost}"),
            total_input_tokens: self
                .total_input_tokens
                .saturating_add(other.total_input_tokens),
            total_output_tokens: self
                .total_output_tokens
                .saturating_add(other.total_output_tokens),
            total_cache_creation_tokens: self
                .total_cache_creation_tokens
                .saturating_add(other.total_cache_creation_tokens),
            total_cache_read_tokens: self
                .total_cache_read_tokens
                .saturating_add(other.total_cache_read_tokens),
            success_count: self.success_count.saturating_add(other.success_count),
            success_rate: 0.0,
            real_total_tokens: 0,
            cache_hit_rate: 0.0,
        };
        merged.recompute_rates();
        merged
    }

    /// 按 token 总量重算「真实消耗」「缓存命中率」「成功率」。
    pub fn recompute_rates(&mut self) {
        self.real_total_tokens = self
            .total_input_tokens
            .saturating_add(self.total_output_tokens)
            .saturating_add(self.total_cache_creation_tokens)
            .saturating_add(self.total_cache_read_tokens);
        self.success_rate = if self.total_requests == 0 {
            0.0
        } else {
            self.success_count as f32 / self.total_requests as f32
        };
        let cache_base = self
            .total_input_tokens
            .saturating_add(self.total_cache_creation_tokens)
            .saturating_add(self.total_cache_read_tokens);
        self.cache_hit_rate = if cache_base == 0 {
            0.0
        } else {
            self.total_cache_read_tokens as f64 / cache_base as f64
        };
    }
}

/// 单台设备在局域网汇总里的一行。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanDeviceUsage {
    pub device_id: String,
    pub device_name: String,
    /// 该设备是本机还是远端。
    pub is_local: bool,
    pub summary: LanUsageSummary,
}

/// 一台设备的连接状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanDeviceStatus {
    pub device_id: String,
    pub device_name: String,
    pub host: String,
    pub port: u16,
    /// 是否在本机数据库里登记过（登记过的才会被拉取）。
    pub registered: bool,
    pub enabled: bool,
    pub paired: bool,
    pub last_seen_at: Option<i64>,
    pub last_sync_at: Option<i64>,
    pub last_error: Option<String>,
    /// 最近一次同步拉回多少行。
    pub last_imported: i64,
}

/// 局域网用量总览：本机 + 各远端 + 合计。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanUsageOverview {
    pub local: LanUsageSummary,
    pub remote: LanUsageSummary,
    pub combined: LanUsageSummary,
    pub devices: Vec<LanDeviceUsage>,
}

/// 按天的合计（跨设备）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanDailyTotal {
    pub date: String,
    pub summary: LanUsageSummary,
}

/// 局域网共享设置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanShareSettings {
    #[serde(default = "default_shared_app_types")]
    pub shared_app_types: Vec<String>,
    /// 是否对外提供本机用量（关掉就完全不对局域网暴露）。
    pub enabled: bool,
    pub port: u16,
    /// 对外暴露的本机名称，默认取主机名。
    pub device_name: String,
    /// 定期把已登记设备的用量拉回来。
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

/// 扫描局域网发现的对端设备。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredDevice {
    pub device_id: String,
    pub device_name: String,
    pub app_version: String,
    pub host: String,
    pub port: u16,
}

/// 把来源列表拼成 SQL 的 IN 列表。
pub fn sql_in_list(values: &[&str]) -> String {
    values
        .iter()
        .map(|v| format!("'{}'", v.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// 生成本地日期字符串（`YYYY-MM-DD`），和汇总表的 date 列同格式。
pub fn local_date_string(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_else(|| "1970-01-01".to_string())
}
