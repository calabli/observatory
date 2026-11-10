//! Client: pulls the usage from the registered peer devices.

use super::{identity, storage, DeviceInfo, UsageExport};
use crate::database::Database;
use crate::error::AppError;
use std::sync::{Arc, LazyLock};

/// The HTTP client shared by all peer requests: it reuses the connection pool and saves the
/// repeated TCP handshake of every 300 second round. connect_timeout identifies unreachable
/// hosts quickly (on a LAN a refused connection is immediate anyway, the overall timeout is
/// only eaten up when a firewall silently drops packets); the overall timeout is raised --
/// the export of a peer with a big database plus the transfer can easily take much longer than
/// the old 20 seconds, and too short a timeout would keep that device from ever syncing.
static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        // LAN peers must never be forwarded through system/HTTP proxies.
        .no_proxy()
        // Do not follow redirects: the token travels in a custom header and reqwest strips only
        // standard headers such as Authorization/Cookie on a cross-origin redirect, so a custom
        // header would be carried to the new address.
        // The peer protocol never returns a 3xx, so any such response is treated as a failure.
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(3))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .expect("LAN HTTP client 构建失败")
});

/// Upper bound for the response body of a usage export. A normal export of a big database is
/// tens of MB, so there is plenty of headroom; anything above that is treated as a rogue
/// peer (or a man in the middle), so that a single sync cannot eat up all the memory.
const MAX_EXPORT_BYTES: usize = 256 * 1024 * 1024;
/// Upper bound for the small responses (identity, pairing, fingerprint, scan status).
const MAX_SMALL_BYTES: usize = 1024 * 1024;

/// Read the response body in chunks and abort as soon as the accumulated size exceeds
/// `limit`. Content-Length is checked first: a declared size above the limit is rejected right
/// away, and a missing or dishonest declaration is truncated while reading.
async fn read_limited(
    mut resp: reqwest::Response,
    limit: usize,
    url: &str,
) -> Result<Vec<u8>, AppError> {
    let too_large = || AppError::Message(format!("{url} 的响应超过 {} MB 上限", limit >> 20));
    if resp
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(too_large());
    }
    let mut body = Vec::with_capacity(
        resp.content_length()
            .map_or(0, |length| length as usize)
            .min(limit),
    );
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| AppError::Message(format!("读取 {url} 的响应失败: {e}")))?
    {
        if body.len() + chunk.len() > limit {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Read and parse a small JSON response (bounded by [`MAX_SMALL_BYTES`]).
async fn small_json<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
    url: &str,
) -> Result<T, AppError> {
    let body = read_limited(resp, MAX_SMALL_BYTES, url).await?;
    serde_json::from_slice(&body)
        .map_err(|e| AppError::Message(format!("解析 {url} 的响应失败: {e}")))
}

/// Pull the usage export of a peer.
pub async fn fetch_export(
    host: &str,
    port: u16,
    own_id: &str,
    token: &str,
) -> Result<UsageExport, AppError> {
    fetch_export_sized(host, port, own_id, token)
        .await
        .map(|(export, _)| export)
}

/// Same as [`fetch_export`], but also returns the size of the response body in bytes (for the activity log).
async fn fetch_export_sized(
    host: &str,
    port: u16,
    own_id: &str,
    token: &str,
) -> Result<(UsageExport, usize), AppError> {
    let url = format!("http://{host}:{port}/api/lan/usage");
    let resp = HTTP
        .get(&url)
        .header("x-observatory-device", own_id)
        .header("x-observatory-token", token)
        .send()
        .await
        .map_err(|e| AppError::Message(format!("连接 {url} 失败: {e}")))?;
    if resp.status() == reqwest::StatusCode::FORBIDDEN {
        return Err(AppError::HttpStatus {
            status: 403,
            body: "对端已撤销授权，请重新配对".into(),
        });
    }
    if !resp.status().is_success() {
        return Err(AppError::Message(format!(
            "{url} 返回 HTTP {}",
            resp.status().as_u16()
        )));
    }
    // Parsing the whole export with serde costs seconds of CPU (a big database can be tens of MB):
    // buffer it into bytes and move the parsing to the blocking thread pool so that no tokio
    // worker is occupied.
    let body = read_limited(resp, MAX_EXPORT_BYTES, &url).await?;
    let size = body.len();
    tokio::task::spawn_blocking(move || {
        serde_json::from_slice::<UsageExport>(&body)
            .map(|export| (export, size))
            .map_err(|e| AppError::Message(format!("解析 {url} 的响应失败: {e}")))
    })
    .await
    .map_err(|e| AppError::Message(e.to_string()))?
}

pub async fn pair(
    host: &str,
    port: u16,
    own: &DeviceInfo,
    own_port: u16,
    code: &str,
    reverse_token: &str,
) -> Result<String, AppError> {
    let url = format!("http://{host}:{port}/api/lan/pair");
    let response = HTTP
        .post(url)
        .timeout(std::time::Duration::from_secs(10))
        .json(&serde_json::json!({"code":code,"deviceId":own.device_id,"deviceName":own.device_name,"port":own_port,"reverseToken":reverse_token}))
        .send()
        .await
        .map_err(|e| AppError::Message(format!("连接失败: {e}")))?;
    if !response.status().is_success() {
        return Err(AppError::Message(
            if response.status() == reqwest::StatusCode::BAD_GATEWAY {
                "对端无法连接本机，请确认两台设备均开启局域网共享并检查防火墙"
            } else {
                "配对失败：请检查配对码或确认对方已升级"
            }
            .into(),
        ));
    }
    let value: serde_json::Value = small_json(response, "/api/lan/pair").await?;
    value
        .get("token")
        .and_then(|v| v.as_str())
        .filter(|v| v.len() == 64)
        .map(str::to_owned)
        .ok_or_else(|| AppError::Message("配对响应无效".into()))
}

pub async fn scan_request(
    db: &Database,
    device_id: &str,
    key: &str,
    start: bool,
) -> Result<serde_json::Value, AppError> {
    let (host, port) = storage::device_endpoint(db, device_id)?
        .ok_or_else(|| AppError::Message("设备未登记".into()))?;
    let token = db
        .get_setting(&format!("lan_outgoing_token_v1:{device_id}"))?
        .ok_or_else(|| AppError::Message("设备未配对".into()))?;
    let own_id = identity::device_id(db)?;
    let route = if start { "scan" } else { "scan-status" };
    let response = HTTP
        .post(format!("http://{host}:{port}/api/lan/{route}"))
        .timeout(std::time::Duration::from_secs(10))
        .header("x-observatory-device", own_id)
        .header("x-observatory-token", &token)
        .json(&serde_json::json!({"key":key}))
        .send()
        .await
        .map_err(|e| AppError::Message(format!("设备连接失败: {e}")))?;
    if response.status() == reqwest::StatusCode::FORBIDDEN {
        clear_revoked_token(db, device_id, &token)?;
        return Err(AppError::Message("对端已撤销授权，请重新配对".into()));
    }
    if !response.status().is_success() {
        return Err(AppError::Message(format!(
            "设备未配对、任务不存在或版本不支持 (HTTP {})",
            response.status()
        )));
    }
    small_json(response, &format!("/api/lan/{route}")).await
}

pub async fn revoke_remote(db: &Database, device_id: &str) -> Result<(), AppError> {
    let (host, port) = storage::device_endpoint(db, device_id)?
        .ok_or_else(|| AppError::Message("设备未登记".into()))?;
    let token = db
        .get_setting(&format!("lan_outgoing_token_v1:{device_id}"))?
        .ok_or_else(|| AppError::Message("设备未配对".into()))?;
    let response = HTTP
        .post(format!("http://{host}:{port}/api/lan/revoke"))
        .timeout(std::time::Duration::from_secs(5))
        .header("x-observatory-device", identity::device_id(db)?)
        .header("x-observatory-token", token)
        .send()
        .await
        .map_err(|e| AppError::Message(e.to_string()))?;
    if !response.status().is_success() {
        return Err(AppError::Message("对端未确认撤销".into()));
    }
    Ok(())
}

/// Read the identity information of a peer.
pub async fn fetch_info(host: &str, port: u16) -> Result<DeviceInfo, AppError> {
    let url = format!("http://{host}:{port}/api/lan/info");
    let resp = HTTP
        .get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| AppError::Message(format!("连接 {url} 失败: {e}")))?;
    if !resp.status().is_success() {
        return Err(AppError::Message(format!(
            "{url} 返回 HTTP {}",
            resp.status().as_u16()
        )));
    }
    small_json(resp, &url).await
}

/// Sync one registered device: pull -> store -> update the sync state.
///
/// Every round first asks for the peer's write epoch fingerprint
/// (`/api/lan/usage/fingerprint`): if the content did not change the whole download and
/// import round is skipped -- in the vast majority of rounds the peer has no new data and the
/// fingerprint request is only a few hundred bytes. When the peer runs an older version the
/// request answers with 404, which is treated as "not supported" and the full pull happens as
/// usual.
pub async fn sync_device(db: &Arc<Database>, device_id: &str) -> Result<usize, AppError> {
    let started = std::time::Instant::now();
    let mut trace = SyncTrace::default();
    let result = sync_device_inner(db, device_id, &mut trace).await;
    // Record one activity per sync: skipped (fingerprint identical), downloaded and written, or the reason for the failure.
    let duration_ms = started.elapsed().as_millis() as i64;
    let status = match &result {
        Ok(_) if trace.skipped => "skipped",
        Ok(_) => "ok",
        Err(_) => "error",
    };
    let mut summary = serde_json::json!({
        "deviceId": device_id,
        "host": trace.host,
        "port": trace.port,
        "skipped": trace.skipped,
        "downloadedBytes": trace.downloaded_bytes,
        "events": trace.events,
        "daily": trace.daily,
    });
    match &result {
        Ok(written) => summary["writtenRows"] = serde_json::json!(written),
        Err(error) => summary["error"] = serde_json::json!(error.to_string()),
    }
    let log_db = db.clone();
    let log_id = device_id.to_string();
    tokio::task::spawn_blocking(move || {
        let name = storage::device_label(&log_db, &log_id);
        summary["deviceName"] = serde_json::json!(name);
        crate::activity::record(
            &log_db,
            crate::activity::Event {
                kind: "sync",
                title: format!("同步设备 {name}"),
                status,
                duration_ms: Some(duration_ms),
                summary,
                detail: None,
            },
        );
    });
    result
}

/// The record of one sync round, used by the activity log.
#[derive(Default)]
struct SyncTrace {
    host: Option<String>,
    port: Option<u16>,
    /// The peer content fingerprint is identical to last time, skip the download for the whole round.
    skipped: bool,
    downloaded_bytes: Option<usize>,
    events: Option<usize>,
    daily: Option<usize>,
}

async fn sync_device_inner(
    db: &Arc<Database>,
    device_id: &str,
    trace: &mut SyncTrace,
) -> Result<usize, AppError> {
    let (host, port) = match storage::device_endpoint(db, device_id)? {
        Some(v) => v,
        None => {
            let _ = storage::record_sync_result(db, device_id, 0, Some("设备未登记"));
            return Err(AppError::Message("设备未登记".to_string()));
        }
    };

    trace.host = Some(host.clone());
    trace.port = Some(port);
    let token = db
        .get_setting(&format!("lan_outgoing_token_v1:{device_id}"))?
        .ok_or_else(|| AppError::Message("设备未配对，请重新连接".into()))?;
    let own_id = identity::device_id(db)?;
    let peer_epoch = fetch_peer_epoch(&host, port, &own_id, &token).await;
    if let Some(epoch) = peer_epoch {
        let last = db
            .get_setting(&format!("lan_last_snapshot_epoch:{device_id}"))?
            .and_then(|value| value.parse::<u64>().ok());
        if last == Some(epoch) {
            trace.skipped = true;
            return Ok(0);
        }
    }
    match fetch_export_sized(&host, port, &own_id, &token).await {
        Ok((export, size)) => {
            trace.downloaded_bytes = Some(size);
            trace.events = Some(export.events.len());
            trace.daily = Some(export.daily.len());
            // The parsing is done, but the remaining validation, deep copy, serialization and the large
            // transaction are still heavy work: move them to the blocking thread pool together with the
            // database handle.
            let blocking_db = db.clone();
            let blocking_device = device_id.to_string();
            let outcome =
                tokio::task::spawn_blocking(move || -> Result<(usize, Option<u64>), AppError> {
                    // The device_id of the peer is the authoritative one: it prevents somebody from pointing the
                    // URL at another machine and mixing the data into the wrong device.
                    if export.device.device_id != blocking_device {
                        let error = "设备身份不匹配，请重新连接设备";
                        storage::record_sync_result(
                            &blocking_db,
                            &blocking_device,
                            0,
                            Some(error),
                        )?;
                        return Err(AppError::Message(error.to_string()));
                    }
                    let written = storage::store_authorized_export(&blocking_db, &export, &token)?;
                    storage::record_sync_result(
                        &blocking_db,
                        &blocking_device,
                        written as i64,
                        None,
                    )?;
                    Ok((written, peer_epoch))
                })
                .await
                .map_err(|e| AppError::Message(e.to_string()))?;
            let (written, epoch) = outcome?;
            // Remember the fingerprint that belongs to the content of this round: when the content was
            // written to between the download and the store, the peer epoch keeps advancing, so the next
            // round sees a different fingerprint, pulls again automatically and never skips by mistake.
            if let Some(epoch) = epoch {
                db.set_setting_quietly(
                    &format!("lan_last_snapshot_epoch:{device_id}"),
                    &epoch.to_string(),
                )?;
            }
            Ok(written)
        }
        Err(e) => {
            if matches!(e, AppError::HttpStatus { status: 403, .. }) {
                clear_revoked_token(db, device_id, &token)?;
            }
            let _ = storage::record_sync_result(db, device_id, 0, Some(&e.to_string()));
            Err(e)
        }
    }
}

/// Query the peer's write epoch fingerprint; returns None when the peer does not support it
/// (404 on older versions) or the request fails, and the caller falls back to a full pull.
async fn fetch_peer_epoch(host: &str, port: u16, own_id: &str, token: &str) -> Option<u64> {
    let url = format!("http://{host}:{port}/api/lan/usage/fingerprint");
    let resp = HTTP
        .get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .header("x-observatory-device", own_id)
        .header("x-observatory-token", token)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let value: serde_json::Value = small_json(resp, &url).await.ok()?;
    value.get("epoch").and_then(|v| v.as_u64())
}

fn clear_revoked_token(db: &Database, device_id: &str, token: &str) -> Result<(), AppError> {
    let conn = crate::database::lock_conn!(db.conn);
    conn.execute(
        "DELETE FROM settings WHERE key = ?1 AND value = ?2",
        rusqlite::params![format!("lan_outgoing_token_v1:{device_id}"), token],
    )?;
    Ok(())
}

/// Sync every enabled, registered device and return the result of each one.
pub async fn sync_all(db: &Arc<Database>) -> Vec<(String, Result<usize, String>)> {
    let devices = match storage::list_devices(db) {
        Ok(v) => v,
        Err(e) => return vec![(String::new(), Err(e.to_string()))],
    };
    let mut out = Vec::new();
    for device in devices {
        if !device.enabled {
            continue;
        }
        // Skip ourselves
        if device.device_id == identity::device_id(db).unwrap_or_default() {
            continue;
        }
        let result = match sync_device(db, &device.device_id).await {
            Ok(n) => Ok(n),
            Err(e) => Err(e.to_string()),
        };
        out.push((device.device_id.clone(), result));
    }
    out
}
