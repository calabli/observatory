// The model id is a case-insensitive key.
//
// The built-in prices, the ids models.dev publishes and the ids the user
// types by hand spell the same model in different cases (`minimax-m3` and
// `MiniMax-M3` are the same model). They all have to address one row,
// otherwise the price list shows the same model twice and an exact lookup can
// hit an arbitrary one of them.

// The std::sync::MutexGuard returned by `isolate()` serializes the tests that
// change environment variables and is held across an await: every
// #[tokio::test] has its own single-threaded runtime and the other tests
// merely block on their own threads, so there is no deadlock.
#![allow(clippy::await_holding_lock)]

use observatory_usage_core::{api::UsageBackend, database::Database, error::AppError};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

/// The pricing file and the data directory live in the environment, so the
/// tests that change them are serialized.
fn isolate() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

fn pricing(model_id: &str, input: &str) -> Value {
    json!({
        "modelId": model_id,
        "displayName": model_id,
        "inputCostPerMillion": input,
        "outputCostPerMillion": "2",
        "cacheReadCostPerMillion": "0.1",
        "cacheCreationCostPerMillion": "0",
    })
}

fn rows_of(list: &Value, model_id: &str) -> Vec<Value> {
    list.as_array()
        .expect("a price list")
        .iter()
        .filter(|entry| {
            entry["modelId"]
                .as_str()
                .is_some_and(|id| id.eq_ignore_ascii_case(model_id))
        })
        .cloned()
        .collect()
}

#[tokio::test]
async fn a_model_id_is_one_row_whatever_case_it_is_spelled_in() {
    let _guard = isolate();
    let temp = tempfile::tempdir().unwrap();
    let previous = std::env::var_os("OBSERVATORY_HOME");
    std::env::set_var("OBSERVATORY_HOME", temp.path());

    let db = Arc::new(Database::memory().unwrap());
    let backend = UsageBackend::with_database(db.clone());

    backend
        .call("pricing_update", pricing("MiniMax-M3", "1"))
        .await
        .unwrap();
    backend
        .call("pricing_update", pricing("minimax-m3", "9"))
        .await
        .unwrap();

    let list = backend.call("pricing_list", json!({})).await.unwrap();
    let rows = rows_of(&list, "minimax-m3");
    assert_eq!(rows.len(), 1, "the case variants must share one row");
    assert_eq!(rows[0]["modelId"], "minimax-m3");

    // The price of the last write wins and is found under either spelling.
    for spelling in ["MiniMax-M3", "minimax-m3", "MINIMAX-M3"] {
        let input: String = db
            .with_read_connection(|conn| {
                conn.query_row(
                    "SELECT input_cost_per_million FROM model_pricing
                     WHERE model_id = ?1 COLLATE NOCASE",
                    rusqlite::params![spelling],
                    |row| row.get(0),
                )
                .map_err(|error| AppError::Database(error.to_string()))
            })
            .unwrap();
        assert_eq!(input, "9", "spelling {spelling} must find the row");
    }

    // The models.dev import writes a whole batch and has to fold the same way.
    backend
        .call(
            "pricing_batch",
            json!({ "entries": [pricing("MINIMAX-M3", "7")] }),
        )
        .await
        .unwrap();
    let list = backend.call("pricing_list", json!({})).await.unwrap();
    let rows = rows_of(&list, "minimax-m3");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["inputCostPerMillion"], "7");

    // A deletion addresses the model by id, so it ignores the case too.
    backend
        .call("pricing_delete", json!({ "modelId": "MiniMax-M3" }))
        .await
        .unwrap();
    let list = backend.call("pricing_list", json!({})).await.unwrap();
    assert!(
        rows_of(&list, "minimax-m3").is_empty(),
        "the deleted model must be gone"
    );

    match previous {
        Some(value) => std::env::set_var("OBSERVATORY_HOME", value),
        None => std::env::remove_var("OBSERVATORY_HOME"),
    }
}
