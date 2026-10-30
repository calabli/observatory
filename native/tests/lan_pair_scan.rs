use observatory_usage_core::{
    database::Database,
    lan_usage::{client, identity, pairing, server, storage},
};
use std::sync::Arc;

#[tokio::test]
async fn connect_api_pairs_both_devices_once() {
    use observatory_usage_core::{api::UsageBackend, lan_usage};
    let local = Arc::new(Database::memory().unwrap());
    let remote = Arc::new(Database::memory().unwrap());
    local.set_setting("usage_enabled_sources", "[]").unwrap();
    remote.set_setting("usage_enabled_sources", "[]").unwrap();
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let mut settings = identity::share_settings(&local);
    settings.enabled = true;
    settings.port = port;
    identity::save_share_settings(&local, &settings).unwrap();
    let remote_server = server::start(remote.clone(), 0).await.unwrap();
    let remote_id = identity::device_id(&remote).unwrap();
    let local_id = identity::device_id(&local).unwrap();
    let (code, _) = pairing::generate(&remote).unwrap();
    let backend = UsageBackend::with_database(local.clone());
    let result = backend.call("lan_connect", serde_json::json!({"host":"127.0.0.1","port":remote_server.port,"code":code})).await.unwrap();
    assert!(result["error"].is_null(), "{result}");
    assert_eq!(result["syncPending"], true);
    assert!(storage::list_devices(&local).unwrap().iter().any(|d| d.device_id == remote_id && d.paired));
    assert!(storage::list_devices(&remote).unwrap().iter().any(|d| d.device_id == local_id && d.paired));
    assert!(client::sync_device(&local, &remote_id).await.is_ok());
    assert!(client::sync_device(&remote, &local_id).await.is_ok());
    let result = backend.call("lan_disconnect", serde_json::json!({"deviceId":remote_id})).await.unwrap();
    assert_eq!(result["remoteRevoked"], true);
    assert!(storage::list_devices(&local).unwrap().is_empty());
    assert!(storage::list_devices(&remote).unwrap().is_empty());
    lan_usage::stop_server();
    remote_server.stop();
}

#[tokio::test]
async fn unreachable_initiator_does_not_consume_code() {
    let local = Arc::new(Database::memory().unwrap());
    let remote = Arc::new(Database::memory().unwrap());
    let server = server::start(remote.clone(), 0).await.unwrap();
    let (code, _) = pairing::generate(&remote).unwrap();
    let own = identity::device_info(&local).unwrap();
    let token = pairing::new_token();
    assert!(
        client::pair("127.0.0.1", server.port, &own, 0, &code, &token)
            .await
            .is_err()
    );
    assert!(pairing::accept(&remote, &code, &own.device_id).is_ok());
    server.stop();
}

#[tokio::test]
async fn pairing_controls_read_scan_and_revocation() {
    let local = Arc::new(Database::memory().unwrap());
    let remote = Arc::new(Database::memory().unwrap());
    local.set_setting("usage_enabled_sources", "[]").unwrap();
    remote.set_setting("usage_enabled_sources", "[]").unwrap();
    let handle = server::start(remote.clone(), 0).await.unwrap();
    let local_handle = server::start(local.clone(), 0).await.unwrap();
    let own_id = identity::device_id(&local).unwrap();
    let peer = identity::device_info(&remote).unwrap();
    storage::register_device(&local, &peer.device_id, "fixture", "127.0.0.1", handle.port).unwrap();
    assert!(client::sync_device(&local, &peer.device_id).await.is_err());
    let (code, _) = pairing::generate(&remote).unwrap();
    let reverse_token = pairing::new_token();
    pairing::store_grant(&local, &peer.device_id, &reverse_token).unwrap();
    let token = client::pair(
        "127.0.0.1",
        handle.port,
        &identity::device_info(&local).unwrap(),
        local_handle.port,
        &code,
        &reverse_token,
    )
    .await
    .unwrap();
    assert_eq!(pairing::incoming(&remote).unwrap(), vec![own_id.clone()]);
    local
        .set_setting(&format!("lan_outgoing_token_v1:{}", peer.device_id), &token)
        .unwrap();
    assert!(client::sync_device(&local, &peer.device_id).await.is_ok());
    assert!(client::sync_device(&remote, &own_id).await.is_ok());
    let local_backend = observatory_usage_core::api::UsageBackend::with_database(local.clone());
    let before = local_backend.call("scan_revision", serde_json::json!({})).await.unwrap()["revision"].as_u64().unwrap();
    assert!(client::scan_request(&remote, &own_id, "reverse-key", true)
        .await
        .is_ok());
    let mut reverse = client::scan_request(&remote, &own_id, "reverse-key", false)
        .await
        .unwrap();
    for _ in 0..50 {
        if reverse["state"] != "scanning" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        reverse = client::scan_request(&remote, &own_id, "reverse-key", false)
            .await
            .unwrap();
    }
    assert_eq!(reverse["state"], "completed");
    let after = local_backend.call("scan_revision", serde_json::json!({})).await.unwrap()["revision"].as_u64().unwrap();
    assert!(after > before, "remote scans must advance the local refresh signal");
    let first = client::scan_request(&local, &peer.device_id, "fixture-key", true)
        .await
        .unwrap();
    let retry = client::scan_request(&local, &peer.device_id, "fixture-key", true)
        .await
        .unwrap();
    assert_eq!(first["jobId"], retry["jobId"]);
    let mut status = first;
    for _ in 0..50 {
        if status["state"] != "scanning" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        status = client::scan_request(&local, &peer.device_id, "fixture-key", false)
            .await
            .unwrap();
    }
    assert_eq!(status["state"], "completed");
    client::revoke_remote(&local, &peer.device_id)
        .await
        .unwrap();
    assert!(pairing::incoming(&remote).unwrap().is_empty());
    assert!(storage::list_devices(&remote).unwrap().is_empty());
    assert!(client::sync_device(&local, &peer.device_id).await.is_err());
    assert!(local
        .get_setting(&format!("lan_outgoing_token_v1:{}", peer.device_id))
        .unwrap()
        .is_none());
    assert!(
        client::scan_request(&local, &peer.device_id, "fixture-key", false)
            .await
            .is_err()
    );
    handle.stop();
    local_handle.stop();
}
