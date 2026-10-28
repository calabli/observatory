//! ZCode per-request usage, read only; never combine model_usage and turn_usage.
use super::{session_usage::SessionSyncResult, sql_helpers::INPUT_TOKEN_SEMANTICS_FRESH};
use crate::{
    database::{lock_conn, Database},
    error::AppError,
    proxy::usage::{calculator::CostCalculator, parser::TokenUsage},
};
use rusqlite::{params, Connection, OpenFlags};
use std::{path::PathBuf, time::Duration};

pub fn sync_zcode_usage(db: &Database) -> Result<SessionSyncResult, AppError> {
    let path = std::env::var_os("ZCODE_DB")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|p| p.join(".zcode/cli/db/db.sqlite")));
    let Some(path) = path.filter(|p| p.is_file()) else {
        return Ok(SessionSyncResult::default());
    };
    let source = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.busy_timeout(Duration::from_millis(500))?;
    source.execute_batch("PRAGMA query_only=ON")?;
    import_source(db, &source)
}

// Modern ZCode records totals alongside its counters. Accept only a conservative,
// total-preserving interpretation; reasoning is never added twice.
fn normalize(
    input: i64,
    output: i64,
    reasoning: i64,
    read: i64,
    write: i64,
    total: i64,
) -> Option<[u32; 4]> {
    if [input, output, reasoning, read, write]
        .iter()
        .any(|v| *v < 0)
        || total <= 0
    {
        return None;
    }
    let cache = read.checked_add(write)?;
    for fresh in [input.checked_sub(cache).filter(|v| *v >= 0), Some(input)]
        .into_iter()
        .flatten()
    {
        for combined in [Some(output), output.checked_add(reasoning)]
            .into_iter()
            .flatten()
        {
            if fresh.checked_add(cache)?.checked_add(combined)? == total {
                return Some([
                    u32::try_from(fresh).ok()?,
                    u32::try_from(combined).ok()?,
                    u32::try_from(read).ok()?,
                    u32::try_from(write).ok()?,
                ]);
            }
        }
    }
    None
}

fn import_source(db: &Database, source: &Connection) -> Result<SessionSyncResult, AppError> {
    let mut result = SessionSyncResult {
        files_scanned: 1,
        ..Default::default()
    };
    // No timestamp watermark: out-of-order completions and retries must not be lost.
    let mut stmt=source.prepare("SELECT id,session_id,model_id,status,started_at,completed_at,duration_ms,time_to_first_token_ms,input_tokens,output_tokens,reasoning_tokens,cache_read_input_tokens,cache_creation_input_tokens,provider_total_tokens,computed_total_tokens FROM model_usage WHERE status IN ('completed','error','cancelled') AND completed_at IS NOT NULL ORDER BY completed_at,id")?;
    let mut rows = stmt.query([])?;
    let mut conn = lock_conn!(db.conn);
    let tx = conn.transaction()?;
    let mut known = std::collections::HashSet::<String>::new();
    {
        let mut q = tx.prepare(
            "SELECT request_id FROM session_usage_dedup WHERE data_source='zcode_session'",
        )?;
        for id in q.query_map([], |r| r.get::<_, String>(0))? {
            known.insert(id?);
        }
    }
    let mut prices = std::collections::HashMap::new();
    while let Some(row) = rows.next()? {
        let native_id: String = row.get(0)?;
        if native_id.is_empty() {
            result.skipped += 1;
            continue;
        }
        let id = format!("zcode:{native_id}");
        if known.contains(&id) {
            result.skipped += 1;
            continue;
        }
        let provider: Option<i64> = row.get(13)?;
        let computed: Option<i64> = row.get(14)?;
        if matches!((provider,computed),(Some(a),Some(b)) if a>0&&b>0&&a!=b) {
            result.skipped += 1;
            continue;
        }
        let total = provider.filter(|n| *n > 0).or(computed).unwrap_or(0);
        let n = |i: usize| row.get::<_, Option<i64>>(i).map(|v| v.unwrap_or(0));
        let Some([input, output, read, write]) =
            normalize(n(8)?, n(9)?, n(10)?, n(11)?, n(12)?, total)
        else {
            result.skipped += 1;
            continue;
        };
        let session: String = row.get(1)?;
        let model: Option<String> = row.get(2)?;
        let model = model
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "unknown".into());
        let status: String = row.get(3)?;
        let start: Option<i64> = row.get(4)?;
        let completed: i64 = row.get(5)?;
        let created = start.filter(|t| *t > 0).unwrap_or(completed);
        if created <= 0 {
            result.skipped += 1;
            continue;
        }
        let duration: Option<i64> = row.get(6)?;
        let first: Option<i64> = row.get(7)?;
        let usage = TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: read,
            cache_creation_tokens: write,
            cache_creation_1h_tokens: 0,
            model: Some(model.clone()),
            message_id: None,
        };
        let price = prices
            .entry(model.clone())
            .or_insert_with(|| super::usage_stats::find_model_pricing(&tx, &model));
        let costs = price
            .as_ref()
            .map(|p| {
                let c = CostCalculator::calculate_for_app(
                    "zcode",
                    &usage,
                    p,
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
        let fresh=tx.execute("INSERT OR IGNORE INTO session_usage_dedup(data_source,request_id,semantic_id,has_entry_id) VALUES('zcode_session',?1,?1,1)",[&id])?;
        if fresh == 0 {
            result.skipped += 1;
            continue;
        }
        let error = (status != "completed").then(|| format!("ZCode request {status}"));
        tx.execute("INSERT INTO proxy_request_logs(request_id,provider_id,app_type,model,request_model,input_tokens,output_tokens,cache_read_tokens,cache_creation_tokens,input_cost_usd,output_cost_usd,cache_read_cost_usd,cache_creation_cost_usd,total_cost_usd,latency_ms,duration_ms,first_token_ms,status_code,error_message,session_id,provider_type,is_streaming,cost_multiplier,created_at,data_source,input_token_semantics) VALUES(?1,'_zcode_session','zcode',?2,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?12,?13,?14,?15,?16,'zcode_session',1,'1',?17,'zcode_session',?18)",params![id,model,input,output,read,write,costs[0],costs[1],costs[2],costs[3],costs[4],duration.filter(|v|*v>=0).unwrap_or(0),first.filter(|v|*v>=0),if status=="completed"{200}else{0},error,session,created/1000,INPUT_TOKEN_SEMANTICS_FRESH])?;
        known.insert(id);
        result.imported += 1;
    }
    tx.commit()?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE model_usage(id TEXT,session_id TEXT,model_id TEXT,status TEXT,started_at INTEGER,completed_at INTEGER,duration_ms INTEGER,time_to_first_token_ms INTEGER,input_tokens INTEGER,output_tokens INTEGER,reasoning_tokens INTEGER,cache_read_input_tokens INTEGER,cache_creation_input_tokens INTEGER,provider_total_tokens INTEGER,computed_total_tokens INTEGER); INSERT INTO model_usage VALUES('fixture','s','GLM-5.3','completed',1792767776968,1792767779426,2458,1947,27528,37,0,22080,0,27565,27565);").unwrap();
        c
    }
    #[test]
    fn zcode_normalizes_cache_and_reasoning_without_double_counting() {
        assert_eq!(
            normalize(27528, 37, 0, 22080, 0, 27565),
            Some([5448, 37, 22080, 0])
        );
        assert_eq!(normalize(10, 5, 2, 3, 1, 15), Some([6, 5, 3, 1]));
        assert_eq!(normalize(10, 5, 2, 3, 1, 19), Some([10, 5, 3, 1]));
        assert_eq!(normalize(10, 5, 2, 3, 1, 17), Some([6, 7, 3, 1]));
        assert_eq!(normalize(-1, 5, 0, 0, 0, 4), None);
        assert_eq!(normalize(10, 5, 0, 20, 0, 15), None);
    }
    #[test]
    fn zcode_retains_timing_and_deduplicates_after_pruning() {
        let db = Database::memory().unwrap();
        let s = source();
        assert_eq!(import_source(&db, &s).unwrap().imported, 1);
        assert_eq!(import_source(&db, &s).unwrap().imported, 0);
        {
            let c = db.conn.lock().unwrap();
            let got=c.query_row("SELECT input_tokens,output_tokens,cache_read_tokens,latency_ms,first_token_ms,created_at,app_type FROM proxy_request_logs",[],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?,r.get::<_,i64>(5)?,r.get::<_,String>(6)?))).unwrap();
            assert_eq!(
                got,
                (5448, 37, 22080, 2458, 1947, 1792767776, "zcode".into())
            );
            c.execute("DELETE FROM proxy_request_logs", []).unwrap();
        }
        assert_eq!(import_source(&db, &s).unwrap().imported, 0);
    }
    #[test]
    fn zcode_waits_for_completion_and_rejects_inconsistent_totals() {
        let db = Database::memory().unwrap();
        let s = source();
        s.execute(
            "UPDATE model_usage SET status='running',completed_at=NULL",
            [],
        )
        .unwrap();
        assert_eq!(import_source(&db, &s).unwrap().imported, 0);
        s.execute("UPDATE model_usage SET status='completed',completed_at=1792767779426,computed_total_tokens=1",[]).unwrap();
        assert_eq!(import_source(&db, &s).unwrap().imported, 0);
        s.execute("UPDATE model_usage SET computed_total_tokens=27565", [])
            .unwrap();
        assert_eq!(import_source(&db, &s).unwrap().imported, 1);
    }
}
