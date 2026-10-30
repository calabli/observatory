//! 客户端：从已登记的对端设备拉取用量。

use super::{identity, storage, DeviceInfo, UsageExport};
use crate::database::Database;
use crate::error::AppError;

/// 拉取对端的用量导出。
pub async fn fetch_export(
    host: &str,
    port: u16,
    own_id: &str,
    token: &str,
) -> Result<UsageExport, AppError> {
    let url = format!("http://{host}:{port}/api/lan/usage");
    let client = reqwest::Client::builder()
        // LAN peers must never be forwarded through system/HTTP proxies.
        .no_proxy()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| AppError::Message(format!("创建 HTTP 客户端失败: {e}")))?;
    let resp = client
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
    resp.json::<UsageExport>()
        .await
        .map_err(|e| AppError::Message(format!("解析 {url} 的响应失败: {e}")))
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
    let response = reqwest::Client::builder().no_proxy().timeout(std::time::Duration::from_secs(10)).build()
        .map_err(|e| AppError::Message(e.to_string()))?.post(url)
        .json(&serde_json::json!({"code":code,"deviceId":own.device_id,"deviceName":own.device_name,"port":own_port,"reverseToken":reverse_token})).send().await
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
    let value: serde_json::Value = response
        .json()
        .await
        .map_err(|e| AppError::Message(e.to_string()))?;
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
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| AppError::Message(e.to_string()))?
        .post(format!("http://{host}:{port}/api/lan/{route}"))
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
    response
        .json()
        .await
        .map_err(|e| AppError::Message(e.to_string()))
}

pub async fn revoke_remote(db: &Database, device_id: &str) -> Result<(), AppError> {
    let (host, port) = storage::device_endpoint(db, device_id)?
        .ok_or_else(|| AppError::Message("设备未登记".into()))?;
    let token = db
        .get_setting(&format!("lan_outgoing_token_v1:{device_id}"))?
        .ok_or_else(|| AppError::Message("设备未配对".into()))?;
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| AppError::Message(e.to_string()))?
        .post(format!("http://{host}:{port}/api/lan/revoke"))
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

/// 读取对端的身份信息。
pub async fn fetch_info(host: &str, port: u16) -> Result<DeviceInfo, AppError> {
    let url = format!("http://{host}:{port}/api/lan/info");
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| AppError::Message(format!("创建 HTTP 客户端失败: {e}")))?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| AppError::Message(format!("连接 {url} 失败: {e}")))?;
    if !resp.status().is_success() {
        return Err(AppError::Message(format!(
            "{url} 返回 HTTP {}",
            resp.status().as_u16()
        )));
    }
    resp.json::<DeviceInfo>()
        .await
        .map_err(|e| AppError::Message(format!("解析 {url} 的响应失败: {e}")))
}

/// 同步一台已登记的设备：拉取 → 落库 → 更新同步状态。
pub async fn sync_device(db: &Database, device_id: &str) -> Result<usize, AppError> {
    let (host, port) = match storage::device_endpoint(db, device_id)? {
        Some(v) => v,
        None => {
            let _ = storage::record_sync_result(db, device_id, 0, Some("设备未登记"));
            return Err(AppError::Message("设备未登记".to_string()));
        }
    };

    let token = db
        .get_setting(&format!("lan_outgoing_token_v1:{device_id}"))?
        .ok_or_else(|| AppError::Message("设备未配对，请重新连接".into()))?;
    let own_id = identity::device_id(db)?;
    match fetch_export(&host, port, &own_id, &token).await {
        Ok(export) => {
            // 对端的 device_id 才是权威：防止有人改了 URL 指向别的机器，
            // 导致数据串到错误的设备名下。
            if export.device.device_id != device_id {
                let error = "设备身份不匹配，请重新连接设备";
                storage::record_sync_result(db, device_id, 0, Some(error))?;
                return Err(AppError::Message(error.to_string()));
            }
            let written = storage::store_authorized_export(db, &export, &token)?;
            storage::record_sync_result(db, device_id, written as i64, None)?;
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

fn clear_revoked_token(db: &Database, device_id: &str, token: &str) -> Result<(), AppError> {
    let conn = crate::database::lock_conn!(db.conn);
    conn.execute(
        "DELETE FROM settings WHERE key = ?1 AND value = ?2",
        rusqlite::params![format!("lan_outgoing_token_v1:{device_id}"), token],
    )?;
    Ok(())
}

/// 同步所有已启用、登记过的设备，返回每台设备的结果。
pub async fn sync_all(db: &Database) -> Vec<(String, Result<usize, String>)> {
    let devices = match storage::list_devices(db) {
        Ok(v) => v,
        Err(e) => return vec![(String::new(), Err(e.to_string()))],
    };
    let mut out = Vec::new();
    for device in devices {
        if !device.enabled {
            continue;
        }
        // 跳过自己
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
