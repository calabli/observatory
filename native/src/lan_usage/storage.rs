//! Local storage for the LAN usage: registers devices and keeps the shared snapshots that are exported.

use super::{DailyRollupRow, LanDeviceStatus, UsageEventRow, UsageExport, UsageMetadata};
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::services::sql_helpers::fresh_input_sql;
use rusqlite::params;

/// Register or update one peer device.
pub fn register_device(
    db: &Database,
    device_id: &str,
    device_name: &str,
    host: &str,
    port: u16,
) -> Result<(), AppError> {
    let conn = lock_conn!(db.conn);
    conn.execute(
        "INSERT INTO lan_devices (device_id, device_name, host, port, enabled, auto_sync, last_seen_at, created_at)
         VALUES (?1, ?2, ?3, ?4, 1, 1, ?5, ?5)
         ON CONFLICT(device_id) DO UPDATE SET
           device_name = excluded.device_name,
           host = excluded.host,
           port = excluded.port,
           last_seen_at = excluded.last_seen_at",
        params![device_id, device_name, host, port, chrono::Utc::now().timestamp()],
    )
    .map_err(|e| AppError::Database(format!("登记局域网设备失败: {e}")))?;
    Ok(())
}

/// List the registered devices (including their sync state).
///
/// The device page polls every few seconds: it uses the read-only connection and one query
/// that also returns the pairing state, so it returns immediately even while a big snapshot
/// sync or a scan holds the write connection.
pub fn list_devices(db: &Database) -> Result<Vec<LanDeviceStatus>, AppError> {
    db.with_read_connection(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT device_id, device_name, host, port, enabled, last_seen_at, last_sync_at, last_error,
                        (SELECT COUNT(*) FROM lan_usage_events e WHERE e.device_id = lan_devices.device_id),
                        EXISTS (SELECT 1 FROM settings WHERE key = 'lan_outgoing_token_v1:' || lan_devices.device_id)
                      FROM lan_devices ORDER BY device_name COLLATE NOCASE",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(LanDeviceStatus {
                    device_id: row.get(0)?,
                    device_name: row.get(1)?,
                    host: row.get(2)?,
                    port: row.get::<_, i64>(3)? as u16,
                    registered: true,
                    enabled: row.get::<_, i64>(4)? != 0,
                    paired: row.get::<_, i64>(9)? != 0,
                    last_seen_at: row.get(5)?,
                    last_sync_at: row.get(6)?,
                    last_error: row.get(7)?,
                    last_imported: row.get(8)?,
                })
            })
            .map_err(|e| AppError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::Database(e.to_string()))
    })
}

/// The display name of a device: the name of a registered device, otherwise its device ID (for display in the activity log and elsewhere).
pub fn device_label(db: &Database, device_id: &str) -> String {
    db.with_read_connection(|conn| {
        conn.query_row(
            "SELECT device_name FROM lan_devices WHERE device_id = ?1",
            [device_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(AppError::from)
    })
    .ok()
    .filter(|name| !name.trim().is_empty())
    .unwrap_or_else(|| device_id.to_string())
}

/// The address (host, port) of a device.
pub fn device_endpoint(db: &Database, device_id: &str) -> Result<Option<(String, u16)>, AppError> {
    let conn = lock_conn!(db.conn);
    let mut stmt = conn
        .prepare("SELECT host, port FROM lan_devices WHERE device_id = ?1")
        .map_err(|e| AppError::Database(e.to_string()))?;
    let mut rows = stmt
        .query(params![device_id])
        .map_err(|e| AppError::Database(e.to_string()))?;
    match rows.next().map_err(|e| AppError::Database(e.to_string()))? {
        Some(row) => {
            let host: String = row.get(0).map_err(|e| AppError::Database(e.to_string()))?;
            let port: i64 = row.get(1).map_err(|e| AppError::Database(e.to_string()))?;
            Ok(Some((host, port as u16)))
        }
        None => Ok(None),
    }
}

/// Remove one device and delete the usage rows it pulled in.
///
/// All four tables are deleted in one transaction: a single commit (deleting the detail rows
/// of a big device is no longer four separate flushes) and a failure halfway cannot leave a
/// half-deleted state where the device is gone but its copy is still there.
pub fn remove_device(db: &Database, device_id: &str) -> Result<(), AppError> {
    let mut conn = lock_conn!(db.conn);
    let tx = conn
        .transaction()
        .map_err(|e| AppError::Database(e.to_string()))?;
    for table in [
        "lan_usage_events",
        "lan_usage_daily",
        "lan_usage_snapshots",
        "lan_devices",
    ] {
        tx.execute(
            &format!("DELETE FROM {table} WHERE device_id = ?1"),
            params![device_id],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
    }
    tx.commit().map_err(|e| AppError::Database(e.to_string()))?;
    Ok(())
}

/// Enable or disable one device (a disabled device is no longer pulled from, but the history it already brought in is kept).
pub fn set_device_enabled(db: &Database, device_id: &str, enabled: bool) -> Result<(), AppError> {
    let conn = lock_conn!(db.conn);
    conn.execute(
        "UPDATE lan_devices SET enabled = ?2 WHERE device_id = ?1",
        params![device_id, if enabled { 1 } else { 0 }],
    )
    .map_err(|e| AppError::Database(e.to_string()))?;
    Ok(())
}

/// Record the result of a sync (success or failure).
pub fn record_sync_result(
    db: &Database,
    device_id: &str,
    _imported: i64,
    error: Option<&str>,
) -> Result<(), AppError> {
    let now = chrono::Utc::now().timestamp();
    // The sync time and the error are bookkeeping fields for display only and do not affect any
    // statistics result: they do not advance the write epoch, otherwise every sync round would
    // blow away all aggregation caches and make the peer think the local content changed.
    db.write_quietly(|conn| {
        conn.execute(
            "UPDATE lan_devices SET last_sync_at = CASE WHEN ?3 IS NULL THEN ?2 ELSE last_sync_at END, last_error = ?3, last_seen_at = CASE WHEN ?3 IS NULL THEN ?2 ELSE last_seen_at END WHERE device_id = ?1",
            params![device_id, now, error],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    })
}

/// Store one export of a peer's usage.
///
/// Overwrite rules (same device, same day):
/// - once the peer has rolled a day up into its daily rollup table there are no detail rows
///   for that day any more;
/// - when a daily rollup row arrives, the locally stored detail rows of that device and day
///   are deleted first;
/// - conversely, when detail rows arrive and the local database already holds the daily
///   rollup row of that day, the peer has exported its history back as detail rows; the
///   rollup row wins then (it is the peer's own computation).
///
/// This way a device and a day always have exactly one granularity, so summing the rows
/// during aggregation never counts twice.
pub fn store_export(db: &Database, export: &UsageExport) -> Result<usize, AppError> {
    store_export_inner(db, export, None)
}

pub fn store_authorized_export(
    db: &Database,
    export: &UsageExport,
    token: &str,
) -> Result<usize, AppError> {
    store_export_inner(db, export, Some(token))
}

fn store_export_inner(
    db: &Database,
    export: &UsageExport,
    expected_token: Option<&str>,
) -> Result<usize, AppError> {
    let mut sanitized = export.clone();
    // started_at only marks when the peer process started and is unrelated to the usage content;
    // older peers filled in the current time on every export. Without normalizing it the
    // "snapshot is exactly the same as last time" comparison never holds and every round
    // would delete and rewrite the whole thing.
    sanitized.device.started_at = 0;
    if let Some(apps) = &export.shared_app_types {
        let apps = super::normalize_shared_app_types(apps);
        sanitized.events.retain(|row| apps.contains(&row.app_type));
        sanitized.daily.retain(|row| apps.contains(&row.app_type));
        sanitized.shared_app_types = Some(apps);
    }
    let payload =
        serde_json::to_string(&sanitized).map_err(|e| AppError::Message(e.to_string()))?;
    // Legacy status panels have fewer dimensions. Collapse their copy only;
    // the canonical snapshot above retains provider and pricing dimensions.
    let mut daily = std::collections::BTreeMap::new();
    for row in std::mem::take(&mut sanitized.daily) {
        let key = (row.date.clone(), row.app_type.clone(), row.model.clone());
        daily
            .entry(key)
            .and_modify(|total: &mut DailyRollupRow| {
                let requests = total.request_count.saturating_add(row.request_count);
                total.avg_latency_ms = if requests > 0 {
                    (total
                        .avg_latency_ms
                        .saturating_mul(total.request_count)
                        .saturating_add(row.avg_latency_ms.saturating_mul(row.request_count)))
                        / requests
                } else {
                    0
                };
                total.request_count = requests;
                total.success_count = total.success_count.saturating_add(row.success_count);
                total.input_tokens = total.input_tokens.saturating_add(row.input_tokens);
                total.output_tokens = total.output_tokens.saturating_add(row.output_tokens);
                total.cache_read_tokens = total
                    .cache_read_tokens
                    .saturating_add(row.cache_read_tokens);
                total.cache_creation_tokens = total
                    .cache_creation_tokens
                    .saturating_add(row.cache_creation_tokens);
                total.total_cost_usd = (total.total_cost_usd.parse::<f64>().unwrap_or(0.0)
                    + row.total_cost_usd.parse::<f64>().unwrap_or(0.0))
                .to_string();
            })
            .or_insert(row);
    }
    sanitized.daily = daily.into_values().collect();
    let export = &sanitized;
    let device_id = export.device.device_id.as_str();
    // Return early when the snapshot is exactly the same as last time: the event and rollup
    // tables were already aligned with it in the previous commit (the snapshot and the two tables
    // are always written in one transaction, and remove_device deletes all three together), which
    // skips the whole delete and rewrite round. The peer pulls every 300 seconds and in the vast
    // majority of rounds the content does not change.
    {
        let conn = lock_conn!(db.conn);
        let unchanged: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM lan_usage_snapshots WHERE device_id = ?1 AND payload = ?2)",
                params![device_id, payload],
                |r| r.get(0),
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        drop(conn);
        if unchanged {
            return Ok(0);
        }
    }
    let mut conn = lock_conn!(db.conn);
    let tx = conn
        .transaction()
        .map_err(|e| AppError::Database(e.to_string()))?;
    if let Some(token) = expected_token {
        let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM lan_devices d JOIN settings s ON s.key = 'lan_outgoing_token_v1:' || d.device_id WHERE d.device_id = ?1 AND d.enabled = 1 AND s.value = ?2)", params![device_id, token], |r| r.get(0))?;
        if !valid {
            return Err(AppError::Message("设备已停用或配对已解除".into()));
        }
    }
    tx.execute(
        "INSERT INTO lan_usage_snapshots (device_id, payload) VALUES (?1, ?2)
        ON CONFLICT(device_id) DO UPDATE SET payload = excluded.payload",
        params![device_id, payload],
    )
    .map_err(|e| AppError::Database(e.to_string()))?;
    if export.snapshot_version >= 2 {
        tx.execute(
            "DELETE FROM lan_usage_events WHERE device_id = ?1",
            [device_id],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        tx.execute(
            "DELETE FROM lan_usage_daily WHERE device_id = ?1",
            [device_id],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
    }
    let now = chrono::Utc::now().timestamp();
    let mut written = 0usize;

    // A source deselected by the peer must disappear from both imported tables.
    // Older peers omit the scope, so their existing history remains compatible.
    let selected = export
        .shared_app_types
        .as_ref()
        .map(|apps| super::normalize_shared_app_types(apps));
    if let Some(apps) = &selected {
        let names: Vec<&str> = apps.iter().map(String::as_str).collect();
        let scope = super::sql_in_list(&names);
        for table in ["lan_usage_events", "lan_usage_daily"] {
            let sql = if apps.is_empty() {
                format!("DELETE FROM {table} WHERE device_id = ?1")
            } else {
                format!("DELETE FROM {table} WHERE device_id = ?1 AND app_type NOT IN ({scope})")
            };
            tx.execute(&sql, params![device_id])
                .map_err(|e| AppError::Database(e.to_string()))?;
        }
    }

    // The daily rollup row
    let mut insert_daily = tx
        .prepare_cached(
            "INSERT INTO lan_usage_daily (
                device_id, date, app_type, model, request_count, success_count,
                input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                input_token_semantics, total_cost_usd, avg_latency_ms, synced_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
             ON CONFLICT(device_id, date, app_type, model) DO UPDATE SET
                request_count = excluded.request_count,
                success_count = excluded.success_count,
                input_tokens = excluded.input_tokens,
                output_tokens = excluded.output_tokens,
                cache_read_tokens = excluded.cache_read_tokens,
                cache_creation_tokens = excluded.cache_creation_tokens,
                input_token_semantics = excluded.input_token_semantics,
                total_cost_usd = excluded.total_cost_usd,
                avg_latency_ms = excluded.avg_latency_ms,
                synced_at = excluded.synced_at",
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
    for row in &export.daily {
        if selected
            .as_ref()
            .is_some_and(|apps| !apps.contains(&row.app_type))
        {
            continue;
        }
        // The detail rows of that device and day are void (the peer has already rolled them up into
        // its rollup table).
        // A v2 snapshot just cleared the whole event table, so this deletion only matters for the v1
        // merge path.
        if export.snapshot_version < 2 {
            tx.execute(
                "DELETE FROM lan_usage_events
                 WHERE device_id = ?1 AND date(created_at, 'unixepoch', 'localtime') = ?2 AND app_type = ?3",
                params![device_id, row.date, row.app_type],
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        }
        insert_daily
            .execute(params![
                device_id,
                row.date,
                row.app_type,
                row.model,
                row.request_count,
                row.success_count,
                row.input_tokens,
                row.output_tokens,
                row.cache_read_tokens,
                row.cache_creation_tokens,
                row.input_token_semantics,
                row.total_cost_usd,
                row.avg_latency_ms,
                now
            ])
            .map_err(|e| AppError::Database(e.to_string()))?;
        written += 1;
    }
    drop(insert_daily);

    // Fetch the already rolled up (date, app_type) pairs in one go, replacing the per-event EXISTS queries.
    let daily_keys: std::collections::HashSet<(String, String)> = {
        let mut stmt = tx
            .prepare_cached(
                "SELECT DISTINCT date, app_type FROM lan_usage_daily WHERE device_id = ?1",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![device_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| AppError::Database(e.to_string()))?;
        rows.collect::<Result<std::collections::HashSet<_>, _>>()
            .map_err(|e| AppError::Database(e.to_string()))?
    };

    // The detail rows
    let mut insert_event = tx
        .prepare_cached(
            "INSERT INTO lan_usage_events (
                device_id, request_id, data_source, app_type, model, session_id,
                input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                input_token_semantics, input_cost_usd, output_cost_usd, cache_read_cost_usd,
                cache_creation_cost_usd, total_cost_usd, status_code, latency_ms,
                created_at, synced_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)
             ON CONFLICT(device_id, request_id) DO UPDATE SET
                model = excluded.model,
                input_tokens = excluded.input_tokens,
                output_tokens = excluded.output_tokens,
                cache_read_tokens = excluded.cache_read_tokens,
                cache_creation_tokens = excluded.cache_creation_tokens,
                input_token_semantics = excluded.input_token_semantics,
                total_cost_usd = excluded.total_cost_usd,
                status_code = excluded.status_code,
                latency_ms = excluded.latency_ms,
                synced_at = excluded.synced_at",
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
    for row in &export.events {
        if selected
            .as_ref()
            .is_some_and(|apps| !apps.contains(&row.app_type))
        {
            continue;
        }
        let date = super::local_date_string(row.created_at);
        // The peer has already rolled that day up into its rollup table (when both granularities arrive the rollup wins)
        if daily_keys.contains(&(date, row.app_type.clone())) {
            continue;
        }
        insert_event
            .execute(params![
                device_id,
                row.request_id,
                row.data_source,
                row.app_type,
                row.model,
                row.session_id,
                row.input_tokens,
                row.output_tokens,
                row.cache_read_tokens,
                row.cache_creation_tokens,
                row.input_token_semantics,
                row.input_cost_usd,
                row.output_cost_usd,
                row.cache_read_cost_usd,
                row.cache_creation_cost_usd,
                row.total_cost_usd,
                row.status_code,
                row.latency_ms,
                row.created_at,
                now
            ])
            .map_err(|e| AppError::Database(e.to_string()))?;
        written += 1;
    }
    drop(insert_event);

    tx.commit()
        .map_err(|e| AppError::Database(format!("写入局域网用量失败: {e}")))?;
    Ok(written)
}

/// The local sharing export: filters the detail records and the historical rollups by the
/// selected apps and only reads the statistics columns.
///
/// The detail records are the rows of the selected apps in `proxy_request_logs`; history that
/// was already rolled up into the per-day table is taken from `usage_daily_rollups`. Detail
/// records that arrive late after the rollup are kept, so it matches the local query exactly.
pub fn export_local_usage(db: &Database) -> Result<UsageExport, AppError> {
    let settings = super::identity::share_settings(db);
    let device = super::identity::device_info(db)?;
    let selected = super::normalize_shared_app_types(&settings.shared_app_types);
    if selected.is_empty() {
        return Ok(UsageExport {
            snapshot_version: 2,
            device,
            shared_app_types: Some(selected),
            events: Vec::new(),
            daily: Vec::new(),
        });
    }
    let selected_refs: Vec<&str> = selected.iter().map(String::as_str).collect();
    let app_types = super::sql_in_list(&selected_refs);
    // The full export is the heaviest read-only query (the peer triggers it on every sync): it
    // runs in a read snapshot on the read-only connection, keeps the detail records and the daily
    // rollups consistent and does not hold the write connection, so scans and UI actions do not
    // have to queue.
    let (events, daily) = db.with_read_connection(|conn| -> Result<_, AppError> {
    let mut events = Vec::new();
    let provider_name = crate::services::usage_stats::provider_name_coalesce("l", "p");
    let effective = crate::services::usage_stats::effective_usage_log_filter("l");
    {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT l.request_id, l.data_source, l.app_type, l.model, l.session_id,
                        l.input_tokens, l.output_tokens, l.cache_read_tokens, l.cache_creation_tokens,
                        l.input_token_semantics, l.input_cost_usd, l.output_cost_usd,
                        l.cache_read_cost_usd, l.cache_creation_cost_usd, l.total_cost_usd,
                        l.status_code, l.latency_ms, l.created_at,
                        l.provider_id, {provider_name}, l.request_model, l.pricing_model,
                        l.first_token_ms, l.duration_ms, l.is_streaming, l.cost_multiplier
                 FROM proxy_request_logs l LEFT JOIN providers p ON l.provider_id = p.id AND l.app_type = p.app_type
                 WHERE l.app_type IN ({app_types}) AND {effective}
"
            ))
            .map_err(|e| AppError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(UsageEventRow {
                    metadata: UsageMetadata {
                        provider_id: row.get(18)?,
                        provider_name: row.get(19)?,
                        request_model: row.get(20)?,
                        pricing_model: row.get(21)?,
                        first_token_ms: row.get(22)?,
                        duration_ms: row.get(23)?,
                        is_streaming: row.get(24)?,
                        cost_multiplier: row.get(25)?,
                    },
                    request_id: row.get(0)?,
                    data_source: row.get(1)?,
                    app_type: row.get(2)?,
                    model: row.get(3)?,
                    session_id: row.get(4)?,
                    input_tokens: row.get(5)?,
                    output_tokens: row.get(6)?,
                    cache_read_tokens: row.get(7)?,
                    cache_creation_tokens: row.get(8)?,
                    input_token_semantics: row.get(9)?,
                    input_cost_usd: row.get(10)?,
                    output_cost_usd: row.get(11)?,
                    cache_read_cost_usd: row.get(12)?,
                    cache_creation_cost_usd: row.get(13)?,
                    total_cost_usd: row.get(14)?,
                    status_code: row.get(15)?,
                    latency_ms: row.get(16)?,
                    created_at: row.get(17)?,
                })
            })
            .map_err(|e| AppError::Database(e.to_string()))?;
        for row in rows {
            events.push(row.map_err(|e| AppError::Database(e.to_string()))?);
        }
    }

    let mut daily = Vec::new();
    let fresh_input = fresh_input_sql("r");
    let provider_name = crate::services::usage_stats::provider_name_coalesce("r", "p");
    {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT r.date, r.app_type, r.model, SUM(r.request_count), SUM(r.success_count),
                        SUM({fresh_input}), SUM(r.output_tokens), SUM(r.cache_read_tokens), SUM(r.cache_creation_tokens),
                        2, CAST(SUM(CAST(r.total_cost_usd AS REAL)) AS TEXT),
                        CAST(COALESCE(SUM(r.avg_latency_ms * r.request_count) * 1.0 / NULLIF(SUM(r.request_count), 0), 0) AS INTEGER),
                        r.provider_id, {provider_name}, r.request_model, r.pricing_model
                 FROM usage_daily_rollups r LEFT JOIN providers p ON r.provider_id = p.id AND r.app_type = p.app_type
                 WHERE r.app_type IN ({app_types})
                 GROUP BY r.date, r.app_type, r.model, r.provider_id, r.request_model, r.pricing_model"
            ))
            .map_err(|e| AppError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(DailyRollupRow {
                    metadata: UsageMetadata {
                        provider_id: row.get(12)?,
                        provider_name: row.get(13)?,
                        request_model: row.get(14)?,
                        pricing_model: row.get(15)?,
                        ..Default::default()
                    },
                    date: row.get(0)?,
                    app_type: row.get(1)?,
                    model: row.get(2)?,
                    request_count: row.get(3)?,
                    success_count: row.get(4)?,
                    input_tokens: row.get(5)?,
                    output_tokens: row.get(6)?,
                    cache_read_tokens: row.get(7)?,
                    cache_creation_tokens: row.get(8)?,
                    input_token_semantics: row.get(9)?,
                    total_cost_usd: row.get(10)?,
                    avg_latency_ms: row.get(11)?,
                })
            })
            .map_err(|e| AppError::Database(e.to_string()))?;
        for row in rows {
            daily.push(row.map_err(|e| AppError::Database(e.to_string()))?);
        }
    }
    Ok((events, daily))
    })?;

    Ok(UsageExport {
        snapshot_version: 2,
        device,
        shared_app_types: Some(selected),
        events,
        daily,
    })
}
