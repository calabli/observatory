//! Pruning (rollup_and_prune) contract: detail rows outside the retention
//! window are folded into a daily rollup and then deleted, and the total
//! returned by queries stays the same before and after the deletion (nothing
//! double counted, nothing lost); detail rows added later on the same date are
//! accumulated into the existing rollup row when pruning runs again; session
//! twin rows hidden by a proxy row are deleted along with the batch but do not
//! produce a rollup.

use observatory_usage_core::{database::Database, error::AppError};

fn seed_row(
    db: &Database,
    request_id: &str,
    data_source: &str,
    age_seconds: i64,
    input: i64,
    output: i64,
    cost: &str,
) {
    db.execute_batch(&format!(
        "INSERT INTO proxy_request_logs (
            request_id, provider_id, app_type, model, request_model, pricing_model,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            input_token_semantics, input_cost_usd, output_cost_usd, cache_read_cost_usd,
            cache_creation_cost_usd, total_cost_usd, latency_ms, status_code,
            created_at, data_source)
         VALUES ('{request_id}', '_session', 'claude', 'test-model', 'test-model', '',
                 {input}, {output}, 0, 0, 2, '0', '0', '0', '0', '{cost}',
                 1000, 200, strftime('%s', 'now') - {age_seconds}, '{data_source}')"
    ))
    .unwrap();
}

fn total_requests(db: &Database) -> u64 {
    db.get_usage_summary(None, None, None, None, None)
        .unwrap()
        .total_requests
}

fn detail_count(db: &Database) -> i64 {
    db.with_connection(|conn| {
        conn.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |row| {
            row.get(0)
        })
        .map_err(AppError::from)
    })
    .unwrap()
}

fn rollup_rows(db: &Database) -> Vec<(i64, i64, String)> {
    db.with_connection(|conn| {
        let mut stmt = conn
            .prepare("SELECT request_count, input_tokens, total_cost_usd FROM usage_daily_rollups")
            .map_err(AppError::from)?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(AppError::from)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    })
    .unwrap()
}

#[test]
fn prunes_old_details_without_changing_totals() {
    let db = Database::memory().unwrap();
    let day = 86_400i64;
    // A proxy row from 40 days ago plus its session twin (same token, five
    // seconds apart, hidden by the proxy row), plus one recent row from one
    // day ago.
    seed_row(&db, "old-proxy", "proxy", 40 * day, 100, 50, "0.01");
    seed_row(
        &db,
        "old-session",
        "session_log",
        40 * day + 5,
        100,
        50,
        "0.01",
    );
    seed_row(&db, "recent", "proxy", day, 10, 5, "0.001");
    let before = total_requests(&db);
    assert_eq!(before, 2, "会话孪生行应被跨源去重隐藏");

    let pruned = db.rollup_and_prune(30).unwrap();
    assert_eq!(pruned, 2, "到期代理行与隐藏孪生行都应删除");
    assert_eq!(detail_count(&db), 1);
    assert_eq!(
        rollup_rows(&db),
        vec![(1, 100, "0.01".to_string())],
        "只有可见的代理行产生汇总"
    );

    // The total stays the same before and after pruning: nothing double
    // counted, nothing lost.
    assert_eq!(total_requests(&db), before);
    // Steady state: running it again has nothing to do.
    assert_eq!(db.rollup_and_prune(30).unwrap(), 0);
    assert_eq!(total_requests(&db), before);
}

#[test]
fn late_details_for_pruned_date_accumulate_into_existing_rollup() {
    let db = Database::memory().unwrap();
    let day = 86_400i64;
    // Both rows are stored within the same second so they land on the same
    // local date, regardless of when the test runs.
    seed_row(&db, "first", "proxy", 40 * day, 100, 50, "0.01");
    assert_eq!(db.rollup_and_prune(30).unwrap(), 1);

    // A "late" new detail row on an already pruned date (same key, different
    // request): on the query side the rollup + the detail rows are each
    // counted exactly once.
    seed_row(&db, "late", "proxy", 40 * day, 30, 5, "0.002");
    assert_eq!(total_requests(&db), 2);

    assert_eq!(db.rollup_and_prune(30).unwrap(), 1);
    assert_eq!(
        rollup_rows(&db),
        vec![(2, 130, "0.012".to_string())],
        "同键累加而非新行"
    );
    assert_eq!(detail_count(&db), 0);
    assert_eq!(total_requests(&db), 2);
}

#[test]
fn hidden_twin_straddling_cutoff_is_deleted_without_rollup() {
    let db = Database::memory().unwrap();
    let day = 86_400i64;
    // The proxy row is 5 seconds above the pruning line (kept), the session
    // twin is 5 seconds below it (expired): the twin is hidden by the proxy
    // row that is still alive -> it is deleted along with the batch but
    // produces no rollup, so the total does not change.
    // Otherwise, once the proxy row is pruned later, the twin would flip from
    // "hidden" to "visible" and cause a double count.
    seed_row(&db, "live-proxy", "proxy", 30 * day - 5, 100, 50, "0.01");
    seed_row(
        &db,
        "old-twin",
        "session_log",
        30 * day + 5,
        100,
        50,
        "0.01",
    );
    let before = total_requests(&db);
    assert_eq!(before, 1);

    assert_eq!(db.rollup_and_prune(30).unwrap(), 1);
    assert_eq!(detail_count(&db), 1);
    assert!(rollup_rows(&db).is_empty(), "隐藏行不产生汇总");
    assert_eq!(total_requests(&db), before);
}
