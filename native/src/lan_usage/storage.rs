//! 局域网用量的本地存储：登记设备、存对端拉回来的用量行、按设备聚合。

use super::{
    DailyRollupRow, LanDailyTotal, LanDeviceStatus, LanDeviceUsage, LanUsageSummary, UsageEventRow,
    UsageExport, UsageMetadata,
};
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
        "UPDATE lan_devices SET last_sync_at = ?2, last_error = ?3, last_seen_at = ?2 WHERE device_id = ?1",
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

/// 把时间区间换算成按天汇总表用的本地日期区间。
fn date_bounds(start: Option<i64>, end: Option<i64>) -> (Option<String>, Option<String>) {
    let start = start.map(super::local_date_string);
    let end = end.map(super::local_date_string);
    (start, end)
}

/// 把汇总查询的一行读成 `LanUsageSummary`。
///
/// 列顺序固定为：请求数、成本、输入、输出、缓存写入、缓存读取、成功数。
fn summary_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LanUsageSummary> {
    let requests: i64 = row.get(0)?;
    let cost: f64 = row.get(1)?;
    let input: i64 = row.get(2)?;
    let output: i64 = row.get(3)?;
    let cache_creation: i64 = row.get(4)?;
    let cache_read: i64 = row.get(5)?;
    let success: i64 = row.get(6)?;
    let mut summary = LanUsageSummary {
        total_requests: requests.max(0) as u64,
        total_cost: format!("{cost}"),
        total_input_tokens: input.max(0) as u64,
        total_output_tokens: output.max(0) as u64,
        total_cache_creation_tokens: cache_creation.max(0) as u64,
        total_cache_read_tokens: cache_read.max(0) as u64,
        success_count: success.max(0) as u64,
        ..Default::default()
    };
    summary.recompute_rates();
    Ok(summary)
}

/// 远端设备在给定区间内的用量合计。
pub fn remote_summary(
    db: &Database,
    start: Option<i64>,
    end: Option<i64>,
) -> Result<LanUsageSummary, AppError> {
    let conn = lock_conn!(db.conn);
    let (start_date, end_date) = date_bounds(start, end);
    let mut event_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut event_where: Vec<String> = Vec::new();
    if let Some(s) = start {
        event_where.push("e.created_at >= ?".to_string());
        event_params.push(Box::new(s));
    }
    if let Some(e) = end {
        event_where.push("e.created_at <= ?".to_string());
        event_params.push(Box::new(e));
    }

    let mut daily_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut daily_where: Vec<String> = Vec::new();
    if let Some(s) = &start_date {
        daily_where.push("d.date >= ?".to_string());
        daily_params.push(Box::new(s.clone()));
    }
    if let Some(e) = &end_date {
        daily_where.push("d.date <= ?".to_string());
        daily_params.push(Box::new(e.clone()));
    }

    let (sql, params) =
        build_aggregate_query(&event_where, &daily_where, event_params, daily_params);
    let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let row = conn.query_row(&sql, refs.as_slice(), summary_from_row);
    match row {
        Ok(v) => Ok(v),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(LanUsageSummary::default()),
        Err(e) => Err(AppError::Database(e.to_string())),
    }
}

/// 每台远端设备在给定区间内的用量合计。
pub fn per_device_summaries(
    db: &Database,
    start: Option<i64>,
    end: Option<i64>,
) -> Result<Vec<LanDeviceUsage>, AppError> {
    let devices = list_devices(db)?;
    let mut out = Vec::new();
    for device in devices {
        let summary = device_summary(db, &device.device_id, start, end)?;
        out.push(LanDeviceUsage {
            device_id: device.device_id.clone(),
            device_name: device.device_name.clone(),
            is_local: false,
            summary,
        });
    }
    Ok(out)
}

/// 单台设备的用量合计。
pub fn device_summary(
    db: &Database,
    device_id: &str,
    start: Option<i64>,
    end: Option<i64>,
) -> Result<LanUsageSummary, AppError> {
    let conn = lock_conn!(db.conn);
    let (start_date, end_date) = date_bounds(start, end);

    let mut event_params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(device_id.to_string())];
    let mut event_where: Vec<String> = vec!["e.device_id = ?".to_string()];
    if let Some(s) = start {
        event_where.push("e.created_at >= ?".to_string());
        event_params.push(Box::new(s));
    }
    if let Some(e) = end {
        event_where.push("e.created_at <= ?".to_string());
        event_params.push(Box::new(e));
    }

    let mut daily_params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(device_id.to_string())];
    let mut daily_where: Vec<String> = vec!["d.device_id = ?".to_string()];
    if let Some(s) = &start_date {
        daily_where.push("d.date >= ?".to_string());
        daily_params.push(Box::new(s.clone()));
    }
    if let Some(e) = &end_date {
        daily_where.push("d.date <= ?".to_string());
        daily_params.push(Box::new(e.clone()));
    }

    let (sql, params) =
        build_aggregate_query(&event_where, &daily_where, event_params, daily_params);
    let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let row = conn.query_row(&sql, refs.as_slice(), summary_from_row);
    match row {
        Ok(v) => Ok(v),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(LanUsageSummary::default()),
        Err(e) => Err(AppError::Database(e.to_string())),
    }
}

/// 生成「明细 + 按天汇总」两路合并的聚合 SQL。
///
/// 用 CTE 而不是把 WHERE 抄七遍：参数只要各出现一次，顺序也不会错。
/// 明细和按天汇总对同一台设备的同一天互斥（存储层保证），所以两路直接相加。
fn build_aggregate_query(
    event_where: &[String],
    daily_where: &[String],
    event_params: Vec<Box<dyn rusqlite::ToSql>>,
    daily_params: Vec<Box<dyn rusqlite::ToSql>>,
) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
    let event_clause = if event_where.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", event_where.join(" AND "))
    };
    let daily_clause = if daily_where.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", daily_where.join(" AND "))
    };
    let fresh_event = fresh_input_sql("e");
    let fresh_daily = fresh_input_sql("d");

    let sql = format!(
        "WITH ev AS (
            SELECT COUNT(*) AS request_count,
                   COALESCE(SUM(CAST(e.total_cost_usd AS REAL)), 0) AS total_cost,
                   COALESCE(SUM({fresh_event}), 0) AS input_tokens,
                   COALESCE(SUM(e.output_tokens), 0) AS output_tokens,
                   COALESCE(SUM(e.cache_creation_tokens), 0) AS cache_creation_tokens,
                   COALESCE(SUM(e.cache_read_tokens), 0) AS cache_read_tokens,
                   COALESCE(SUM(CASE WHEN e.status_code >= 200 AND e.status_code < 300 THEN 1 ELSE 0 END), 0) AS success_count
            FROM lan_usage_events e {event_clause}
         ),
         dl AS (
            SELECT COALESCE(SUM(d.request_count), 0) AS request_count,
                   COALESCE(SUM(CAST(d.total_cost_usd AS REAL)), 0) AS total_cost,
                   COALESCE(SUM({fresh_daily}), 0) AS input_tokens,
                   COALESCE(SUM(d.output_tokens), 0) AS output_tokens,
                   COALESCE(SUM(d.cache_creation_tokens), 0) AS cache_creation_tokens,
                   COALESCE(SUM(d.cache_read_tokens), 0) AS cache_read_tokens,
                   COALESCE(SUM(d.success_count), 0) AS success_count
            FROM lan_usage_daily d {daily_clause}
         )
         SELECT ev.request_count + dl.request_count,
                ev.total_cost + dl.total_cost,
                ev.input_tokens + dl.input_tokens,
                ev.output_tokens + dl.output_tokens,
                ev.cache_creation_tokens + dl.cache_creation_tokens,
                ev.cache_read_tokens + dl.cache_read_tokens,
                ev.success_count + dl.success_count
         FROM ev, dl"
    );

    let mut params = event_params;
    params.extend(daily_params);
    (sql, params)
}

/// 按天的跨设备合计。
pub fn daily_totals(
    db: &Database,
    start: Option<i64>,
    end: Option<i64>,
) -> Result<Vec<LanDailyTotal>, AppError> {
    let conn = lock_conn!(db.conn);
    let (start_date, end_date) = date_bounds(start, end);
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut where_parts = Vec::new();
    if let Some(s) = &start_date {
        where_parts.push("date >= ?".to_string());
        params.push(Box::new(s.clone()));
    }
    if let Some(e) = &end_date {
        where_parts.push("date <= ?".to_string());
        params.push(Box::new(e.clone()));
    }
    let where_clause = if where_parts.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_parts.join(" AND "))
    };
    let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn
        .prepare(&format!(
            "SELECT date, SUM(request_count), SUM(CAST(total_cost_usd AS REAL)),
                    SUM(input_tokens), SUM(output_tokens), SUM(cache_creation_tokens),
                    SUM(cache_read_tokens), SUM(success_count)
             FROM lan_usage_daily {where_clause}
             GROUP BY date ORDER BY date"
        ))
        .map_err(|e| AppError::Database(e.to_string()))?;
    let rows = stmt
        .query_map(refs.as_slice(), |r| {
            let date: String = r.get(0)?;
            let requests: i64 = r.get(1)?;
            let cost: f64 = r.get(2)?;
            let input: i64 = r.get(3)?;
            let output: i64 = r.get(4)?;
            let cache_creation: i64 = r.get(5)?;
            let cache_read: i64 = r.get(6)?;
            let success: i64 = r.get(7)?;
            let mut summary = LanUsageSummary {
                total_requests: requests.max(0) as u64,
                total_cost: format!("{cost}"),
                total_input_tokens: input.max(0) as u64,
                total_output_tokens: output.max(0) as u64,
                total_cache_creation_tokens: cache_creation.max(0) as u64,
                total_cache_read_tokens: cache_read.max(0) as u64,
                success_count: success.max(0) as u64,
                ..Default::default()
            };
            summary.recompute_rates();
            Ok(LanDailyTotal { date, summary })
        })
        .map_err(|e| AppError::Database(e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| AppError::Database(e.to_string()))?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lan_usage::{DailyRollupRow, DeviceInfo, UsageEventRow};

    fn device(id: &str) -> DeviceInfo {
        DeviceInfo {
            device_id: id.to_string(),
            device_name: format!("device-{id}"),
            app_version: "4.0.19".to_string(),
            platform: "linux".to_string(),
            started_at: 1_700_000_000,
        }
    }

    fn event(id: &str, created_at: i64, input: i64) -> UsageEventRow {
        UsageEventRow {
            metadata: Default::default(),
            request_id: id.to_string(),
            data_source: "dsh_session".to_string(),
            app_type: "dsh".to_string(),
            model: "fixture".to_string(),
            session_id: Some("s1".to_string()),
            input_tokens: input,
            output_tokens: 10,
            cache_read_tokens: 5,
            cache_creation_tokens: 2,
            input_token_semantics: 2,
            input_cost_usd: "1.5".to_string(),
            output_cost_usd: "0.5".to_string(),
            cache_read_cost_usd: "0.25".to_string(),
            cache_creation_cost_usd: "0.125".to_string(),
            total_cost_usd: "2.375".to_string(),
            status_code: 200,
            latency_ms: 100,
            created_at,
        }
    }

    fn daily(date: &str, requests: i64, input: i64) -> DailyRollupRow {
        DailyRollupRow {
            metadata: Default::default(),
            date: date.to_string(),
            app_type: "dsh".to_string(),
            model: "fixture".to_string(),
            request_count: requests,
            success_count: requests,
            input_tokens: input,
            output_tokens: 10,
            cache_read_tokens: 5,
            cache_creation_tokens: 2,
            input_token_semantics: 2,
            total_cost_usd: "3".to_string(),
            avg_latency_ms: 100,
        }
    }

    /// 构造某个本地日期 12:00 的 unix 秒（按本机时区算，避免写死时刻）
    fn ts_for(date: &str) -> i64 {
        use chrono::TimeZone;
        let naive = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        chrono::Local
            .from_local_datetime(&naive)
            .single()
            .unwrap()
            .timestamp()
    }

    #[test]
    fn stores_events_and_summarizes_per_device() {
        let db = Database::memory().unwrap();
        register_device(&db, "dev-a", "A", "203.0.113.2", 17890).unwrap();
        let ts = ts_for("2024-05-01");
        let export = UsageExport {
            snapshot_version: 0,
            device: device("dev-a"),
            shared_app_types: None,
            events: vec![event("r1", ts, 100), event("r2", ts + 60, 200)],
            daily: vec![],
        };
        assert_eq!(store_export(&db, &export).unwrap(), 2);

        let summary = device_summary(&db, "dev-a", None, None).unwrap();
        assert_eq!(summary.total_requests, 2);
        assert_eq!(summary.total_input_tokens, 300);
        assert_eq!(summary.success_count, 2);
        assert!((summary.total_cost.parse::<f64>().unwrap() - 4.75).abs() < 1e-9);
    }

    /// 对端把某天收进按天汇总表后，本地存的明细行必须作废，
    /// 否则同一天会被算两遍。
    #[test]
    fn daily_rollup_replaces_detail_rows_for_the_same_day() {
        let db = Database::memory().unwrap();
        register_device(&db, "dev-a", "A", "203.0.113.2", 17890).unwrap();
        let ts = ts_for("2024-05-01");

        let detail = UsageExport {
            snapshot_version: 0,
            device: device("dev-a"),
            shared_app_types: None,
            events: vec![event("r1", ts, 100), event("r2", ts + 60, 200)],
            daily: vec![],
        };
        store_export(&db, &detail).unwrap();
        assert_eq!(
            device_summary(&db, "dev-a", None, None)
                .unwrap()
                .total_requests,
            2
        );

        let rolled = UsageExport {
            snapshot_version: 0,
            device: device("dev-a"),
            shared_app_types: None,
            events: vec![],
            daily: vec![daily("2024-05-01", 7, 999)],
        };
        store_export(&db, &rolled).unwrap();

        let summary = device_summary(&db, "dev-a", None, None).unwrap();
        // 汇总行接管了那一天：请求数是 7 而不是 2+7
        assert_eq!(summary.total_requests, 7);
        assert_eq!(summary.total_input_tokens, 999);
    }

    /// 反向：对端把历史重新导回成明细时，已有的按天汇总行为准。
    #[test]
    fn detail_rows_are_ignored_when_the_day_already_has_a_rollup() {
        let db = Database::memory().unwrap();
        register_device(&db, "dev-a", "A", "203.0.113.2", 17890).unwrap();
        let ts = ts_for("2024-05-01");

        let rolled = UsageExport {
            snapshot_version: 0,
            device: device("dev-a"),
            shared_app_types: None,
            events: vec![],
            daily: vec![daily("2024-05-01", 7, 999)],
        };
        store_export(&db, &rolled).unwrap();

        let detail = UsageExport {
            snapshot_version: 0,
            device: device("dev-a"),
            shared_app_types: None,
            events: vec![event("r1", ts, 100)],
            daily: vec![],
        };
        store_export(&db, &detail).unwrap();

        let summary = device_summary(&db, "dev-a", None, None).unwrap();
        assert_eq!(summary.total_requests, 7);
        assert_eq!(summary.total_input_tokens, 999);
    }

    #[test]
    fn remote_summary_adds_up_all_devices() {
        let db = Database::memory().unwrap();
        register_device(&db, "dev-a", "A", "203.0.113.2", 17890).unwrap();
        register_device(&db, "dev-b", "B", "203.0.113.3", 17890).unwrap();
        let ts = ts_for("2024-05-01");
        store_export(
            &db,
            &UsageExport {
                snapshot_version: 0,
                device: device("dev-a"),
                shared_app_types: None,
                events: vec![event("a1", ts, 100)],
                daily: vec![],
            },
        )
        .unwrap();
        store_export(
            &db,
            &UsageExport {
                snapshot_version: 0,
                device: device("dev-b"),
                shared_app_types: None,
                events: vec![event("b1", ts, 300)],
                daily: vec![],
            },
        )
        .unwrap();

        let remote = remote_summary(&db, None, None).unwrap();
        assert_eq!(remote.total_requests, 2);
        assert_eq!(remote.total_input_tokens, 400);

        let devices = per_device_summaries(&db, None, None).unwrap();
        assert_eq!(devices.len(), 2);
    }

    #[test]
    fn disconnecting_a_device_clears_its_rows() {
        let db = Database::memory().unwrap();
        register_device(&db, "dev-a", "A", "203.0.113.2", 17890).unwrap();
        let ts = ts_for("2024-05-01");
        store_export(
            &db,
            &UsageExport {
                snapshot_version: 0,
                device: device("dev-a"),
                shared_app_types: None,
                events: vec![event("a1", ts, 100)],
                daily: vec![],
            },
        )
        .unwrap();
        remove_device(&db, "dev-a").unwrap();
        assert!(list_devices(&db).unwrap().is_empty());
        assert_eq!(remote_summary(&db, None, None).unwrap().total_requests, 0);
    }

    #[test]
    fn daily_totals_group_by_date() {
        let db = Database::memory().unwrap();
        register_device(&db, "dev-a", "A", "203.0.113.2", 17890).unwrap();
        store_export(
            &db,
            &UsageExport {
                snapshot_version: 0,
                device: device("dev-a"),
                shared_app_types: None,
                events: vec![],
                daily: vec![daily("2024-05-01", 3, 30), daily("2024-05-02", 4, 40)],
            },
        )
        .unwrap();
        let totals = daily_totals(&db, None, None).unwrap();
        assert_eq!(totals.len(), 2);
        assert_eq!(totals[0].date, "2024-05-01");
        assert_eq!(totals[0].summary.total_requests, 3);
        assert_eq!(totals[1].summary.total_requests, 4);
    }
    fn seed_local_usage(db: &Database) {
        let conn = db.conn.lock().unwrap();
        for app in super::super::SHARED_APP_TYPES {
            // Include proxy-origin statistics as well as locally imported sessions.
            conn.execute(
                "INSERT INTO proxy_request_logs (request_id, provider_id, app_type, model, data_source, latency_ms, status_code, created_at, input_tokens)
                 VALUES (?1, 'fixture', ?1, 'fixture', 'proxy', 100, 200, ?2, 100)",
                params![app, ts_for("2024-05-02")],
            ).unwrap();
            for provider in ["first", "second"] {
                conn.execute(
                    "INSERT INTO usage_daily_rollups (date, app_type, provider_id, model, request_count, success_count, input_tokens, total_cost_usd)
                     VALUES ('2024-05-01', ?1, ?2, 'fixture', 2, 2, 200, '1.25')",
                    params![app, provider],
                ).unwrap();
            }
        }
    }

    #[test]
    fn export_filters_details_and_rollups_and_merges_provider_dimensions() {
        let db = Database::memory().unwrap();
        seed_local_usage(&db);
        let all = export_local_usage(&db).unwrap();
        assert_eq!(all.events.len(), super::super::SHARED_APP_TYPES.len());
        assert_eq!(all.daily.len(), super::super::SHARED_APP_TYPES.len() * 2);
        let mut settings = super::super::identity::share_settings(&db);
        settings.shared_app_types = vec!["mcode".into()];
        super::super::identity::save_share_settings(&db, &settings).unwrap();
        let selected = export_local_usage(&db).unwrap();
        assert_eq!(selected.shared_app_types, Some(vec!["mcode".into()]));
        assert_eq!(selected.events.len(), 1);
        assert_eq!(selected.events[0].app_type, "mcode");
        assert_eq!(selected.daily.len(), 2);
        assert_eq!(selected.daily[0].app_type, "mcode");
        assert_eq!(
            selected.daily.iter().map(|r| r.request_count).sum::<i64>(),
            4
        );
        assert_eq!(
            selected.daily.iter().map(|r| r.input_tokens).sum::<i64>(),
            400
        );
        assert_eq!(selected.daily[0].input_token_semantics, 2);
        assert_eq!(
            selected
                .daily
                .iter()
                .map(|r| r.total_cost_usd.parse::<f64>().unwrap())
                .sum::<f64>(),
            2.5
        );
        settings.shared_app_types.clear();
        super::super::identity::save_share_settings(&db, &settings).unwrap();
        let none = export_local_usage(&db).unwrap();
        assert_eq!(none.shared_app_types, Some(vec![]));
        assert!(none.events.is_empty());
        assert!(none.daily.is_empty());
        // Selecting none changes exports, never deletes original local records.
        let conn = db.conn.lock().unwrap();
        let local: i64 = conn
            .query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(local as usize, super::super::SHARED_APP_TYPES.len());
    }

    #[test]
    fn source_revocation_clears_only_that_peers_unselected_copies() {
        let db = Database::memory().unwrap();
        register_device(&db, "a", "A", "203.0.113.2", 17890).unwrap();
        register_device(&db, "b", "B", "203.0.113.3", 17890).unwrap();
        let ts = ts_for("2024-05-02");
        let mut zcode = event("zcode", ts, 10);
        zcode.app_type = "zcode".into();
        zcode.data_source = "zcode_session".into();
        let mut export = UsageExport {
            snapshot_version: 0,
            device: device("a"),
            shared_app_types: None,
            events: vec![event("dsh", ts, 20), zcode],
            daily: vec![daily("2024-05-01", 3, 30)],
        };
        store_export(&db, &export).unwrap();
        export.device = device("b");
        store_export(&db, &export).unwrap();
        export.device = device("a");
        export.shared_app_types = Some(vec!["zcode".into()]);
        // Rows outside the declared selection are ignored even if present in the payload.
        store_export(&db, &export).unwrap();
        assert_eq!(
            device_summary(&db, "a", None, None).unwrap().total_requests,
            1
        );
        assert_eq!(
            device_summary(&db, "b", None, None).unwrap().total_requests,
            5
        );
        export.shared_app_types = Some(vec![]);
        store_export(&db, &export).unwrap();
        assert_eq!(
            device_summary(&db, "a", None, None).unwrap().total_requests,
            0
        );
        assert_eq!(
            device_summary(&db, "b", None, None).unwrap().total_requests,
            5
        );
    }

    #[test]
    fn one_apps_rollup_does_not_remove_another_apps_same_day_details() {
        let db = Database::memory().unwrap();
        register_device(&db, "a", "A", "203.0.113.2", 17890).unwrap();
        let ts = ts_for("2024-05-01");
        let mut zcode = event("zcode", ts, 10);
        zcode.app_type = "zcode".into();
        let mut export = UsageExport {
            snapshot_version: 0,
            device: device("a"),
            shared_app_types: None,
            events: vec![event("dsh", ts, 20), zcode.clone()],
            daily: vec![],
        };
        store_export(&db, &export).unwrap();
        export.events.clear();
        export.daily.push(daily("2024-05-01", 3, 30));
        store_export(&db, &export).unwrap();
        assert_eq!(
            device_summary(&db, "a", None, None).unwrap().total_requests,
            4
        );
        // A rollup for DSH must not suppress a new ZCode detail, either.
        zcode.request_id = "zcode-new".into();
        export.events.push(zcode);
        store_export(&db, &export).unwrap();
        assert_eq!(
            device_summary(&db, "a", None, None).unwrap().total_requests,
            5
        );
    }

    #[test]
    fn legacy_export_without_scope_remains_readable() {
        let value = serde_json::json!({"device": device("old"), "events": [], "daily": []});
        let export: UsageExport = serde_json::from_value(value).unwrap();
        assert_eq!(export.shared_app_types, None);
    }
}
