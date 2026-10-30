//! A cached, in-memory statistical projection. Native usage queries run unchanged on
//! local records plus peer snapshots. No remote record is written into native logs.
use std::sync::{Arc, LazyLock, Mutex};

use rusqlite::{params, params_from_iter, types::Value, Connection};
use serde::de::DeserializeOwned;

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::services::usage_stats::provider_name_coalesce;

use super::{identity, DeviceInfo, UsageExport};

struct CachedProjection {
    key: (usize, String, i64, i64, String),
    db: Arc<Database>,
}

// Keep aggregate and one selected peer projection to avoid rebuilding each when both windows are open.
static CACHE: LazyLock<Mutex<Vec<CachedProjection>>> = LazyLock::new(|| Mutex::new(Vec::new()));

fn sql_error(error: rusqlite::Error) -> AppError {
    AppError::Database(error.to_string())
}

fn namespace(device: &str) -> String {
    let hex: String = device
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("lan@{hex}|")
}

fn create_table(source: &Connection, target: &Connection, table: &str) -> Result<(), AppError> {
    let sql: String = source
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    target.execute_batch(&sql).map_err(sql_error)
}

fn copy_usage_table(
    source: &Connection,
    target: &Connection,
    table: &str,
    origin: &str,
) -> Result<(), AppError> {
    let mut stmt = source
        .prepare(&format!("SELECT * FROM {table}"))
        .map_err(sql_error)?;
    let columns: Vec<String> = stmt
        .column_names()
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    let placeholders = vec!["?"; columns.len()].join(",");
    let mut insert = target
        .prepare(&format!("INSERT INTO {table} VALUES ({placeholders})"))
        .map_err(sql_error)?;
    let mut rows = stmt.query([]).map_err(sql_error)?;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let mut values = Vec::with_capacity(columns.len());
        for (index, name) in columns.iter().enumerate() {
            let value: Value = row.get(index).map_err(sql_error)?;
            values.push(match (name.as_str(), value) {
                ("request_id" | "provider_id" | "session_id", Value::Text(value)) => {
                    Value::Text(format!("{origin}{value}"))
                }
                (_, value) => value,
            });
        }
        insert
            .execute(params_from_iter(values))
            .map_err(sql_error)?;
    }
    Ok(())
}

fn add_provider(
    target: &Connection,
    origin: &str,
    id: &str,
    app: &str,
    name: &str,
) -> Result<String, AppError> {
    let provider = format!("{origin}{}", if id.is_empty() { "_legacy" } else { id });
    target.execute(
        "INSERT OR IGNORE INTO providers (id, app_type, name, settings_config) VALUES (?1, ?2, ?3, '{}')",
        params![provider, app, if name.is_empty() { "Unknown provider (legacy peer)" } else { name }],
    ).map_err(sql_error)?;
    Ok(provider)
}

fn import_peer(target: &Connection, export: &UsageExport) -> Result<(), AppError> {
    let origin = namespace(&export.device.device_id);
    let selection = export
        .shared_app_types
        .as_ref()
        .map(|apps| super::normalize_shared_app_types(apps));
    for row in &export.events {
        if selection
            .as_ref()
            .is_some_and(|apps| !apps.contains(&row.app_type))
        {
            continue;
        }
        let meta = &row.metadata;
        let provider = add_provider(
            target,
            &origin,
            &meta.provider_id,
            &row.app_type,
            &meta.provider_name,
        )?;
        target.execute(
            "INSERT OR REPLACE INTO proxy_request_logs (
                request_id, provider_id, app_type, model, request_model, pricing_model,
                session_id, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                input_token_semantics, input_cost_usd, output_cost_usd, cache_read_cost_usd,
                cache_creation_cost_usd, total_cost_usd, latency_ms, first_token_ms, duration_ms,
                status_code, created_at, data_source, is_streaming, cost_multiplier)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25)",
            params![format!("{origin}{}", row.request_id), provider, row.app_type, row.model,
                meta.request_model, meta.pricing_model, row.session_id.as_ref().map(|id| format!("{origin}{id}")),
                row.input_tokens, row.output_tokens, row.cache_read_tokens, row.cache_creation_tokens,
                row.input_token_semantics, row.input_cost_usd, row.output_cost_usd, row.cache_read_cost_usd,
                row.cache_creation_cost_usd, row.total_cost_usd, row.latency_ms, meta.first_token_ms,
                meta.duration_ms, row.status_code, row.created_at, row.data_source, meta.is_streaming, meta.cost_multiplier.as_deref().unwrap_or("1")],
        ).map_err(sql_error)?;
    }
    for row in &export.daily {
        if selection
            .as_ref()
            .is_some_and(|apps| !apps.contains(&row.app_type))
        {
            continue;
        }
        let meta = &row.metadata;
        let provider = add_provider(
            target,
            &origin,
            &meta.provider_id,
            &row.app_type,
            &meta.provider_name,
        )?;
        target
            .execute(
                "INSERT OR REPLACE INTO usage_daily_rollups (
                date, app_type, provider_id, model, request_model, pricing_model,
                request_count, success_count, input_tokens, output_tokens, cache_read_tokens,
                cache_creation_tokens, input_token_semantics, total_cost_usd, avg_latency_ms)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
                params![
                    row.date,
                    row.app_type,
                    provider,
                    row.model,
                    meta.request_model.as_deref().unwrap_or(""),
                    meta.pricing_model.as_deref().unwrap_or(""),
                    row.request_count,
                    row.success_count,
                    row.input_tokens,
                    row.output_tokens,
                    row.cache_read_tokens,
                    row.cache_creation_tokens,
                    row.input_token_semantics,
                    row.total_cost_usd,
                    row.avg_latency_ms
                ],
            )
            .map_err(sql_error)?;
    }
    Ok(())
}

/// Older installed versions stored only the legacy tables. Keep that offline history
/// visible until the next successful sync replaces it with a richer snapshot.
fn legacy_rows<T: DeserializeOwned>(
    source: &Connection,
    table: &str,
    device: &str,
) -> Result<Vec<T>, AppError> {
    let mut stmt = source
        .prepare(&format!("SELECT * FROM {table} WHERE device_id = ?1"))
        .map_err(sql_error)?;
    let names: Vec<String> = stmt
        .column_names()
        .iter()
        .map(|name| {
            let mut parts = name.split('_');
            let mut camel = parts.next().unwrap_or_default().to_string();
            for part in parts {
                let mut chars = part.chars();
                if let Some(first) = chars.next() {
                    camel.extend(first.to_uppercase());
                    camel.extend(chars);
                }
            }
            camel
        })
        .collect();
    let mut rows = stmt.query([device]).map_err(sql_error)?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().map_err(sql_error)? {
        let mut object = serde_json::Map::new();
        for (index, name) in names.iter().enumerate() {
            let value: Value = row.get(index).map_err(sql_error)?;
            let value = match value {
                Value::Null => serde_json::Value::Null,
                Value::Integer(value) => serde_json::json!(value),
                Value::Real(value) => serde_json::json!(value),
                Value::Text(value) => serde_json::Value::String(value),
                Value::Blob(_) => continue,
            };
            object.insert(name.clone(), value);
        }
        result.push(
            serde_json::from_value(serde_json::Value::Object(object))
                .map_err(|e| AppError::Message(e.to_string()))?,
        );
    }
    Ok(result)
}

pub fn database(db: &Database) -> Result<Arc<Database>, AppError> {
    database_selected(db, None)
}

pub fn database_for_peer(db: &Database, peer_id: &str) -> Result<Arc<Database>, AppError> {
    database_selected(db, Some(peer_id))
}

fn database_selected(db: &Database, peer_id: Option<&str>) -> Result<Arc<Database>, AppError> {
    let device_id = identity::device_id(db)?;
    let conn = lock_conn!(db.conn);
    let read = conn.unchecked_transaction().map_err(sql_error)?;
    let changes = read
        .query_row("SELECT total_changes()", [], |row| row.get::<_, i64>(0))
        .map_err(sql_error)?;
    let version = read
        .query_row("PRAGMA data_version", [], |row| row.get::<_, i64>(0))
        .map_err(sql_error)?;
    let key = (
        std::ptr::from_ref(db) as usize,
        device_id.clone(),
        changes,
        version,
        peer_id.unwrap_or("").to_owned(),
    );
    if let Some(peer) = peer_id {
        let exists: bool = read.query_row("SELECT EXISTS(SELECT 1 FROM lan_devices WHERE device_id = ?1 AND enabled = 1 AND EXISTS (SELECT 1 FROM settings WHERE key = 'lan_outgoing_token_v1:' || lan_devices.device_id))", [peer], |r| r.get(0)).map_err(sql_error)?;
        if !exists {
            return Err(AppError::Message("设备不存在或未启用".into()));
        }
    }
    let mut cache = CACHE.lock().map_err(|e| AppError::Message(e.to_string()))?;
    if let Some(cached) = cache.iter().find(|cached| cached.key == key) {
        return Ok(cached.db.clone());
    }

    let mut target = Connection::open_in_memory().map_err(sql_error)?;
    let tx = target.transaction().map_err(sql_error)?;
    // Only statistics and display names are copied. No settings, keys, messages or
    // provider configurations enter the projection. Empty pricing avoids repricing
    // remote records with the receiving machine's local price list.
    for table in [
        "proxy_request_logs",
        "usage_daily_rollups",
        "providers",
        "model_pricing",
    ] {
        create_table(&read, &tx, table)?;
    }
    let origin = namespace(&device_id);
    for table in ["proxy_request_logs", "usage_daily_rollups"]
        .into_iter()
        .filter(|_| peer_id.is_none())
    {
        copy_usage_table(&read, &tx, table, &origin)?;
        let name = provider_name_coalesce("l", "p");
        let mut providers = read.prepare(&format!("SELECT DISTINCT l.provider_id, l.app_type, {name} FROM {table} l LEFT JOIN providers p ON p.id = l.provider_id AND p.app_type = l.app_type")).map_err(sql_error)?;
        let rows = providers
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(sql_error)?;
        for row in rows {
            let (id, app, name) = row.map_err(sql_error)?;
            add_provider(&tx, &origin, &id, &app, &name)?;
        }
    }
    {
        let mut peers = read.prepare("SELECT d.device_id, d.device_name, s.payload FROM lan_devices d LEFT JOIN lan_usage_snapshots s ON d.device_id = s.device_id WHERE d.enabled = 1 AND EXISTS (SELECT 1 FROM settings WHERE key = 'lan_outgoing_token_v1:' || d.device_id) AND d.device_id <> ?1 AND (?2 IS NULL OR d.device_id = ?2)").map_err(sql_error)?;
        let rows = peers
            .query_map(params![device_id, peer_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(sql_error)?;
        for row in rows {
            let (id, name, payload) = row.map_err(sql_error)?;
            let export = if let Some(payload) = payload {
                serde_json::from_str::<UsageExport>(&payload)
                    .map_err(|e| AppError::Message(e.to_string()))?
            } else {
                UsageExport {
                    snapshot_version: 0,
                    device: DeviceInfo {
                        device_id: id.clone(),
                        device_name: name,
                        app_version: String::new(),
                        platform: String::new(),
                        started_at: 0,
                    },
                    shared_app_types: None,
                    events: legacy_rows(&read, "lan_usage_events", &id)?,
                    daily: legacy_rows(&read, "lan_usage_daily", &id)?,
                }
            };
            import_peer(&tx, &export)?;
        }
    }
    // Provider selectors are display-name based. Combine matching names/apps across
    // machines, preserving all model/alias dimensions and normalizing mixed cache semantics.
    let fresh = crate::services::sql_helpers::fresh_input_sql("r");
    tx.execute_batch(&format!("INSERT OR IGNORE INTO providers (id, app_type, name, settings_config)
        SELECT 'group@' || hex(name), app_type, name, '{{}}' FROM providers;
        UPDATE proxy_request_logs SET provider_id = (
            SELECT 'group@' || hex(p.name) FROM providers p
            WHERE p.id = proxy_request_logs.provider_id AND p.app_type = proxy_request_logs.app_type);
        CREATE TEMP TABLE merged_rollups AS
        SELECT r.date, r.app_type, 'group@' || hex(p.name) AS provider_id,
            r.model, r.request_model, r.pricing_model,
            SUM(r.request_count) AS request_count, SUM(r.success_count) AS success_count,
            SUM({fresh}) AS input_tokens, SUM(r.output_tokens) AS output_tokens,
            SUM(r.cache_read_tokens) AS cache_read_tokens, SUM(r.cache_creation_tokens) AS cache_creation_tokens,
            2 AS input_token_semantics, CAST(SUM(CAST(r.total_cost_usd AS REAL)) AS TEXT) AS total_cost_usd,
            CAST(COALESCE(SUM(CAST(r.avg_latency_ms AS REAL) * r.request_count) / NULLIF(SUM(r.request_count), 0), 0) AS INTEGER) AS avg_latency_ms
        FROM usage_daily_rollups r JOIN providers p ON r.provider_id = p.id AND r.app_type = p.app_type
        GROUP BY r.date, r.app_type, p.name, r.model, r.request_model, r.pricing_model;
        DELETE FROM usage_daily_rollups;
        INSERT INTO usage_daily_rollups (date,app_type,provider_id,model,request_model,pricing_model,
            request_count,success_count,input_tokens,output_tokens,cache_read_tokens,cache_creation_tokens,
            input_token_semantics,total_cost_usd,avg_latency_ms)
        SELECT * FROM merged_rollups;
        DROP TABLE merged_rollups;"))
        .map_err(sql_error)?;
    // Preserve the native indexes used by the original filtering and dedup queries.
    let mut indexes = read.prepare("SELECT sql FROM sqlite_master WHERE type = 'index' AND tbl_name IN ('proxy_request_logs', 'usage_daily_rollups') AND sql IS NOT NULL").map_err(sql_error)?;
    for sql in indexes
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(sql_error)?
    {
        tx.execute_batch(&sql.map_err(sql_error)?)
            .map_err(sql_error)?;
    }
    drop(indexes);
    tx.commit().map_err(sql_error)?;
    read.commit().map_err(sql_error)?;
    let projection = Arc::new(Database {
        conn: Mutex::new(target),
        log_count_cache: Mutex::new(None),
    });
    cache.retain(|cached| cached.key.0 != key.0 || cached.key.4 != key.4);
    if cache.len() >= 2 {
        cache.remove(0);
    }
    cache.push(CachedProjection {
        key,
        db: projection.clone(),
    });
    Ok(projection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lan_usage::storage;
    use crate::services::usage_stats::LogFilters;

    fn seed(db: &Database, source: &str) {
        let conn = db.conn.lock().unwrap();
        conn.execute("INSERT INTO providers (id, app_type, name, settings_config) VALUES ('shared', 'codex', 'Shared', '{\"apiKey\":\"must-not-copy\"}')", []).unwrap();
        conn.execute(
            "INSERT INTO proxy_request_logs (
            request_id, provider_id, app_type, model, request_model, pricing_model,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            input_token_semantics, total_cost_usd, latency_ms, first_token_ms, duration_ms,
            is_streaming, status_code, created_at, data_source)
            VALUES ('same-request', 'shared', 'codex', 'raw-model', 'alias', 'priced-model',
                100, 50, 20, 10, 1, '3', 1000, 100, 1000, 1, 200, 1714651200, ?1)",
            [source],
        )
        .unwrap();
    }

    fn connect(db: &Database, peer: &Database) -> UsageExport {
        let export = storage::export_local_usage(peer).unwrap();
        storage::register_device(db, &export.device.device_id, "Peer", "127.0.0.1", 17890).unwrap();
        storage::store_export(db, &export).unwrap();
        export
    }

    #[test]
    fn every_native_query_uses_the_same_combined_filters_and_cache_semantics() {
        let local = Database::memory().unwrap();
        let peer = Database::memory().unwrap();
        seed(&local, "proxy");
        seed(&peer, "proxy");
        peer.conn.lock().unwrap().execute("INSERT INTO usage_daily_rollups (
            date, app_type, provider_id, model, request_model, pricing_model, request_count,
            success_count, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            input_token_semantics, total_cost_usd, avg_latency_ms)
            VALUES ('2024-05-01','codex','shared','raw-model','alias','priced-model',4,4,400,200,80,40,1,'12',1000)", []).unwrap();
        let export = connect(&local, &peer);
        let projected = database(&local).unwrap();
        let again = database(&local).unwrap();
        assert!(Arc::ptr_eq(&projected, &again));
        let summary = projected
            .get_usage_summary(
                None,
                None,
                Some("codex"),
                Some("Shared"),
                Some("priced-model"),
            )
            .unwrap();
        assert_eq!(summary.total_requests, 6);
        assert_eq!(summary.total_input_tokens, 420);
        assert_eq!(summary.total_output_tokens, 300);
        assert_eq!(summary.total_cache_read_tokens, 120);
        assert_eq!(summary.total_cache_creation_tokens, 60);
        assert_eq!(summary.real_total_tokens, 900);
        assert_eq!(summary.total_cost.parse::<f64>().unwrap(), 18.0);
        assert_eq!(summary.success_rate, 100.0);
        assert!((summary.cache_hit_rate - 0.2).abs() < 1e-9);
        let by_app = projected
            .get_usage_summary_by_app(None, None, Some("Shared"), Some("priced-model"))
            .unwrap();
        assert_eq!(by_app[0].summary.total_requests, 6);
        let providers = projected
            .get_provider_stats(
                None,
                None,
                Some("codex"),
                Some("Shared"),
                Some("priced-model"),
            )
            .unwrap();
        // Provider IDs are device-local; rows still add up to the same filtered total.
        assert_eq!(providers.len(), 1);
        assert_eq!(
            providers.iter().map(|row| row.request_count).sum::<u64>(),
            6
        );
        let models = projected
            .get_model_stats(
                None,
                None,
                Some("codex"),
                Some("Shared"),
                Some("priced-model"),
            )
            .unwrap();
        assert_eq!(models[0].request_count, 6);
        let trends = projected
            .get_daily_trends(
                Some(1714435200),
                Some(1714780800),
                Some("codex"),
                Some("Shared"),
                Some("priced-model"),
            )
            .unwrap();
        assert_eq!(trends.iter().map(|row| row.request_count).sum::<u64>(), 6);
        assert_eq!(
            projected
                .get_first_usage_date(Some("codex"), Some("Shared"), Some("priced-model"))
                .unwrap()
                .as_deref(),
            Some("2024-05-01")
        );
        let filters = LogFilters {
            app_type: Some("codex".into()),
            provider_name: Some("Shared".into()),
            model: Some("priced-model".into()),
            ..Default::default()
        };
        let first = projected.get_request_logs(&filters, 0, 1).unwrap();
        let second = projected.get_request_logs(&filters, 1, 1).unwrap();
        assert_eq!(first.total, 2);
        assert_ne!(first.data[0].request_id, second.data[0].request_id);
        let detail = projected
            .get_request_detail(&first.data[0].request_id)
            .unwrap()
            .unwrap();
        assert_eq!(detail.pricing_model.as_deref(), Some("priced-model"));
        assert_eq!(detail.first_token_ms, Some(100));
        assert_eq!(
            projected
                .get_usage_summary(None, None, None, Some("Other"), None)
                .unwrap()
                .total_requests,
            0
        );
        // Original tables never receive remote copies; configs and price lists stay local.
        assert_eq!(
            local
                .get_usage_summary(None, None, None, None, None)
                .unwrap()
                .total_requests,
            1
        );
        assert_eq!(
            projected
                .conn
                .lock()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM providers WHERE settings_config <> '{}'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        storage::store_export(&local, &export).unwrap();
        assert_eq!(
            database(&local)
                .unwrap()
                .get_usage_summary(None, None, None, None, None)
                .unwrap()
                .total_requests,
            6
        );
        storage::set_device_enabled(&local, &export.device.device_id, false).unwrap();
        assert_eq!(
            database(&local)
                .unwrap()
                .get_usage_summary(None, None, None, None, None)
                .unwrap()
                .total_requests,
            1
        );
        storage::set_device_enabled(&local, &export.device.device_id, true).unwrap();
        let mut revoked = export.clone();
        revoked.shared_app_types = Some(vec![]);
        storage::store_export(&local, &revoked).unwrap();
        assert_eq!(
            database(&local)
                .unwrap()
                .get_usage_summary(None, None, None, None, None)
                .unwrap()
                .total_requests,
            1
        );
    }

    #[test]
    fn identical_records_on_different_devices_do_not_cross_deduplicate() {
        let local = Database::memory().unwrap();
        let peer = Database::memory().unwrap();
        seed(&local, "codex_session");
        seed(&peer, "proxy");
        connect(&local, &peer);
        assert_eq!(
            database(&local)
                .unwrap()
                .get_usage_summary(None, None, None, None, None)
                .unwrap()
                .total_requests,
            2
        );
        local.conn.lock().unwrap().execute("INSERT INTO proxy_request_logs (
            request_id,provider_id,app_type,model,input_tokens,output_tokens,cache_read_tokens,
            cache_creation_tokens,input_token_semantics,total_cost_usd,latency_ms,status_code,created_at,data_source)
            VALUES ('local-proxy','shared','codex','raw-model',100,50,20,10,1,'3',1000,200,1714651200,'proxy')",[]).unwrap();
        // Local proxy/session duplicates still collapse, independently of the remote proxy.
        assert_eq!(
            database(&local)
                .unwrap()
                .get_usage_summary(None, None, None, None, None)
                .unwrap()
                .total_requests,
            2
        );
        let export = storage::export_local_usage(&local).unwrap();
        assert_eq!(
            export.events.len(),
            1,
            "do not re-export remote copies or native duplicates"
        );
    }

    #[test]
    fn same_device_is_never_added_back_into_its_own_totals() {
        let db = Database::memory().unwrap();
        seed(&db, "proxy");
        connect(&db, &db);
        assert_eq!(
            database(&db)
                .unwrap()
                .get_usage_summary(None, None, None, None, None)
                .unwrap()
                .total_requests,
            1
        );
    }

    #[tokio::test]
    async fn http_sync_retains_provider_filters_and_replaces_deleted_rows() {
        use axum::{routing::get, Json, Router};
        let local = Database::memory().unwrap();
        let peer = Database::memory().unwrap();
        seed(&local, "proxy");
        seed(&peer, "proxy");
        let export = storage::export_local_usage(&peer).unwrap();
        let payload = Arc::new(Mutex::new(export.clone()));
        let served = payload.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = Router::new().route(
            "/api/lan/usage",
            get(move || {
                let served = served.clone();
                async move { Json(served.lock().unwrap().clone()) }
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        storage::register_device(&local, &export.device.device_id, "Peer", "127.0.0.1", port)
            .unwrap();
        super::super::client::sync_device(&local, &export.device.device_id)
            .await
            .unwrap();
        assert_eq!(
            database(&local)
                .unwrap()
                .get_usage_summary(None, None, None, Some("Shared"), Some("priced-model"))
                .unwrap()
                .total_requests,
            2
        );
        payload.lock().unwrap().events.clear();
        super::super::client::sync_device(&local, &export.device.device_id)
            .await
            .unwrap();
        assert_eq!(
            database(&local)
                .unwrap()
                .get_usage_summary(None, None, None, None, None)
                .unwrap()
                .total_requests,
            1
        );
        let mut wrong = export.clone();
        wrong.device.device_id = "different-device".into();
        *payload.lock().unwrap() = wrong;
        assert!(
            super::super::client::sync_device(&local, &export.device.device_id)
                .await
                .is_err()
        );
        server.abort();
    }
    #[test]
    fn exported_totals_preserve_late_details_on_a_day_that_already_has_rollups() {
        let peer = Database::memory().unwrap();
        seed(&peer, "proxy");
        {
            let conn = peer.conn.lock().unwrap();
            conn.execute("UPDATE proxy_request_logs SET created_at = 1714564800", [])
                .unwrap();
            conn.execute("INSERT INTO usage_daily_rollups (date,app_type,provider_id,model,request_model,pricing_model,
                request_count,success_count,input_tokens,input_token_semantics,total_cost_usd)
                VALUES ('2024-05-01','codex','shared','raw-model','alias','priced-model',4,4,280,2,'12')", []).unwrap();
        }
        let native = peer
            .get_usage_summary(None, None, None, Some("Shared"), Some("priced-model"))
            .unwrap();
        let receiver = Database::memory().unwrap();
        let export = connect(&receiver, &peer);
        assert_eq!(export.events.len(), 1);
        assert_eq!(export.daily.len(), 1);
        let combined = database(&receiver)
            .unwrap()
            .get_usage_summary(None, None, None, Some("Shared"), Some("priced-model"))
            .unwrap();
        assert_eq!(combined.total_requests, native.total_requests);
        assert_eq!(combined.total_input_tokens, native.total_input_tokens);
        assert_eq!(combined.total_cost, native.total_cost);
    }
}
