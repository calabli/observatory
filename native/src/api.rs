use crate::{
    database::{lock_conn, Database},
    error::AppError,
    lan_usage::{self, identity, storage},
    services::{model_pricing, session_usage, usage_stats::LogFilters},
};
use chrono::TimeZone;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};

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
    page: u32,
    page_size: u32,
    request_id: String,
}

pub struct UsageBackend {
    pub db: Arc<Database>,
    scan_lock: Arc<tokio::sync::Mutex<()>>,
    _instance_lock: Option<std::fs::File>,
}
impl UsageBackend {
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
        model_pricing::sync_local_model_pricing(&db)?;
        let empty = db
            .get_usage_summary(None, None, None, None, None)?
            .total_requests
            == 0;
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
        lan_usage::start_background(self.db.clone());
        let db = self.db.clone();
        let lock = self.scan_lock.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let interval = db
                    .get_setting("usage_refresh_interval")
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| "30".into());
                if interval == "manual"
                    || db
                        .get_setting("usage_initial_import_error")
                        .ok()
                        .flatten()
                        .is_some()
                {
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
                if let Err(e) =
                    tokio::task::spawn_blocking(move || scan_database(&source, &enabled)).await
                {
                    log::warn!("Background usage scan failed: {e}");
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
    fn status(&self) -> Result<Value, AppError> {
        let source = Self::cc_switch_path();
        let info = identity::device_info(&self.db)?;
        let stats = self.db.get_usage_summary_by_app(None, None, None, None)?;
        let enabled = self.enabled_sources()?;
        let initial_import_error = self.db.get_setting("usage_initial_import_error")?;
        let initial_import_notice = self.db.get_setting("usage_initial_import_notice")?;
        let refresh_interval = self
            .db
            .get_setting("usage_refresh_interval")?
            .unwrap_or_else(|| "30".into());
        let conn = lock_conn!(self.db.conn);
        let requests: i64 =
            conn.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |r| r.get(0))?;
        Ok(
            json!({"version":env!("CARGO_PKG_VERSION"),"dataDir":crate::config::get_app_config_dir(),"databasePath":crate::config::get_app_config_dir().join("observatory.db"),"device":info,"ccSwitchPath":source,"ccSwitchAvailable":source.is_file(),"detailRecords":requests,"lastScanAt":session_usage::last_sync_completed_at(),"initialImportError":initial_import_error,"initialImportNotice":initial_import_notice,"refreshInterval":refresh_interval,"sources":APPS.iter().map(|(id,name,native)|json!({"id":id,"name":name,"nativeSupported":native,"enabled":enabled.iter().any(|v|v==id),"requests":stats.iter().find(|s|&s.app_type==id).map(|s|s.summary.total_requests).unwrap_or(0)})).collect::<Vec<_>>() }),
        )
    }
    fn query_database(&self, multi: bool) -> Result<Arc<Database>, AppError> {
        if multi {
            lan_usage::projection::database(&self.db)
        } else {
            Ok(self.db.clone())
        }
    }
    pub async fn call(&self, op: &str, args: Value) -> Result<Value, AppError> {
        match op {
            "status" => self.status(),
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
                self.db.set_setting("usage_refresh_interval", interval)?;
                self.status()
            }
            "dashboard" => {
                let q: Query = serde_json::from_value(args)
                    .map_err(|e| AppError::InvalidInput(e.to_string()))?;
                let db = self.query_database(q.multi_device)?;
                let f = &q.filters;
                let (s, e, a, p, m) = (
                    f.start_date,
                    f.end_date,
                    f.app_type.as_deref(),
                    f.provider_name.as_deref(),
                    f.model.as_deref(),
                );
                let summary = db.get_usage_summary(s, e, a, p, m)?;
                let first_date = db.get_first_usage_date(a, p, m)?;
                let trend_start = s.or_else(|| {
                    first_date
                        .as_deref()
                        .and_then(|date| chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
                        .and_then(|day| day.and_hms_opt(0, 0, 0))
                        .and_then(|day| chrono::Local.from_local_datetime(&day).earliest())
                        .map(|time| time.timestamp())
                });
                let trends = db.get_daily_trends(
                    trend_start,
                    Some(e.unwrap_or_else(|| chrono::Utc::now().timestamp())),
                    a,
                    p,
                    m,
                )?;
                let providers = db.get_provider_stats(s, e, a, p, m)?;
                let models = db.get_model_stats(s, e, a, p, m)?;
                let mut logs =
                    db.get_request_logs(f, q.page.max(1) - 1, q.page_size.clamp(1, 200).max(20))?;
                logs.page = q.page.max(1);
                let by_app = db.get_usage_summary_by_app(s, e, p, m)?;
                let options_models = db.get_model_stats(s, e, a, None, None)?;
                let options_providers = db.get_provider_stats(s, e, a, None, None)?;
                Ok(
                    json!({"summary":summary,"trends":trends,"providers":providers,"models":models,"logs":logs,"byApp":by_app,"firstDate":first_date,"options":{"models":options_models.iter().map(|m|&m.model).collect::<Vec<_>>(),"providers":options_providers.iter().map(|p|&p.provider_name).collect::<std::collections::BTreeSet<_>>()}}),
                )
            }
            "request_detail" => {
                let q: Query = serde_json::from_value(args)
                    .map_err(|e| AppError::InvalidInput(e.to_string()))?;
                let db = self.query_database(q.multi_device)?;
                to_json(db.get_request_detail(&q.request_id)?)
            }
            "sources_save" => {
                let selected: Vec<String> =
                    serde_json::from_value(args.get("enabled").cloned().unwrap_or(json!([])))
                        .map_err(|e| AppError::InvalidInput(e.to_string()))?;
                if selected.iter().any(|id| !APPS.iter().any(|a| a.0 == id)) {
                    return Err(AppError::InvalidInput("Unknown application source".into()));
                }
                self.db.set_setting(
                    "usage_enabled_sources",
                    &serde_json::to_string(&selected).unwrap(),
                )?;
                self.status()
            }
            "sync" => {
                let result = {
                    let _guard = self.scan_lock.lock().await;
                    let recent = session_usage::last_sync_completed_at()
                        .is_some_and(|at| chrono::Utc::now().timestamp_millis() - at < 5000);
                    if args
                        .get("automatic")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                        && recent
                    {
                        session_usage::SessionSyncResult::default()
                    } else {
                        let enabled = self.enabled_sources()?;
                        let db = self.db.clone();
                        tokio::task::spawn_blocking(move || scan_database(&db, &enabled))
                            .await
                            .map_err(|e| AppError::Message(e.to_string()))?
                    }
                };
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
                self.db.delete_setting("usage_initial_import_error")?;
                to_json(imported)
            }
            "pricing_list" => {
                self.db.ensure_model_pricing_seeded()?;
                model_pricing::sync_local_model_pricing(&self.db)?;
                let conn = lock_conn!(self.db.conn);
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
            }
            "pricing_update" => {
                let entry = serde_json::from_value(args)
                    .map_err(|e| AppError::InvalidInput(e.to_string()))?;
                to_json(model_pricing::update_model_pricing(&self.db, entry)?)
            }
            "pricing_batch" => {
                let entries =
                    serde_json::from_value(args.get("entries").cloned().unwrap_or(json!([])))
                        .map_err(|e| AppError::InvalidInput(e.to_string()))?;
                to_json(model_pricing::update_model_pricing_batch(
                    &self.db, entries,
                )?)
            }
            "pricing_delete" => {
                let id = args
                    .get("modelId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AppError::InvalidInput("Model ID is required".into()))?;
                model_pricing::delete_model_pricing(&self.db, id)?;
                Ok(json!(true))
            }
            "modelsdev_config_get" => to_json(model_pricing::get_models_dev_sync_state(&self.db)?),
            "modelsdev_config_save" => {
                let config = serde_json::from_value(args.get("config").cloned().unwrap_or(args))
                    .map_err(|e| AppError::InvalidInput(e.to_string()))?;
                model_pricing::save_models_dev_sync_config(&self.db, config)?;
                to_json(model_pricing::get_models_dev_sync_state(&self.db)?)
            }
            "modelsdev_record_result" => {
                model_pricing::record_models_dev_sync_result(
                    &self.db,
                    args.get("syncedAt").and_then(Value::as_i64),
                    args.get("error")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                )?;
                Ok(json!(true))
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
            "lan_status" => Ok(
                json!({"settings":identity::share_settings(&self.db),"local":identity::device_info(&self.db)?,"devices":storage::list_devices(&self.db)?}),
            ),
            "lan_settings_save" => {
                let settings = serde_json::from_value(args)
                    .map_err(|e| AppError::InvalidInput(e.to_string()))?;
                identity::save_share_settings(&self.db, &settings)?;
                lan_usage::ensure_server(self.db.clone()).await?;
                to_json(identity::share_settings(&self.db))
            }
            "lan_discover" => {
                let port = identity::share_settings(&self.db).port;
                let mut found = lan_usage::discovery::discover(port, 1200)
                    .await
                    .map_err(AppError::Message)?;
                if port != 17890 {
                    found.extend(
                        lan_usage::discovery::discover(17890, 1200)
                            .await
                            .map_err(AppError::Message)?,
                    );
                }
                let own = identity::device_id(&self.db)?;
                let old = self
                    .db
                    .get_setting("cc_switch_local_device_id")?
                    .unwrap_or_default();
                found.retain(|d| d.device_id != own && d.device_id != old);
                let mut ids = std::collections::HashSet::new();
                found.retain(|d| ids.insert(d.device_id.clone()));
                to_json(found)
            }
            "lan_connect" => {
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
                if info.device_id == identity::device_id(&self.db)?
                    || Some(info.device_id.clone())
                        == self.db.get_setting("cc_switch_local_device_id")?
                {
                    return Err(AppError::InvalidInput(
                        "This device is already included as local usage".into(),
                    ));
                }
                storage::register_device(&self.db, &info.device_id, &info.device_name, host, port)?;
                let result = lan_usage::client::sync_device(&self.db, &info.device_id).await;
                Ok(
                    json!({"device":info,"imported":result.as_ref().ok(),"error":result.err().map(|e|e.to_string())}),
                )
            }
            "lan_device_enabled" => {
                let id = string_arg(&args, "deviceId")?;
                storage::set_device_enabled(
                    &self.db,
                    id,
                    args.get("enabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                )?;
                Ok(json!(true))
            }
            "lan_disconnect" => {
                storage::remove_device(&self.db, string_arg(&args, "deviceId")?)?;
                Ok(json!(true))
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
fn string_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, AppError> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| AppError::InvalidInput(format!("Missing {key}")))
}

fn read_enabled_sources(db: &Database) -> Result<Vec<String>, AppError> {
    match db.get_setting("usage_enabled_sources")? {
        Some(value) => serde_json::from_str(&value)
            .map_err(|e| AppError::Config(format!("Invalid source selection: {e}"))),
        None => Ok(APPS.iter().map(|a| a.0.to_string()).collect()),
    }
}

fn scan_database(db: &Database, enabled: &[String]) -> session_usage::SessionSyncResult {
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
    for (id, scan) in scanners {
        if !enabled.iter().any(|v| v == id) {
            continue;
        }
        match scan(db) {
            Ok(r) => result.merge(r),
            Err(e) => result.errors.push(format!("{id}: {e}")),
        }
    }
    session_usage::notify_sync_result(&result);
    session_usage::record_sync_completed(chrono::Utc::now().timestamp_millis());
    result
}
