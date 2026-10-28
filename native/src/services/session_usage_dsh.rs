//! Read-only DeepSeek Harness v4 session import. No credentials or plugin required.
use super::{
    session_usage::{load_sync_cursors, SessionSyncResult},
    sql_helpers::INPUT_TOKEN_SEMANTICS_FRESH,
};
use crate::{
    database::{lock_conn, Database},
    error::AppError,
    proxy::usage::{calculator::CostCalculator, parser::TokenUsage},
};
use rusqlite::params;
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

pub fn sync_dsh_usage(db: &Database) -> Result<SessionSyncResult, AppError> {
    let home = std::env::var_os("DSH_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|p| p.join(".dsh")));
    let Some(home) = home else {
        return Ok(SessionSyncResult::default());
    };
    sync_dir(db, &home.join("sessions"))
}

fn collect_logs(root: &Path, depth: usize, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if depth > 4 {
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect_logs(&entry.path(), depth + 1, files)?;
        } else if kind.is_file()
            && matches!(
                entry.file_name().to_str(),
                Some("session.v4.jsonl.zstd" | "session.v4.jsonl")
            )
        {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn sync_dir(db: &Database, root: &Path) -> Result<SessionSyncResult, AppError> {
    let mut result = SessionSyncResult::default();
    if !root.exists() {
        return Ok(result);
    }
    let mut files = Vec::new();
    collect_logs(root, 0, &mut files).map_err(|e| AppError::io(root, e))?;
    let cursors = load_sync_cursors(db)?;
    for path in files {
        let key = format!("dsh:{}", path.display());
        let metadata = match fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => {
                result.deferred_files += 1;
                continue;
            }
        };
        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
            .unwrap_or(0);
        let size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
        if modified != 0
            && cursors
                .get(&key)
                .is_some_and(|c| c.last_modified == modified && c.last_byte_offset == Some(size))
        {
            continue;
        }
        result.files_scanned += 1;
        let reader = (|| -> std::io::Result<Box<dyn Read>> {
            let file = fs::File::open(&path)?;
            if path.extension().is_some_and(|e| e == "zstd") {
                // Decoder reads concatenated frames, including the live append-only format.
                Ok(Box::new(zstd::stream::read::Decoder::new(file)?))
            } else {
                Ok(Box::new(file))
            }
        })();
        match reader {
            Ok(reader) => {
                let (imported, skipped, complete) = import_reader(db, reader)?;
                result.imported += imported;
                result.skipped += skipped;
                // A changing/truncated tail must be retried even if mtime resolution is coarse.
                let stable = fs::metadata(&path).ok().is_some_and(|m| {
                    m.len() == metadata.len() && m.modified().ok() == metadata.modified().ok()
                });
                if complete && stable {
                    let conn = lock_conn!(db.conn);
                    conn.execute("INSERT INTO session_log_sync(file_path,last_modified,last_line_offset,last_synced_at,last_byte_offset) VALUES (?1,?2,0,unixepoch(),?3) ON CONFLICT(file_path) DO UPDATE SET last_modified=?2,last_byte_offset=?3,last_synced_at=unixepoch()", params![key,modified,size])?;
                } else {
                    result.deferred_files += 1;
                }
            }
            Err(_) => result.deferred_files += 1,
        }
    }
    Ok(result)
}

fn count(value: &Value, key: &str, required: bool) -> Option<u32> {
    match value.get(key) {
        None | Some(Value::Null) if !required => Some(0),
        Some(v) => v.as_u64().and_then(|n| u32::try_from(n).ok()),
        _ => None,
    }
}

fn import_reader(db: &Database, reader: impl Read) -> Result<(u32, u32, bool), AppError> {
    let mut reader = BufReader::new(reader.take(256 * 1024 * 1024));
    let mut line = String::new();
    let mut session = None;
    let mut seeded = false;
    let mut complete = true;
    let mut conn = lock_conn!(db.conn);
    let tx = conn.transaction()?;
    let (mut imported, mut skipped) = (0, 0);
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Err(_) => {
                complete = false;
                break;
            }
            Ok(_) => {}
        }
        if !line.ends_with('\n') {
            complete = false;
            break;
        }
        let row: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                complete = false;
                continue;
            }
        };
        match row["type"].as_str() {
            Some("session") => {
                if row["version"].as_u64() != Some(4) {
                    complete = false;
                    break;
                }
                session = row["id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
                seeded = row["isSeeded"].as_bool().unwrap_or(false);
                continue;
            }
            Some("session/end-seed") => {
                seeded = false;
                continue;
            }
            Some("assistant/message") if !seeded => {}
            _ => continue,
        }
        let Some(session) = &session else {
            continue;
        };
        let Some(seq) = row["seq"].as_u64() else {
            continue;
        };
        let Some(time) = row["time"].as_i64().filter(|v| *v > 0) else {
            continue;
        };
        let native = &row["data"]["usage"];
        let (Some(input), Some(output), Some(cache), Some(write)) = (
            count(native, "inputTokens", true),
            count(native, "outputTokens", true),
            count(native, "cacheReadTokens", false),
            count(native, "cacheWriteTokens", false),
        ) else {
            continue;
        };
        if [input, output, cache, write].iter().all(|v| *v == 0) {
            continue;
        }
        let model = row["data"]["message"]["source"]["model"]
            .as_str()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or("unknown");
        let id = format!("dsh:{session}:{seq}");
        // Tombstones survive pruning, so re-reading a rewritten log cannot resurrect history.
        let fresh = tx.execute("INSERT OR IGNORE INTO session_usage_dedup(data_source,request_id,semantic_id,has_entry_id) VALUES('dsh_session',?1,?1,1)", [&id])?;
        if fresh == 0 {
            skipped += 1;
            continue;
        }
        let usage = TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache,
            cache_creation_tokens: write,
            cache_creation_1h_tokens: 0,
            model: Some(model.into()),
            message_id: None,
        };
        let costs = super::usage_stats::find_model_pricing(&tx, model)
            .map(|p| {
                let c = CostCalculator::calculate_for_app(
                    "dsh",
                    &usage,
                    &p,
                    rust_decimal::Decimal::ONE,
                );
                [
                    c.input_cost,
                    c.output_cost,
                    c.cache_read_cost,
                    c.cache_creation_cost,
                    c.total_cost,
                ]
                .map(|v| v.to_string())
            })
            .unwrap_or_else(|| std::array::from_fn(|_| "0".into()));
        // Harness inputTokens excludes cache; output already contains reasoning.
        tx.execute("INSERT INTO proxy_request_logs(request_id,provider_id,app_type,model,request_model,input_tokens,output_tokens,cache_read_tokens,cache_creation_tokens,input_cost_usd,output_cost_usd,cache_read_cost_usd,cache_creation_cost_usd,total_cost_usd,latency_ms,status_code,session_id,provider_type,is_streaming,cost_multiplier,created_at,data_source,input_token_semantics) VALUES(?1,'_dsh_session','dsh',?2,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,0,200,?12,'dsh_session',1,'1',?13,'dsh_session',?14)",params![id,model,input,output,cache,write,costs[0],costs[1],costs[2],costs[3],costs[4],session,time/1000,INPUT_TOKEN_SEMANTICS_FRESH])?;
        imported += 1;
    }
    if reader.get_ref().limit() == 0 {
        complete = false;
    }
    tx.commit()?;
    Ok((imported, skipped, complete))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn log(seeded: bool) -> String {
        format!(
            "{}\n",
            json!({"type":"session","version":4,"id":"s","isSeeded":seeded})
        )
    }
    fn event(seq: u64) -> Value {
        json!({"type":"assistant/message","seq":seq,"time":1792509550806_i64,"data":{"message":{"source":{"model":"fixture-unpriced"}},"usage":{"inputTokens":80,"outputTokens":12,"cacheReadTokens":20,"cacheWriteTokens":5}}})
    }
    #[test]
    fn dsh_preserves_native_semantics_and_never_resurrects_pruned_rows() {
        let db = Database::memory().unwrap();
        let text = format!("{}{}\n", log(false), event(1));
        assert_eq!(import_reader(&db, text.as_bytes()).unwrap(), (1, 0, true));
        assert_eq!(import_reader(&db, text.as_bytes()).unwrap(), (0, 1, true));
        {
            let conn = db.conn.lock().unwrap();
            let row=conn.query_row("SELECT model,input_tokens,output_tokens,cache_read_tokens,cache_creation_tokens,created_at,total_cost_usd,input_token_semantics FROM proxy_request_logs",[],|r|Ok((r.get::<_,String>(0)?,r.get::<_,u32>(1)?,r.get::<_,u32>(2)?,r.get::<_,u32>(3)?,r.get::<_,u32>(4)?,r.get::<_,i64>(5)?,r.get::<_,String>(6)?,r.get::<_,i64>(7)?))).unwrap();
            assert_eq!(
                row,
                (
                    "fixture-unpriced".into(),
                    80,
                    12,
                    20,
                    5,
                    1792509550,
                    "0".into(),
                    INPUT_TOKEN_SEMANTICS_FRESH
                )
            );
            conn.execute("DELETE FROM proxy_request_logs", []).unwrap();
        }
        assert_eq!(import_reader(&db, text.as_bytes()).unwrap(), (0, 1, true));
    }
    #[test]
    fn dsh_skips_seed_history_and_retries_partial_tails() {
        let db = Database::memory().unwrap();
        let text = format!(
            "{}{}\n{{\"type\":\"session/end-seed\"}}\n{}\n{{\"type\":",
            log(true),
            event(1),
            event(2)
        );
        assert_eq!(import_reader(&db, text.as_bytes()).unwrap(), (1, 0, false));
        let mut invalid = event(3);
        invalid["data"]["usage"]["inputTokens"] = json!(-1);
        let text = format!("{}{}\n{}\n", log(false), invalid, event(4));
        assert_eq!(import_reader(&db, text.as_bytes()).unwrap(), (1, 0, true));
    }
    #[test]
    fn dsh_reads_all_zstd_frames_and_syncs_only_changed_files() {
        let db = Database::memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.v4.jsonl.zstd");
        let mut bytes = zstd::stream::encode_all(log(false).as_bytes(), 3).unwrap();
        bytes.extend(zstd::stream::encode_all(format!("{}\n", event(1)).as_bytes(), 3).unwrap());
        fs::write(&path, &bytes).unwrap();
        assert_eq!(sync_dir(&db, dir.path()).unwrap().imported, 1);
        assert_eq!(sync_dir(&db, dir.path()).unwrap().files_scanned, 0);
        bytes.extend(zstd::stream::encode_all(format!("{}\n", event(2)).as_bytes(), 3).unwrap());
        fs::write(&path, &bytes).unwrap();
        assert_eq!(sync_dir(&db, dir.path()).unwrap().imported, 1);
    }
}
