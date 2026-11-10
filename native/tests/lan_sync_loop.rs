// Two mutually paired devices must not trigger a full download of each other
// just because they store each other's data.

use observatory_usage_core::{
    database::Database,
    error::AppError,
    lan_usage::{client, identity, pairing, server, storage},
};
use std::sync::Arc;

fn seed(db: &Database, prefix: &str, rows: usize) {
    db.with_connection(|conn| {
        for i in 0..rows {
            conn.execute(
                "INSERT INTO proxy_request_logs (request_id, provider_id, app_type, model, input_tokens, output_tokens, latency_ms, status_code, created_at)
                 VALUES (?1, 'p', 'claude', 'm', 10, 5, 100, 200, ?2)",
                rusqlite::params![format!("{prefix}-{i}"), chrono::Utc::now().timestamp() - i as i64],
            )?;
        }
        Ok::<_, AppError>(())
    })
    .unwrap();
}

#[tokio::test]
async fn paired_devices_do_not_redownload_each_other_forever() {
    let a = Arc::new(Database::memory().unwrap());
    let b = Arc::new(Database::memory().unwrap());
    for db in [&a, &b] {
        db.set_setting("usage_enabled_sources", "[]").unwrap();
    }
    seed(&a, "a", 50);
    seed(&b, "b", 50);
    let a_server = server::start(a.clone(), 0).await.unwrap();
    let b_server = server::start(b.clone(), 0).await.unwrap();
    let a_id = identity::device_id(&a).unwrap();
    let b_id = identity::device_id(&b).unwrap();

    // The same two-way pairing as lan_connect: A stores the reverse
    // authorization first, then submits the pairing code to B.
    let (code, _) = pairing::generate(&b).unwrap();
    let reverse = pairing::new_token();
    pairing::store_grant(&a, &b_id, &reverse).unwrap();
    let token = client::pair(
        "127.0.0.1",
        b_server.port,
        &identity::device_info(&a).unwrap(),
        a_server.port,
        &code,
        &reverse,
    )
    .await
    .unwrap();
    storage::register_device(&a, &b_id, "B", "127.0.0.1", b_server.port).unwrap();
    a.set_setting(&format!("lan_outgoing_token_v1:{b_id}"), &token)
        .unwrap();

    // First round: each side pulls the other's full data once. B already pulled
    // A once in the background when the pairing succeeded, so the call here may
    // hit the fingerprint and return 0; only assert that B ends up holding all
    // of A's detail records.
    assert_eq!(client::sync_device(&a, &b_id).await.unwrap(), 50);
    client::sync_device(&b, &a_id).await.unwrap();
    let a_on_b = storage::list_devices(&b)
        .unwrap()
        .into_iter()
        .find(|d| d.device_id == a_id)
        .unwrap();
    assert_eq!(a_on_b.last_imported, 50);

    // After every real download of an export the pulling side records the peer
    // fingerprint; a fingerprint hit skips the download and does not update it.
    // The return value and the write epoch are also 0 / unchanged when the
    // export was downloaded but the content did not change, so on their own
    // they cannot tell whether a whole export was downloaded for nothing.
    let downloaded_marker = |db: &Database, peer: &str| {
        db.get_setting(&format!("lan_last_snapshot_epoch:{peer}"))
            .unwrap()
    };

    // Steady state: neither side's local data changed, so syncing back and forth
    // should neither download again nor rewrite the local database.
    for round in 0..3 {
        let (a_epoch, b_epoch) = (a.data_epoch(), b.data_epoch());
        let (a_marker, b_marker) = (downloaded_marker(&a, &b_id), downloaded_marker(&b, &a_id));
        assert_eq!(
            client::sync_device(&a, &b_id).await.unwrap(),
            0,
            "round {round}: A re-pulled B"
        );
        assert_eq!(
            client::sync_device(&b, &a_id).await.unwrap(),
            0,
            "round {round}: B re-pulled A"
        );
        assert_eq!(a.data_epoch(), a_epoch, "round {round}: A rewrote B's copy");
        assert_eq!(b.data_epoch(), b_epoch, "round {round}: B rewrote A's copy");
        assert_eq!(
            downloaded_marker(&a, &b_id),
            a_marker,
            "round {round}: A re-downloaded B"
        );
        assert_eq!(
            downloaded_marker(&b, &a_id),
            b_marker,
            "round {round}: B re-downloaded A"
        );
    }

    // Only A's local data changed: B pulls A once; A then pulls B and still
    // skips it (B itself did not change).
    seed(&a, "a-new", 1);
    assert_eq!(client::sync_device(&b, &a_id).await.unwrap(), 51);
    assert_eq!(client::sync_device(&a, &b_id).await.unwrap(), 0);

    a_server.stop();
    b_server.stop();
}

#[test]
fn identical_snapshot_with_new_started_at_is_not_rewritten() {
    let peer = Database::memory().unwrap();
    seed(&peer, "p", 5);
    let local = Database::memory().unwrap();
    let mut export = storage::export_local_usage(&peer).unwrap();
    storage::register_device(&local, &export.device.device_id, "P", "127.0.0.1", 1).unwrap();
    assert_eq!(storage::store_export(&local, &export).unwrap(), 5);

    // Older peers fill started_at with the current time on every export: when
    // the content is the same, the whole export must not be rewritten.
    let before = local.data_epoch();
    export.device.started_at += 3600;
    assert_eq!(storage::store_export(&local, &export).unwrap(), 0);
    assert_eq!(local.data_epoch(), before);
}

#[test]
fn storing_peer_copies_does_not_change_export_fingerprint() {
    let peer = Database::memory().unwrap();
    seed(&peer, "p", 5);
    let local = Database::memory().unwrap();
    let export = storage::export_local_usage(&peer).unwrap();
    storage::register_device(&local, &export.device.device_id, "P", "127.0.0.1", 1).unwrap();

    let before = local.export_epoch();
    storage::store_export(&local, &export).unwrap();
    assert_eq!(
        local.export_epoch(),
        before,
        "peer copy changed local export fingerprint"
    );

    seed(&local, "l", 1);
    assert!(
        local.export_epoch() > before,
        "local data must change the fingerprint"
    );
}
