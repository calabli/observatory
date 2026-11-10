use crate::{
    database::{lock_conn, Database},
    error::AppError,
    lan_usage::{self, identity, storage},
    services::{
        model_pricing, session_usage,
        usage_stats::{self, LogFilters},
    },
};
use chrono::TimeZone;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, sync::Mutex};

pub const APPS: &[(&str, &str, bool)] = &[
    ("claude", "Claude Code", true),
    ("claude-desktop", "Claude Desktop", false),
    ("codex", "Codex", true),
    ("gemini", "Gemini CLI", true),
    ("grokbuild", "Grok Build", true),
    ("opencode", "OpenCode", true),
    ("openclaw", "OpenClaw", false),
    ("hermes", "Hermes", false),
    ("pi", "Pi", true),
    ("mcode", "MiniMax Code", true),
    ("kimi", "Kimi Code", true),
    ("agy", "Antigravity", true),
    ("dsh", "DeepSeek Harness", true),
    ("zcode", "ZCode", true),
];
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Query {
    #[serde(flatten)]
    filters: LogFilters,
    multi_device: bool,
    device_ids: Option<Vec<String>>,
    page: u32,
    page_size: u32,
    request_id: String,
}

pub struct UsageBackend {
    pub db: Arc<Database>,
    scan_lock: Arc<tokio::sync::Mutex<()>>,
    _instance_lock: Option<std::fs::File>,
}

/// Cache for the two full-table aggregates of `status`, keyed by the write epoch
/// of this connection (any row-level write increments it, see `Database::write_epoch`);
/// `None` means the value still has to be computed.
static STATUS_COUNTS: Mutex<Option<(u64, StatusCounts)>> = Mutex::new(None);

#[derive(Clone)]
struct StatusCounts {
    detail_records: i64,
    /// (app_type, total_requests)。
    per_app: Vec<(String, u64)>,
}

/// Result cache for dashboard-style queries: keyed by (database instance, write epoch, query).
///
/// While the write epoch is unchanged the result of the same query cannot differ, yet the
/// tray panel, the overview and the usage page keep requesting the same dashboard when the
/// data has not changed (polling, remounting on tab switch), and each request scans the detail
/// table seven or eight times -- twice as much data in the multi-device view. Every rebuilt
/// projection is a new instance (`Database::instance_id`), so entries of old projections
/// naturally stop matching.
type ResultKey = (u64, u64, String);
static RESULT_CACHE: Mutex<std::collections::VecDeque<(ResultKey, Value)>> =
    Mutex::new(std::collections::VecDeque::new());
const RESULT_CACHE_CAPACITY: usize = 16;

/// Build the cache key. `epoch` must be read before the read snapshot is opened
/// (it has to be the same value that is used for computing the result).
/// A query without an end time uses "now" as its end (the trend is filled up to today), so the
/// local date is part of the key and yesterday's trend is not returned after midnight.
fn result_key(db: &Database, epoch: u64, op: &str, args: &Value) -> ResultKey {
    let open_ended = args.get("endDate").is_none_or(Value::is_null);
    let day = if open_ended {
        chrono::Local::now().format("%Y-%m-%d").to_string()
    } else {
        String::new()
    };
    (db.instance_id, epoch, format!("{op}|{day}|{args}"))
}

fn cached_result(key: &ResultKey) -> Option<Value> {
    let cache = RESULT_CACHE.lock().ok()?;
    cache
        .iter()
        .find(|(cached, _)| cached == key)
        .map(|(_, value)| value.clone())
}

fn store_result(key: ResultKey, value: &Value) {
    if let Ok(mut cache) = RESULT_CACHE.lock() {
        cache.retain(|(cached, _)| cached != &key);
        if cache.len() >= RESULT_CACHE_CAPACITY {
            cache.pop_front();
        }
        cache.push_back((key, value.clone()));
    }
}

impl UsageBackend {
    pub fn today_summary(&self) -> Result<usage_stats::UsageSummary, AppError> {
        if self.db.get_setting("usage_initial_import_error")?.is_some() {
            return Err(AppError::Message("历史用量导入未完成".into()));
        }
        let now = chrono::Local::now();
        let midnight = now
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .and_then(|time| chrono::Local.from_local_datetime(&time).earliest())
            .ok_or_else(|| AppError::Message("无法确定今日开始时间".into()))?;
        self.db.get_usage_summary(
            Some(midnight.timestamp()),
            Some(now.timestamp()),
            None,
            None,
            None,
        )
    }
    pub fn open() -> Result<Self, AppError> {
        let home = crate::config::get_app_config_dir();
        std::fs::create_dir_all(&home).map_err(|e| AppError::io(&home, e))?;
        let lock_path = home.join("engine.lock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|e| AppError::io(&lock_path, e))?;
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| {
            AppError::Message(
                "另一个观测台实例正在使用该数据目录，请先关闭原实例或开发服务。".into(),
            )
        })?;
        let db = Arc::new(Database::open(home.join("observatory.db"))?);
        if db.get_setting("lan_share_port")?.is_none() {
            db.set_setting("lan_share_port", "17891")?;
        }
        // The startup chain only replays the pricing file; the full-table backfill is deferred to the
        // gap after the first scan (see apply_local_model_pricing_deferring_backfill).
        model_pricing::apply_local_model_pricing_deferring_backfill(&db)?;
        // Only checks whether the database holds any detail records, short-circuiting with EXISTS instead of running a full-table aggregate at startup.
        let empty = !db.has_any_request_logs()?;
        let source = Self::cc_switch_path();
        if empty && db.get_setting("cc_switch_import_complete")?.is_none() && source.is_file() {
            match db.import_cc_switch(&source, None) {
                Ok(result) => {
                    db.delete_setting("usage_initial_import_error")?;
                    db.set_setting(
                        "usage_initial_import_notice",
                        &format!(
                            "已从 CC Switch 只读迁移 {} 条明细与 {} 条日汇总。{}",
                            result.requests,
                            result.rollups,
                            result.warnings.join("；")
                        ),
                    )?;
                }
                Err(error) => {
                    db.set_setting("usage_initial_import_error", &error.to_string())?;
                }
            }
        }
        Ok(Self {
            db,
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
            _instance_lock: Some(lock),
        })
    }
    pub fn with_database(db: Arc<Database>) -> Self {
        Self {
            db,
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
            _instance_lock: None,
        }
    }
    pub fn start_background(&self) {
        lan_usage::start_background(self.db.clone(), self.scan_lock.clone());
        let db = self.db.clone();
        let lock = self.scan_lock.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
            let mut last_maintenance = std::time::Instant::now();
            ticker.tick().await;
            loop {
                ticker.tick().await;
                // Reading settings needs the connection lock, so it goes to a blocking thread instead of stalling a worker while a scan holds the lock.
                let source = db.clone();
                let Ok((interval, import_failed)) = tokio::task::spawn_blocking(move || {
                    let interval = source
                        .get_setting("usage_refresh_interval")
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| "30".into());
                    let import_failed = source
                        .get_setting("usage_initial_import_error")
                        .ok()
                        .flatten()
                        .is_some();
                    (interval, import_failed)
                })
                .await
                else {
                    continue;
                };
                if interval == "manual" || import_failed {
                    continue;
                }
                let seconds = interval.parse::<i64>().unwrap_or(30).max(10);
                let elapsed = chrono::Utc::now().timestamp_millis()
                    - session_usage::last_sync_completed_at().unwrap_or(0);
                if elapsed < seconds * 1000 {
                    continue;
                }
                let Ok(_guard) = lock.clone().try_lock_owned() else {
                    continue;
                };
                let elapsed = chrono::Utc::now().timestamp_millis()
                    - session_usage::last_sync_completed_at().unwrap_or(0);
                if elapsed < seconds * 1000 {
                    continue;
                }
                let Ok(enabled) = read_enabled_sources(&db) else {
                    continue;
                };
                let source = db.clone();
                if let Err(e) = tokio::task::spawn_blocking(move || {
                    scan_database(&source, &enabled, crate::activity::ScanTrigger::Background)
                })
                .await
                {
                    log::warn!("Background usage scan failed: {e}");
                }
                // Low-frequency maintenance: move the WAL reclamation and the statistics update into the gap right
                // after a scan finishes, so the checkpoint stall after a large transaction does not land on the query peak.
                if last_maintenance.elapsed() >= std::time::Duration::from_secs(3600) {
                    let source = db.clone();
                    let _ =
                        tokio::task::spawn_blocking(move || source.maintenance_checkpoint()).await;
                    last_maintenance = std::time::Instant::now();
                }
            }
        });
    }
    pub fn cc_switch_path() -> PathBuf {
        std::env::var_os("CC_SWITCH_DB")
            .filter(|v| !v.is_empty())
            .map(Into::into)
            .unwrap_or_else(|| crate::config::get_home_dir().join(".cc-switch/cc-switch.db"))
    }
    fn enabled_sources(&self) -> Result<Vec<String>, AppError> {
        read_enabled_sources(&self.db)
    }
    /// Run one local scan under the same lock used by background scans.
    /// This deliberately never contacts other devices.
    pub async fn scan_local(
        &self,
        automatic: bool,
    ) -> Result<session_usage::SessionSyncResult, AppError> {
        let _guard = self.scan_lock.lock().await;
        let recent = session_usage::last_sync_completed_at()
            .is_some_and(|at| chrono::Utc::now().timestamp_millis() - at < 5000);
        if automatic && recent {
            return Ok(session_usage::SessionSyncResult::default());
        }
        let enabled = self.enabled_sources()?;
        let db = self.db.clone();
        let trigger = if automatic {
            crate::activity::ScanTrigger::Auto
        } else {
            crate::activity::ScanTrigger::Manual
        };
        tokio::task::spawn_blocking(move || scan_database(&db, &enabled, trigger))
            .await
            .map_err(|e| AppError::Message(e.to_string()))
    }
    /// The actual computation of `status`, split into an associated function so it can run in spawn_blocking.
    ///
    /// Two of the full-table aggregates (the detail COUNT and the request count per application) are
    /// cached per scan revision: `status` is invalidated by the revision poll and called at high frequency
    /// by the tray thread, while the row count only changes after a scan (pruning included) or a CC Switch
    /// import. The remaining settings-like fields are computed on every call and are unaffected by the cache.
    fn status_impl(db: &Database) -> Result<Value, AppError> {
        let source = Self::cc_switch_path();
        let info = identity::device_info(db)?;
        let enabled = read_enabled_sources(db)?;
        let initial_import_error = db.get_setting("usage_initial_import_error")?;
        let initial_import_notice = db.get_setting("usage_initial_import_notice")?;
        let refresh_interval = db
            .get_setting("usage_refresh_interval")?
            .unwrap_or_else(|| "30".into());
        // The cache key uses the write epoch instead of scan_revision: the revision increments on every scan
        // round (even without new data), which would blow the cache every 30 seconds; the epoch only changes
        // on real row-level writes, so `status` is a full cache hit during empty scans. Display fields such as
        // lastScanAt are computed outside the cache and are unaffected.
        let epoch = db.write_epoch.load(std::sync::atomic::Ordering::Relaxed);
        let cached = STATUS_COUNTS.lock().ok().and_then(|guard| {
            guard
                .as_ref()
                .filter(|(cached_epoch, _)| *cached_epoch == epoch)
                .map(|(_, counts)| counts.clone())
        });
        let counts = match cached {
            Some(counts) => counts,
            None => {
                let (stats, detail_records) = db.with_read_connection(|conn| {
                    let stats =
                        Database::usage_summary_by_app_on_conn(conn, None, None, None, None)?;
                    let detail_records: i64 =
                        conn.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |r| {
                            r.get(0)
                        })?;
                    Ok::<_, AppError>((stats, detail_records))
                })?;
                let counts = StatusCounts {
                    detail_records,
                    per_app: stats
                        .iter()
                        .map(|s| (s.app_type.clone(), s.summary.total_requests))
                        .collect(),
                };
                if let Ok(mut guard) = STATUS_COUNTS.lock() {
                    *guard = Some((epoch, counts.clone()));
                }
                counts
            }
        };
        let detail_records = counts.detail_records;
        let per_app = counts.per_app;
        Ok(
            json!({"version":env!("CARGO_PKG_VERSION"),"dataDir":crate::config::get_app_config_dir(),"databasePath":crate::config::get_app_config_dir().join("observatory.db"),"device":info,"ccSwitchPath":source,"ccSwitchAvailable":source.is_file(),"detailRecords":detail_records,"lastScanAt":session_usage::last_sync_completed_at(),"initialImportError":initial_import_error,"initialImportNotice":initial_import_notice,"refreshInterval":refresh_interval,"sources":APPS.iter().map(|(id,name,native)|json!({"id":id,"name":name,"nativeSupported":native,"enabled":enabled.iter().any(|v|v==id),"requests":per_app.iter().find(|(app,_)|app==id).map(|(_,requests)|*requests).unwrap_or(0)})).collect::<Vec<_>>() }),
        )
    }
    fn query_database(db: &Arc<Database>, multi: bool) -> Result<Arc<Database>, AppError> {
        if multi {
            lan_usage::projection::database(db)
        } else {
            Ok(db.clone())
        }
    }
    fn query_selected_database(db: &Arc<Database>, q: &Query) -> Result<Arc<Database>, AppError> {
        if !q.multi_device {
            return Ok(db.clone());
        }
        let Some(ids) = &q.device_ids else {
            return Self::query_database(db, true);
        };
        if ids.is_empty() {
            return Err(AppError::InvalidInput("请至少选择一台设备".into()));
        }
        let local = ids.iter().any(|id| id == "local");
        let peers: Vec<String> = ids
            .iter()
            .filter(|id| id.as_str() != "local")
            .cloned()
            .collect();
        if local && peers.is_empty() {
            return Ok(db.clone());
        }
        if local {
            lan_usage::projection::database_for_peers(db, &peers)
        } else {
            lan_usage::projection::database_for_peers_only(db, &peers)
        }
    }
    /// Purely synchronous local database operations all go to spawn_blocking: these ops can hold the
    /// connection lock for seconds (full-table aggregates, projection rebuilds) and must not occupy a
    /// tokio worker -- the LAN service and the revision poll share the same runtime.
    async fn call_blocking<T, F>(&self, f: F) -> Result<T, AppError>
    where
        T: Send + 'static,
        F: FnOnce(&Arc<Database>) -> Result<T, AppError> + Send + 'static,
    {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || f(&db))
            .await
            .map_err(|e| AppError::Message(e.to_string()))?
    }
    /// The multi-way aggregates of the dashboard, split into an associated function so they can run in spawn_blocking.
    ///
    /// All sub-aggregates are collected into a single `with_connection` (one lock, one snapshot): this
    /// saves the repeated locking and de-duplicated row estimates of each path and also guarantees that
    /// the totals, the trend and the groupings cannot be skewed by a background scan running in between.
    fn dashboard_impl(db: &Arc<Database>, q: &Query, args: &Value) -> Result<Value, AppError> {
        let db = Self::query_selected_database(db, q)?;
        let f = &q.filters;
        let (s, e, a, p, m) = (
            f.start_date,
            f.end_date,
            f.app_type.as_deref(),
            f.provider_name.as_deref(),
            f.model.as_deref(),
        );
        // The epoch must be read before the read snapshot is opened, see `with_read_connection`.
        let epoch = db.write_epoch.load(std::sync::atomic::Ordering::Relaxed);
        let key = result_key(&db, epoch, "dashboard", args);
        if let Some(hit) = cached_result(&key) {
            return Ok(hit);
        }
        let value = db.with_read_connection(|conn| {
            let summary = Database::usage_summary_impl(conn, s, e, a, p, m, false)?;
            let first_date = Database::first_usage_date_on_conn(conn, a, p, m)?;
            let trend_start = s.or_else(|| {
                first_date
                    .as_deref()
                    .and_then(|date| chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
                    .and_then(|day| day.and_hms_opt(0, 0, 0))
                    .and_then(|day| chrono::Local.from_local_datetime(&day).earliest())
                    .map(|time| time.timestamp())
            });
            let trends = Database::daily_trends_impl(
                conn,
                trend_start,
                Some(e.unwrap_or_else(|| chrono::Utc::now().timestamp())),
                a,
                p,
                m,
                false,
            )?;
            let providers = Database::provider_stats_impl(conn, s, e, a, p, m, true)?;
            let models = Database::model_stats_impl(conn, s, e, a, p, m, true)?;
            let mut count_cache = db
                .log_count_cache
                .lock()
                .map_err(|err| AppError::Message(format!("Database lock failed: {err}")))?;
            let mut logs = Database::request_logs_on_conn(
                conn,
                &mut count_cache,
                epoch,
                f,
                q.page.max(1) - 1,
                q.page_size.clamp(1, 200).max(20),
            )?;
            drop(count_cache);
            logs.page = q.page.max(1);
            let by_app = Database::usage_summary_by_app_on_conn(conn, s, e, p, m)?;
            // The dropdown only needs the names and is not affected by the provider/model filters. Without those
            // two filters the statistics above and the dropdown use the same conditions, so the names are taken
            // directly, saving two scans; only when a filter is present is a separate (lighter, without the
            // median) variant computed.
            let (option_models, option_providers): (Vec<String>, std::collections::BTreeSet<String>) =
                if p.is_none() && m.is_none() {
                    (
                        models.iter().map(|row| row.model.clone()).collect(),
                        providers.iter().map(|row| row.provider_name.clone()).collect(),
                    )
                } else {
                    (
                        Database::model_stats_impl(conn, s, e, a, None, None, false)?
                            .into_iter()
                            .map(|row| row.model)
                            .collect(),
                        Database::provider_stats_impl(conn, s, e, a, None, None, false)?
                            .into_iter()
                            .map(|row| row.provider_name)
                            .collect(),
                    )
                };
            Ok::<_, AppError>(
                json!({"summary":summary,"trends":trends,"providers":providers,"models":models,"logs":logs,"byApp":by_app,"firstDate":first_date,"options":{"models":option_models,"providers":option_providers}}),
            )
        })?;
        store_result(key, &value);
        Ok(value)
    }
    /// Unified command entry point. Key operations that change data or issue external requests (see
    /// `activity::action_label`) are recorded in the activity log here: sensitive fields are stripped
    /// from the arguments and only a summary of the result is kept; the record is written asynchronously
    /// on the blocking thread pool so the response is not slowed down.
    pub async fn call(&self, op: &str, args: Value) -> Result<Value, AppError> {
        let Some(label) = crate::activity::action_label(op, &args) else {
            return self.dispatch(op, args).await;
        };
        let safe_args = crate::activity::sanitize_args(&args);
        let started = std::time::Instant::now();
        let result = self.dispatch(op, args).await;
        let duration_ms = started.elapsed().as_millis() as i64;
        let (status, outcome) = match &result {
            Ok(value) => (
                "ok",
                json!({"result": crate::activity::result_brief(value)}),
            ),
            Err(error) => ("error", json!({"error": error.to_string()})),
        };
        let db = self.db.clone();
        let op = op.to_string();
        tokio::task::spawn_blocking(move || {
            let mut summary = json!({"op": op, "args": safe_args});
            if let (Some(target), Value::Object(extra)) = (summary.as_object_mut(), outcome) {
                target.extend(extra);
            }
            crate::activity::record(
                &db,
                crate::activity::Event {
                    kind: "action",
                    title: label.to_string(),
                    status,
                    duration_ms: Some(duration_ms),
                    summary,
                    detail: None,
                },
            );
        });
        result
    }

    async fn dispatch(&self, op: &str, args: Value) -> Result<Value, AppError> {
        match op {
            "activity_list" => {
                let kind = args.get("kind").and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_string);
                let before = args.get("beforeId").and_then(Value::as_i64);
                let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(100) as u32;
                self.call_blocking(move |db| to_json(crate::activity::list(db, kind.as_deref(), before, limit)?))
                    .await
            }
            "activity_detail" => {
                let id = args
                    .get("id")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| AppError::InvalidInput("Missing id".into()))?;
                self.call_blocking(move |db| Ok(crate::activity::detail(db, id)?.unwrap_or(Value::Null)))
                    .await
            }
            "subscriptions_get" => {
                self.call_blocking(move |db| to_json(crate::subscriptions::load(db)?)).await
            }
            "subscriptions_save" => {
                let records = serde_json::from_value(args.get("subscriptions").cloned().unwrap_or(Value::Null)).map_err(|_| AppError::InvalidInput("订阅列表无效".into()))?;
                self.call_blocking(move |db| {
                    crate::subscriptions::save(db, records)?;
                    Ok(json!(true))
                })
                .await
            }
            "subscription_usage" => {
                // `dailyLimit` only affects the length of the returned daily series, not the aggregation,
                // so it must not be folded into `SummaryQuery` (that is the public contract of the HTTP API).
                let mut query_args = args.clone();
                let daily_limit = query_args
                    .as_object_mut()
                    .and_then(|map| map.remove("dailyLimit"))
                    .and_then(|value| value.as_u64())
                    .map(|value| value.min(366) as usize);
                let query = serde_json::from_value(query_args).map_err(|e| AppError::InvalidInput(e.to_string()))?;
                let db = self.db.clone();
                let mut value = tokio::task::spawn_blocking(move || lan_usage::public_api::local_summary(&db, query)).await
                    .map_err(|e| AppError::Message(e.to_string()))?
                    .map_err(structured_usage_error)?;
                if let Some(limit) = daily_limit {
                    if let Some(daily) = value.get_mut("daily").and_then(Value::as_array_mut) {
                        if daily.len() > limit {
                            let start = daily.len() - limit;
                            daily.drain(0..start);
                        }
                    }
                }
                Ok(value)
            }
            "subscription_cycle_usage" => {
                // The usage within the current quota cycle; the response body matches `subscription_usage`.
                let query = serde_json::from_value(args).map_err(|e| AppError::InvalidInput(e.to_string()))?;
                let db = self.db.clone();
                tokio::task::spawn_blocking(move || lan_usage::public_api::local_cycle_summary(&db, query)).await
                    .map_err(|e| AppError::Message(e.to_string()))?
                    .map_err(structured_usage_error)
            }
            "subscription_cycle_series" => {
                // The usage series of the last few quota cycles (grouped by calendar day).
                let query = serde_json::from_value(args).map_err(|e| AppError::InvalidInput(e.to_string()))?;
                let db = self.db.clone();
                tokio::task::spawn_blocking(move || lan_usage::public_api::local_cycle_series(&db, query)).await
                    .map_err(|e| AppError::Message(e.to_string()))?
                    .map_err(structured_usage_error)
            }
            // Every op below that touches the database goes through call_blocking: the connection lock is a
            // std::sync::Mutex, and calling it directly from async while a scan batch holds the lock would
            // stall a tokio worker and slow down the LAN service and the revision poll on the same runtime.
            "usage_api_key_status" => {
                self.call_blocking(|db| Ok(json!({"configured":lan_usage::public_api::key_configured(db)?}))).await
            }
            "usage_api_key_rotate" => {
                self.call_blocking(|db| Ok(json!({"key":lan_usage::public_api::rotate_key(db)?}))).await
            }
            "usage_api_key_revoke" => {
                self.call_blocking(|db| {
                    lan_usage::public_api::revoke_key(db)?;
                    Ok(json!(true))
                })
                .await
            }
            "scan_revision" => Ok(json!({"revision":session_usage::scan_revision(),"epoch":self.db.write_epoch.load(std::sync::atomic::Ordering::Relaxed)})),
            "tray_dashboard" => {
                let q: Query = serde_json::from_value(args.clone()).map_err(|e| AppError::InvalidInput(e.to_string()))?;
                let peer = args
                    .get("deviceId")
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
                    .map(str::to_string);
                self.call_blocking(move |db| {
                    let db = if let Some(peer) = peer { lan_usage::projection::database_for_peer(db, &peer)? } else { Self::query_database(db, q.multi_device)? };
                    let (start, end) = (q.filters.start_date, q.filters.end_date);
                    let epoch = db.write_epoch.load(std::sync::atomic::Ordering::Relaxed);
                    let key = result_key(&db, epoch, "tray_dashboard", &args);
                    if let Some(hit) = cached_result(&key) {
                        return Ok(hit);
                    }
                    // The three aggregates are collected into the same read snapshot, so a background scan cannot slip in between them and make the numbers inconsistent.
                    let value = db.with_read_connection(|conn| {
                        let summary =
                            Database::usage_summary_impl(conn, start, end, None, None, None, false)?;
                        let by_app =
                            Database::usage_summary_by_app_on_conn(conn, start, end, None, None)?;
                        let trends =
                            Database::daily_trends_impl(conn, start, end, None, None, None, false)?;
                        Ok::<_, AppError>(json!({"summary":summary,"byApp":by_app,"trends":trends}))
                    })?;
                    store_result(key, &value);
                    Ok(value)
                })
                .await
            }
            "network_international_ipv4" => crate::network_overview::international_ipv4().await,
            "network_snapshot" => {
                let force = args.get("refreshMetadata").and_then(Value::as_bool).unwrap_or(false);
                tokio::task::spawn_blocking(move || crate::network_overview::snapshot(force))
                    .await
                    .map_err(|e| AppError::Message(e.to_string()))
            }
            "status" => self.call_blocking(move |db| Self::status_impl(db)).await,
            "preferences_save" => {
                let interval = args
                    .get("refreshInterval")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AppError::InvalidInput("Refresh interval required".into()))?;
                if !["30", "60", "manual"].contains(&interval) {
                    return Err(AppError::InvalidInput(
                        "Unsupported refresh interval".into(),
                    ));
                }
                let interval = interval.to_string();
                self.call_blocking(move |db| {
                    db.set_setting("usage_refresh_interval", &interval)?;
                    Self::status_impl(db)
                })
                .await
            }
            "dashboard" => {
                let q: Query = serde_json::from_value(args.clone())
                    .map_err(|e| AppError::InvalidInput(e.to_string()))?;
                self.call_blocking(move |db| Self::dashboard_impl(db, &q, &args)).await
            }
            "request_detail" => {
                let q: Query = serde_json::from_value(args)
                    .map_err(|e| AppError::InvalidInput(e.to_string()))?;
                self.call_blocking(move |db| {
                    let db = Self::query_selected_database(db, &q)?;
                    to_json(db.get_request_detail(&q.request_id)?)
                })
                .await
            }
            "sources_save" => {
                let selected: Vec<String> =
                    serde_json::from_value(args.get("enabled").cloned().unwrap_or(json!([])))
                        .map_err(|e| AppError::InvalidInput(e.to_string()))?;
                if selected.iter().any(|id| !APPS.iter().any(|a| a.0 == id)) {
                    return Err(AppError::InvalidInput("Unknown application source".into()));
                }
                let encoded = serde_json::to_string(&selected)
                    .map_err(|e| AppError::Message(e.to_string()))?;
                self.call_blocking(move |db| {
                    db.set_setting("usage_enabled_sources", &encoded)?;
                    Self::status_impl(db)
                })
                .await
            }
            "sync" => {
                let result = self.scan_local(args.get("automatic").and_then(Value::as_bool).unwrap_or(false)).await?;
                let mut peers = Vec::new();
                for (id, result) in lan_usage::client::sync_all(&self.db).await {
                    peers.push(match result {
                        Ok(count) => json!({"deviceId":id,"imported":count}),
                        Err(e) => json!({"deviceId":id,"error":e.to_string()}),
                    });
                }
                Ok(json!({"scan":result,"peers":peers}))
            }
            "import_cc_switch" => {
                let _guard = self.scan_lock.lock().await;
                let path = args
                    .get("path")
                    .and_then(Value::as_str)
                    .filter(|v| !v.trim().is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(Self::cc_switch_path);
                let db = self.db.clone();
                let imported =
                    tokio::task::spawn_blocking(move || db.import_cc_switch(&path, None))
                        .await
                        .map_err(|e| AppError::Message(e.to_string()))??;
                self.call_blocking(|db| db.delete_setting("usage_initial_import_error"))
                    .await?;
                to_json(imported)
            }
            "pricing_list" => {
                self.call_blocking(move |db| {
                    db.ensure_model_pricing_seeded()?;
                    model_pricing::sync_local_model_pricing(db)?;
                    let conn = lock_conn!(db.conn);
                    let mut stmt=conn.prepare("SELECT model_id,display_name,input_cost_per_million,output_cost_per_million,cache_read_cost_per_million,cache_creation_cost_per_million FROM model_pricing ORDER BY model_id")?;
                    let rows = stmt.query_map([], |r| {
                        Ok(model_pricing::ModelPricingInfo {
                            model_id: r.get(0)?,
                            display_name: r.get(1)?,
                            input_cost_per_million: r.get(2)?,
                            output_cost_per_million: r.get(3)?,
                            cache_read_cost_per_million: r.get(4)?,
                            cache_creation_cost_per_million: r.get(5)?,
                        })
                    })?;
                    to_json(rows.collect::<Result<Vec<_>, _>>()?)
                })
                .await
            }
            "pricing_update" => {
                let entry = serde_json::from_value(args)
                    .map_err(|e| AppError::InvalidInput(e.to_string()))?;
                // Changing a price backfills historical costs (possibly a large batch UPDATE), so it has to leave the async thread.
                self.call_blocking(move |db| to_json(model_pricing::update_model_pricing(db, entry)?))
                    .await
            }
            "pricing_batch" => {
                let entries =
                    serde_json::from_value(args.get("entries").cloned().unwrap_or(json!([])))
                        .map_err(|e| AppError::InvalidInput(e.to_string()))?;
                self.call_blocking(move |db| {
                    to_json(model_pricing::update_model_pricing_batch(db, entries)?)
                })
                .await
            }
            "pricing_delete" => {
                let id = args
                    .get("modelId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AppError::InvalidInput("Model ID is required".into()))?
                    .to_string();
                self.call_blocking(move |db| {
                    model_pricing::delete_model_pricing(db, &id)?;
                    Ok(json!(true))
                })
                .await
            }
            "modelsdev_config_get" => {
                self.call_blocking(|db| to_json(model_pricing::get_models_dev_sync_state(db)?))
                    .await
            }
            "modelsdev_config_save" => {
                let config = serde_json::from_value(args.get("config").cloned().unwrap_or(args))
                    .map_err(|e| AppError::InvalidInput(e.to_string()))?;
                self.call_blocking(move |db| {
                    model_pricing::save_models_dev_sync_config(db, config)?;
                    to_json(model_pricing::get_models_dev_sync_state(db)?)
                })
                .await
            }
            "modelsdev_record_result" => {
                let synced_at = args.get("syncedAt").and_then(Value::as_i64);
                let error = args.get("error").and_then(Value::as_str).map(str::to_string);
                self.call_blocking(move |db| {
                    model_pricing::record_models_dev_sync_result(db, synced_at, error)?;
                    Ok(json!(true))
                })
                .await
            }
            "modelsdev_fetch" => {
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(30))
                    .build()
                    .map_err(|e| AppError::Message(e.to_string()))?;
                let response = client
                    .get("https://models.dev/api.json")
                    .send()
                    .await
                    .map_err(|e| AppError::Message(e.to_string()))?;
                if !response.status().is_success() {
                    return Err(AppError::Message(format!(
                        "models.dev returned HTTP {}",
                        response.status()
                    )));
                }
                response
                    .json::<Value>()
                    .await
                    .map_err(|e| AppError::Message(e.to_string()))
            }
            "lan_status" => {
                self.call_blocking(move |db| {
                    Ok(json!({"settings":identity::share_settings(db),"local":identity::device_info(db)?,"devices":storage::list_devices(db)?}))
                })
                .await
            }
            "lan_pair_code" => {
                self.call_blocking(|db| {
                    if !identity::share_settings(db).enabled {
                        return Err(AppError::Message("请先开启局域网共享".into()));
                    }
                    let (code, expires_at) = lan_usage::pairing::generate(db)?;
                    Ok(json!({"code":code,"expiresAt":expires_at}))
                })
                .await
            }
            "lan_incoming_list" => {
                self.call_blocking(|db| to_json(lan_usage::pairing::incoming(db)?)).await
            }
            "lan_incoming_revoke" => {
                let id = string_arg(&args, "deviceId")?.to_string();
                self.call_blocking(move |db| {
                    lan_usage::pairing::revoke(db, &id)?;
                    Ok(json!(true))
                })
                .await
            }
            "scan_local_only" => to_json(self.scan_local(false).await?),
            "scan_remote_start" | "scan_remote_status" => {
                let id = string_arg(&args, "deviceId")?;
                let key = string_arg(&args, "key")?;
                lan_usage::client::scan_request(&self.db, id, key, op == "scan_remote_start").await
            }
            "scan_remote_sync" => {
                let id = string_arg(&args, "deviceId")?;
                let count = lan_usage::client::sync_device(&self.db, id).await?;
                Ok(json!({"imported":count}))
            }
            "lan_settings_save" => {
                let settings = serde_json::from_value(args)
                    .map_err(|e| AppError::InvalidInput(e.to_string()))?;
                self.call_blocking(move |db| identity::save_share_settings(db, &settings))
                    .await?;
                lan_usage::ensure_server(self.db.clone(), self.scan_lock.clone()).await?;
                self.call_blocking(|db| to_json(identity::share_settings(db))).await
            }
            "lan_discover" => {
                let port = self.call_blocking(|db| Ok(identity::share_settings(db).port)).await?;
                // Probe both ports in parallel: each round has to wait out the full 1200ms deadline, so probing them serially is pure dead time.
                let mut found = if port == 17890 {
                    lan_usage::discovery::discover(port, 1200)
                        .await
                        .map_err(AppError::Message)?
                } else {
                    let (own_port, cc_switch_port) = tokio::join!(
                        lan_usage::discovery::discover(port, 1200),
                        lan_usage::discovery::discover(17890, 1200)
                    );
                    let mut merged = own_port.map_err(AppError::Message)?;
                    merged.extend(cc_switch_port.map_err(AppError::Message)?);
                    merged
                };
                let (own, old) = self
                    .call_blocking(|db| {
                        Ok((
                            identity::device_id(db)?,
                            db.get_setting("cc_switch_local_device_id")?.unwrap_or_default(),
                        ))
                    })
                    .await?;
                found.retain(|d| d.device_id != own && d.device_id != old);
                let mut ids = std::collections::HashSet::new();
                found.retain(|d| ids.insert(d.device_id.clone()));
                to_json(found)
            }
            "lan_connect" => {
                // Alternate database steps and network requests: every database read or write goes through
                // call_blocking on its own, so waiting on the network holds no blocking thread and never
                // grabs the connection lock from async.
                let settings = self.call_blocking(|db| Ok(identity::share_settings(db))).await?;
                if !settings.enabled { return Err(AppError::Message("双向配对需要先开启本机局域网共享".into())); }
                lan_usage::ensure_server(self.db.clone(), self.scan_lock.clone()).await?;
                let host = args
                    .get("host")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AppError::InvalidInput("Device address required".into()))?;
                let url = url::Url::parse(&format!("http://{}", host.trim()))
                    .map_err(|_| AppError::InvalidInput("Invalid device address".into()))?;
                if url.path() != "/"
                    || url.query().is_some()
                    || !url.username().is_empty()
                    || url.password().is_some()
                {
                    return Err(AppError::InvalidInput(
                        "Enter an IP or hostname, optionally followed by :port".into(),
                    ));
                }
                let host = url
                    .host_str()
                    .ok_or_else(|| AppError::InvalidInput("Invalid hostname".into()))?;
                let port = args
                    .get("port")
                    .and_then(Value::as_u64)
                    .and_then(|v| u16::try_from(v).ok())
                    .or(url.port())
                    .unwrap_or(17891);
                let info = lan_usage::client::fetch_info(host, port).await?;
                let code = string_arg(&args, "code")?;
                let reverse_token = lan_usage::pairing::new_token();
                // Self-connection check plus writing the reverse authorization up front, done in a single blocking call.
                let (previous_grant, local_info) = {
                    let peer_id = info.device_id.clone();
                    let reverse_token = reverse_token.clone();
                    self.call_blocking(move |db| {
                        if peer_id == identity::device_id(db)?
                            || Some(peer_id.as_str())
                                == db.get_setting("cc_switch_local_device_id")?.as_deref()
                        {
                            return Err(AppError::InvalidInput(
                                "This device is already included as local usage".into(),
                            ));
                        }
                        let previous = db.get_setting(&format!("lan_grant_v1:{peer_id}"))?;
                        lan_usage::pairing::store_grant(db, &peer_id, &reverse_token)?;
                        Ok((previous, identity::device_info(db)?))
                    })
                    .await?
                };
                let token = match lan_usage::client::pair(host, port, &local_info, settings.port, code, &reverse_token).await {
                    Ok(token) => token,
                    Err(error) => {
                        let peer_id = info.device_id.clone();
                        self.call_blocking(move |db| {
                            lan_usage::pairing::restore_grant(db, &peer_id, &reverse_token, previous_grant.as_deref())
                        })
                        .await?;
                        return Err(error);
                    }
                };
                {
                    let (peer_id, peer_name, host) =
                        (info.device_id.clone(), info.device_name.clone(), host.to_string());
                    self.call_blocking(move |db| {
                        storage::register_device(db, &peer_id, &peer_name, &host, port)?;
                        db.set_setting(&format!("lan_outgoing_token_v1:{peer_id}"), &token)
                    })
                    .await?;
                }
                let db = self.db.clone();
                let device_id = info.device_id.clone();
                tokio::spawn(async move {
                    if let Err(error) = lan_usage::client::sync_device(&db, &device_id).await {
                        log::warn!("首次设备同步失败: {error}");
                    }
                });
                Ok(json!({"device":info,"syncPending":true}))
            }
            "lan_device_enabled" => {
                let id = string_arg(&args, "deviceId")?.to_string();
                let enabled = args.get("enabled").and_then(Value::as_bool).unwrap_or(false);
                self.call_blocking(move |db| {
                    storage::set_device_enabled(db, &id, enabled)?;
                    Ok(json!(true))
                })
                .await
            }
            "lan_disconnect" => {
                let id = string_arg(&args, "deviceId")?.to_string();
                let remote_revoked = lan_usage::client::revoke_remote(&self.db, &id).await.is_ok();
                self.call_blocking(move |db| {
                    db.delete_setting(&format!("lan_outgoing_token_v1:{id}"))?;
                    lan_usage::pairing::revoke(db, &id)?;
                    storage::remove_device(db, &id)?;
                    Ok(json!({"remoteRevoked":remote_revoked}))
                })
                .await
            }
            _ => Err(AppError::InvalidInput(format!(
                "Unknown usage operation: {op}"
            ))),
        }
    }
}
fn to_json(value: impl Serialize) -> Result<Value, AppError> {
    serde_json::to_value(value).map_err(|e| AppError::Message(e.to_string()))
}
/// Encode a structured error into an envelope the frontend can parse.
///
/// The error channel of `usage_call` is a plain string: the Tauri command returns
/// `Result<Value, String>` and the browser bridge flattens the Rust-side `AppError` into text in
/// an `error` field as well. To keep `code` and `details` (for example the affected dates of a 422)
/// the command has to carry its own envelope, which the frontend unwraps via the
/// `observatoryError` marker.
///
/// The envelope uses a dedicated marker field instead of "parse it if it happens to be JSON",
/// so that a plain error message that is valid JSON is not misdetected.
fn structured_usage_error(error: lan_usage::public_api::ApiError) -> AppError {
    AppError::Message(serde_json::json!({ "observatoryError": error.to_json() }).to_string())
}
fn string_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, AppError> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| AppError::InvalidInput(format!("Missing {key}")))
}

pub(crate) fn read_enabled_sources(db: &Database) -> Result<Vec<String>, AppError> {
    match db.get_setting("usage_enabled_sources")? {
        Some(value) => serde_json::from_str(&value)
            .map_err(|e| AppError::Config(format!("Invalid source selection: {e}"))),
        None => Ok(APPS.iter().map(|a| a.0.to_string()).collect()),
    }
}

/// Number of days detail records are retained: older detail records are aggregated by day into
/// `usage_daily_rollups` at the end of every scan and then deleted ([`Database::rollup_and_prune`]).
const PRUNE_RETENTION_DAYS: i64 = 30;

pub(crate) fn scan_database(
    db: &Database,
    enabled: &[String],
    trigger: crate::activity::ScanTrigger,
) -> session_usage::SessionSyncResult {
    let started = std::time::Instant::now();
    let mut result = session_usage::SessionSyncResult::default();
    type Scanner = fn(&Database) -> Result<session_usage::SessionSyncResult, AppError>;
    let scanners: &[(&str, Scanner)] = &[
        ("claude", session_usage::sync_claude_session_logs),
        (
            "codex",
            crate::services::session_usage_codex::sync_codex_usage,
        ),
        (
            "gemini",
            crate::services::session_usage_gemini::sync_gemini_usage,
        ),
        (
            "opencode",
            crate::services::session_usage_opencode::sync_opencode_usage,
        ),
        (
            "grokbuild",
            crate::services::session_usage_grokbuild::sync_grokbuild_usage,
        ),
        ("pi", crate::services::session_usage_pi::sync_pi_usage),
        (
            "mcode",
            crate::services::session_usage_mcode::sync_mcode_usage,
        ),
        ("agy", crate::services::session_usage_agy::sync_agy_usage),
        ("kimi", crate::services::session_usage_kimi::sync_kimi_usage),
        ("dsh", crate::services::session_usage_dsh::sync_dsh_usage),
        (
            "zcode",
            crate::services::session_usage_zcode::sync_zcode_usage,
        ),
    ];
    // Take the cursor and detail high-water marks once before and once after each collector, so the activity log can derive per-file read records.
    let mut probe = crate::activity::probe(db);
    let mut sources = Vec::new();
    for (id, scan) in scanners {
        if !enabled.iter().any(|v| v == id) {
            continue;
        }
        let source_started = std::time::Instant::now();
        let outcome = scan(db);
        let after = crate::activity::probe(db);
        let logged = match &outcome {
            Ok(r) => Ok(r.clone()),
            Err(e) => Err(e.to_string()),
        };
        match outcome {
            Ok(r) => result.merge(r),
            Err(e) => result.errors.push(format!("{id}: {e}")),
        }
        sources.push(crate::activity::SourceScan {
            id,
            name: APPS.iter().find(|app| app.0 == *id).map_or(id, |app| app.1),
            duration_ms: source_started.elapsed().as_millis() as i64,
            result: logged,
            rows: crate::activity::new_rows(db, probe.max_rowid(), after.max_rowid()),
            before: std::mem::replace(&mut probe, after.clone()),
            after,
        });
    }
    // Aggregate the detail records outside the retention window into the per-day table and then delete
    // them. There is an EXISTS early exit inside, so in steady state each round is just one index
    // probe; pruning does not count towards imported (that means "newly imported"), and the data
    // changes caused by pruning are covered by the revision and the write epoch of
    // record_sync_completed. A failed pruning does not block the result of this scan round.
    if let Err(e) = db.rollup_and_prune(PRUNE_RETENTION_DAYS) {
        result.errors.push(format!("剪枝: {e}"));
    }
    // If the pricing file changed at startup, run the missing full-table backfill here; on failure the flag is kept so the next round retries.
    if model_pricing::backfill_pending() {
        match db.backfill_missing_usage_costs() {
            Ok(updated) => {
                model_pricing::clear_backfill_pending();
                if updated > 0 {
                    log::info!("启动定价变更后已回填 {updated} 条缺失的用量成本");
                }
            }
            Err(e) => result.errors.push(format!("定价回填: {e}")),
        }
    }
    session_usage::notify_sync_result(&result);
    session_usage::record_sync_completed(chrono::Utc::now().timestamp_millis());
    crate::activity::record_scan(db, &trigger, started, sources);
    result
}
