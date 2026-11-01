//! 局域网用量的本地存储：登记设备，保存与导出共享快照。

use super::{DailyRollupRow, LanDeviceStatus, UsageEventRow, UsageExport, UsageMetadata};
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::services::sql_helpers::fresh_input_sql;
use rusqlite::{params, OptionalExtension};

/// 登记/更新一台对端设备。
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

/// 列出已登记的设备（含同步状态）。
pub fn list_devices(db: &Database) -> Result<Vec<LanDeviceStatus>, AppError> {
    let conn = lock_conn!(db.conn);
    let mut stmt = conn
        .prepare(
            "SELECT device_id, device_name, host, port, enabled, last_seen_at, last_sync_at, last_error,
                    (SELECT COUNT(*) FROM lan_usage_events e WHERE e.device_id = lan_devices.device_id)
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
                paired: false,
                last_seen_at: row.get(5)?,
                last_sync_at: row.get(6)?,
                last_error: row.get(7)?,
                last_imported: row.get(8)?,
            })
        })
        .map_err(|e| AppError::Database(e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| AppError::Database(e.to_string()))?);
    }
    drop(stmt);
    drop(conn);
    for device in &mut out {
        device.paired = db.get_setting(&format!("lan_outgoing_token_v1:{}", device.device_id))?.is_some();
    }
    Ok(out)
}

/// 取出某台设备的地址（host, port）。
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

/// 移除一台设备，并清掉它拉回来的用量行。
pub fn remove_device(db: &Database, device_id: &str) -> Result<(), AppError> {
    let conn = lock_conn!(db.conn);
    conn.execute(
        "DELETE FROM lan_usage_events WHERE device_id = ?1",
        params![device_id],
    )
    .map_err(|e| AppError::Database(e.to_string()))?;
    conn.execute(
        "DELETE FROM lan_usage_daily WHERE device_id = ?1",
        params![device_id],
    )
    .map_err(|e| AppError::Database(e.to_string()))?;
    conn.execute(
        "DELETE FROM lan_usage_snapshots WHERE device_id = ?1",
        params![device_id],
    )
    .map_err(|e| AppError::Database(e.to_string()))?;
    conn.execute(
        "DELETE FROM lan_devices WHERE device_id = ?1",
        params![device_id],
    )
    .map_err(|e| AppError::Database(e.to_string()))?;
    Ok(())
}

/// 启用/停用一台设备（停用后不再拉取，但保留已拉到的历史）。
pub fn set_device_enabled(db: &Database, device_id: &str, enabled: bool) -> Result<(), AppError> {
    let conn = lock_conn!(db.conn);
    conn.execute(
        "UPDATE lan_devices SET enabled = ?2 WHERE device_id = ?1",
        params![device_id, if enabled { 1 } else { 0 }],
    )
    .map_err(|e| AppError::Database(e.to_string()))?;
    Ok(())
}

/// 记录一次同步的结果（成功或失败）。
pub fn record_sync_result(
    db: &Database,
    device_id: &str,
    _imported: i64,
    error: Option<&str>,
) -> Result<(), AppError> {
    let conn = lock_conn!(db.conn);
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE lan_devices SET last_sync_at = CASE WHEN ?3 IS NULL THEN ?2 ELSE last_sync_at END, last_error = ?3, last_seen_at = CASE WHEN ?3 IS NULL THEN ?2 ELSE last_seen_at END WHERE device_id = ?1",
        params![device_id, now, error],
    )
    .map_err(|e| AppError::Database(e.to_string()))?;
    Ok(())
}

/// 落库一份对端导出的用量。
///
/// 覆盖规则（同一台设备、同一天）：
/// - 对端把某天收进按天汇总表后，就不再有那天的明细行；
/// - 收到按天汇总行时，先删掉本地存着的该设备该天的明细行；
/// - 反过来收到明细行时，如果本地已经有那天的按天汇总行，说明对端把历史重新
///   导回成了明细，此时以汇总行为准（它是对端自己算出来的口径）。
///
/// 这样同一台设备的同一天永远只有一种粒度，聚合时直接相加不会重复计数。
pub fn store_export(db: &Database, export: &UsageExport) -> Result<usize, AppError> {
    store_export_inner(db, export, None)
}

pub fn store_authorized_export(db: &Database, export: &UsageExport, token: &str) -> Result<usize, AppError> {
    store_export_inner(db, export, Some(token))
}

fn store_export_inner(db: &Database, export: &UsageExport, expected_token: Option<&str>) -> Result<usize, AppError> {
    let mut sanitized = export.clone();
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
    let mut conn = lock_conn!(db.conn);
    let tx = conn
        .transaction()
        .map_err(|e| AppError::Database(e.to_string()))?;
    if let Some(token) = expected_token {
        let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM lan_devices d JOIN settings s ON s.key = 'lan_outgoing_token_v1:' || d.device_id WHERE d.device_id = ?1 AND d.enabled = 1 AND s.value = ?2)", params![device_id, token], |r| r.get(0))?;
        if !valid { return Err(AppError::Message("设备已停用或配对已解除".into())); }
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

    // 按天汇总行
    for row in &export.daily {
        if selected
            .as_ref()
            .is_some_and(|apps| !apps.contains(&row.app_type))
        {
            continue;
        }
        // 该设备该天的明细行作废（对端已经把它收进汇总表了）
        tx.execute(
            "DELETE FROM lan_usage_events
             WHERE device_id = ?1 AND date(created_at, 'unixepoch', 'localtime') = ?2 AND app_type = ?3",
            params![device_id, row.date, row.app_type],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        tx.execute(
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
            params![
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
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        written += 1;
    }

    // 明细行
    for row in &export.events {
        if selected
            .as_ref()
            .is_some_and(|apps| !apps.contains(&row.app_type))
        {
            continue;
        }
        let date = super::local_date_string(row.created_at);
        // 对端已经把那天收进汇总表了（同时收到两种粒度时以汇总为准）
        let has_daily: bool = tx
            .query_row(
                "SELECT 1 FROM lan_usage_daily WHERE device_id = ?1 AND date = ?2 AND app_type = ?3 LIMIT 1",
                params![device_id, date, row.app_type],
                |_| Ok(true),
            )
            .optional()
            .map_err(|e| AppError::Database(e.to_string()))?
            .unwrap_or(false);
        if has_daily {
            continue;
        }
        tx.execute(
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
            params![
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
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        written += 1;
    }

    tx.commit()
        .map_err(|e| AppError::Database(format!("写入局域网用量失败: {e}")))?;
    Ok(written)
}

/// 本机的共享导出：按勾选应用过滤明细与历史汇总，只读取统计字段。
///
/// 明细取 `proxy_request_logs` 中勾选应用的行；已经被收进按天汇总表的历史
/// 从 `usage_daily_rollups` 取。保留汇总后迟到的新明细，与本机原查询完全一致。
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
    let conn = lock_conn!(db.conn);

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

    Ok(UsageExport {
        snapshot_version: 2,
        device,
        shared_app_types: Some(selected),
        events,
        daily,
    })
}
