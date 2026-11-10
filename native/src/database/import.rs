//! Read-only import of metrics. Provider settings, keys and messages are never copied.
use super::*;
use rusqlite::{types::Value as SqlValue, OpenFlags};
use serde::Serialize;
use std::path::Path;

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub requests: u64,
    pub rollups: u64,
    pub cursors: u64,
    pub ledger: u64,
    pub prices: u64,
    pub warnings: Vec<String>,
}
fn columns(conn: &Connection, table: &str) -> Result<Vec<String>, AppError> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info(\"{table}\")"))?;
    let rows = statement.query_map([], |row| row.get(1))?;
    Ok(rows.collect::<Result<_, _>>()?)
}
fn has_table(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
        [table],
        |r| r.get::<_, i32>(0),
    )
    .optional()
    .ok()
    .flatten()
    .is_some()
}
fn copy_table(
    source: &Connection,
    target: &Connection,
    table: &str,
    replace: bool,
    enabled: Option<&[String]>,
) -> Result<u64, AppError> {
    if !has_table(source, table) {
        return Ok(0);
    }
    let source_columns = columns(source, table)?;
    let selected: Vec<String> = columns(target, table)?
        .into_iter()
        .filter(|c| source_columns.contains(c))
        .collect();
    if selected.is_empty() {
        return Ok(0);
    }
    let fields = selected
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(",");
    let mut statement = source.prepare(&format!("SELECT {fields} FROM \"{table}\""))?;
    let mut rows = statement.query([])?;
    let placeholders = (0..selected.len())
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let sql = if replace {
        format!("INSERT OR REPLACE INTO \"{table}\" ({fields}) VALUES ({placeholders})")
    } else {
        format!("INSERT INTO \"{table}\" ({fields}) VALUES ({placeholders}) ON CONFLICT DO NOTHING")
    };
    let mut insert = target.prepare(&sql)?;
    let app_index = selected.iter().position(|c| c == "app_type");
    let mut count = 0;
    while let Some(row) = rows.next()? {
        let values = (0..selected.len())
            .map(|i| row.get::<_, SqlValue>(i))
            .collect::<Result<Vec<_>, _>>()?;
        if let (Some(index), Some(enabled)) = (app_index, enabled) {
            if let SqlValue::Text(app) = &values[index] {
                if !enabled.contains(app) {
                    continue;
                }
            }
        }
        count += insert.execute(rusqlite::params_from_iter(values))? as u64;
    }
    Ok(count)
}
impl Database {
    pub fn import_cc_switch(
        &self,
        path: &Path,
        enabled: Option<&[String]>,
    ) -> Result<ImportResult, AppError> {
        if !path.is_file() {
            return Err(AppError::InvalidInput(
                "CC Switch database was not found".into(),
            ));
        }
        let own = crate::config::get_app_config_dir().join("observatory.db");
        if path.canonicalize().ok() == own.canonicalize().ok() && own.exists() {
            return Err(AppError::InvalidInput(
                "Cannot import the Observatory database into itself".into(),
            ));
        }
        let source = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        source.busy_timeout(Duration::from_secs(3))?;
        source.execute_batch("PRAGMA query_only=ON; BEGIN DEFERRED;")?;
        for required in ["proxy_request_logs", "usage_daily_rollups"] {
            if !has_table(&source, required) {
                return Err(AppError::InvalidInput(format!(
                    "Missing CC Switch metrics table: {required}"
                )));
            }
        }
        for (table, required) in [
            (
                "proxy_request_logs",
                &[
                    "request_id",
                    "provider_id",
                    "app_type",
                    "model",
                    "input_tokens",
                    "output_tokens",
                    "created_at",
                    "latency_ms",
                    "status_code",
                ][..],
            ),
            (
                "usage_daily_rollups",
                &[
                    "date",
                    "app_type",
                    "provider_id",
                    "model",
                    "request_count",
                    "input_tokens",
                    "output_tokens",
                    "total_cost_usd",
                ][..],
            ),
        ] {
            let available = columns(&source, table)?;
            for column in required {
                if !available.iter().any(|c| c == column) {
                    return Err(AppError::InvalidInput(format!(
                        "源库 {table} 缺少必要字段 {column}，未导入统计。"
                    )));
                }
            }
        }
        let mut conn = lock_conn!(self.conn);
        let tx = conn.transaction()?;
        let mut result = ImportResult::default();
        let source_key = path
            .canonicalize()
            .map_err(|e| AppError::io(path, e))?
            .to_string_lossy()
            .to_string();
        let baseline: Option<String> = tx
            .query_row(
                "SELECT value FROM settings WHERE key='cc_switch_import_complete'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let legacy_baseline: Option<String> = tx
            .query_row(
                "SELECT value FROM settings WHERE key='cc_switch_prices_imported'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let legacy_matches = legacy_baseline
            .as_ref()
            .and_then(|p| std::fs::canonicalize(p).ok())
            .is_some_and(|p| p.to_string_lossy() == source_key);
        let migrated = baseline.is_some() || legacy_matches;
        if let Some(original) = baseline.as_ref() {
            if original != &source_key {
                return Err(AppError::InvalidInput(
                    "已有历史迁移基线，请使用原数据库路径补入明细；其他设备请通过局域网同步接入。"
                        .into(),
                ));
            }
        }
        if !migrated {
            let existing: i64 =
                tx.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |r| r.get(0))?;
            let rollups: i64 =
                source.query_row("SELECT COUNT(*) FROM usage_daily_rollups", [], |r| r.get(0))?;
            if existing > 0 && rollups > 0 {
                return Err(AppError::InvalidInput("本库已有独立采集明细，源库又包含旧日汇总，无法无歧义地合并。请保留当前数据，使用新的 OBSERVATORY_HOME 数据目录先导入历史再开始采集，避免重复累计。".into()));
            }
        }
        result.requests = copy_table(&source, &tx, "proxy_request_logs", false, enabled)?;
        if !migrated {
            result.rollups = copy_table(&source, &tx, "usage_daily_rollups", true, enabled)?;
        } else {
            result
                .warnings
                .push("历史日汇总保留首次迁移基线，本次仅补入缺失明细。".into());
        }
        result.cursors = copy_table(&source, &tx, "session_log_sync", false, None)?;
        result.ledger = copy_table(&source, &tx, "session_usage_dedup", false, None)?;
        if has_table(&source, "providers") {
            let mut statement = source.prepare("SELECT id,app_type,name FROM providers")?;
            let rows = statement.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (id, app, name) = row?;
                tx.execute("INSERT INTO providers(id,app_type,name,settings_config) VALUES(?1,?2,?3,'{}') ON CONFLICT(id,app_type) DO UPDATE SET name=excluded.name",rusqlite::params![id,app,name])?;
            }
        }
        if has_table(&source, "settings") {
            if let Some(id) = source
                .query_row(
                    "SELECT value FROM settings WHERE key='lan_device_id'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
            {
                tx.execute("INSERT INTO settings(key,value) VALUES('cc_switch_local_device_id',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[id])?;
            }
        }
        tx.execute("INSERT INTO settings(key,value) VALUES('cc_switch_import_complete',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[&source_key])?;
        tx.commit()?;
        source.execute_batch("ROLLBACK;")?;
        drop(conn);
        if let Some(parent) = path.parent() {
            let prices = parent.join("model-pricing.json");
            if prices.is_file()
                && self
                    .get_setting("cc_switch_prices_imported")?
                    .as_ref()
                    .and_then(|p| std::fs::canonicalize(p).ok())
                    .is_none_or(|p| p.to_string_lossy() != source_key)
            {
                let attempt = (|| -> Result<usize, AppError> {
                    let text =
                        std::fs::read_to_string(&prices).map_err(|e| AppError::io(&prices, e))?;
                    let value: serde_json::Value =
                        serde_json::from_str(&text).map_err(|e| AppError::json(&prices, e))?;
                    let mut models: Vec<crate::services::model_pricing::ModelPricingInfo> =
                        serde_json::from_value(
                            value
                                .get("models")
                                .cloned()
                                .unwrap_or(serde_json::json!([])),
                        )
                        .map_err(|e| AppError::Config(e.to_string()))?;
                    let own_file = crate::services::model_pricing::model_pricing_file_path();
                    if let Ok(text) = std::fs::read_to_string(own_file) {
                        if let Ok(own) = serde_json::from_str::<serde_json::Value>(&text) {
                            let owned: std::collections::HashSet<String> = own
                                .get("models")
                                .and_then(|v| v.as_array())
                                .into_iter()
                                .flatten()
                                .filter_map(|m| {
                                    m.get("modelId")
                                        .and_then(|v| v.as_str())
                                        .map(str::to_string)
                                })
                                .collect();
                            models.retain(|model| !owned.contains(&model.model_id));
                        }
                    }
                    crate::services::model_pricing::update_model_pricing_batch(self, models)
                })();
                match attempt {
                    Ok(n) => {
                        result.prices = n as u64;
                        self.set_setting("cc_switch_prices_imported", &source_key)?;
                    }
                    Err(e) => result
                        .warnings
                        .push(format!("Price overrides were not imported: {e}")),
                }
            }
        }
        Ok(result)
    }
}
