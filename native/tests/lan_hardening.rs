// Edge cases of the LAN interface: the query scope of a pairing token, the
// response body size limit, and not following redirects.

use observatory_usage_core::{
    database::Database,
    lan_usage::{client, pairing, public_api, server},
};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Start a TCP server that only replies with a fixed raw HTTP response; returns
/// the port.
async fn raw_http_server(response: String) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let response = response.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    port
}

#[tokio::test]
async fn peer_token_can_only_query_local_usage() {
    let db = Arc::new(Database::memory().unwrap());
    let handle = server::start(db.clone(), 0).await.unwrap();
    let (code, _) = pairing::generate(&db).unwrap();
    let token = pairing::accept(&db, &code, "peer-b").unwrap();
    let api_key = public_api::rotate_key(&db).unwrap();
    let http = reqwest::Client::new();
    let url = |query: &str| {
        format!(
            "http://127.0.0.1:{}/api/v1/usage/summary?days=1{query}",
            handle.port
        )
    };
    let as_peer = |query: &str| {
        http.get(url(query))
            .header("x-observatory-device", "peer-b")
            .header("x-observatory-token", &token)
            .send()
    };

    for query in ["", "&devices=local", "&devices=LOCAL"] {
        let resp = as_peer(query).await.unwrap();
        assert_eq!(resp.status(), 200, "peer query {query:?}");
    }
    for query in [
        "&devices=all",
        "&devices=local,some-peer",
        "&devices=some-peer",
    ] {
        let resp = as_peer(query).await.unwrap();
        assert_eq!(resp.status(), 403, "peer query {query:?}");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(
            body["code"], "DEVICE_SCOPE_FORBIDDEN",
            "peer query {query:?}"
        );
    }

    // The read-only API key is the local user's own credential, so the
    // multi-device scope stays available.
    let resp = http
        .get(url("&devices=all"))
        .header("x-api-key", &api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // With neither credential: 401.
    let resp = http.get(url("")).send().await.unwrap();
    assert_eq!(resp.status(), 401);
    handle.stop();
}

#[tokio::test]
async fn oversized_export_is_rejected_before_reading() {
    let port = raw_http_server(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 300000000\r\n\r\n{"
            .to_string(),
    )
    .await;
    let error = client::fetch_export("127.0.0.1", port, "me", &"0".repeat(64))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("上限"), "{error}");
}

#[tokio::test]
async fn redirects_are_not_followed() {
    // The redirect target is a real, working Observatory service: if the
    // client followed redirects, fetch_info would succeed.
    let target_db = Arc::new(Database::memory().unwrap());
    let target = server::start(target_db, 0).await.unwrap();
    assert!(client::fetch_info("127.0.0.1", target.port).await.is_ok());

    let redirector = raw_http_server(format!(
        "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{}/api/lan/info\r\nContent-Length: 0\r\n\r\n",
        target.port
    ))
    .await;
    assert!(client::fetch_info("127.0.0.1", redirector).await.is_err());
    target.stop();
}
