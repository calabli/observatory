// The scope of a backfill: how many rows it would still recalculate.
//
// A model without a public price is left alone by the backfill, so it is not
// part of the scope the user is asked to confirm.

use observatory_usage_core::database::Database;

const INSERT: &str = "INSERT INTO proxy_request_logs (
    request_id, provider_id, app_type, model, request_model,
    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
    input_cost_usd, output_cost_usd, cache_read_cost_usd, cache_creation_cost_usd,
    total_cost_usd, latency_ms, status_code, created_at, data_source
) VALUES (";

fn row(id: &str, model: &str, input: i64, output: i64, cache: i64) -> String {
    format!(
        "{INSERT}'{id}', '_codex_session', 'codex', '{model}', '{model}',
         {input}, {output}, {cache}, 0, '0', '0', '0', '0', '0', 0, 200, 1000, 'codex_session');"
    )
}

#[test]
fn the_backfill_scope_excludes_the_models_without_a_public_price() {
    let db = Database::memory().unwrap();
    db.execute_batch(&format!(
        "{}{}{}",
        row("priced", "gpt-6.1-sol", 100, 10, 50),
        row("free", "codex-auto-review", 100, 10, 50),
        row("empty", "gpt-6.1-sol", 0, 0, 0),
    ))
    .expect("insert the usage logs");

    // The priced model counts, the model without a public price does not, and a
    // row without tokens has nothing to recalculate.
    assert_eq!(db.pending_backfill_count().unwrap(), 1);
}
