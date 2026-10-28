//! 客户端：从已登记的对端设备拉取用量。

use super::{identity, storage, DeviceInfo, UsageExport};
use crate::database::Database;
use crate::error::AppError;

/// 拉取对端的用量导出。
pub async fn fetch_export(host: &str, port: u16) -> Result<UsageExport, AppError> {
    let url = format!("http://{host}:{port}/api/lan/usage");
    let client = reqwest::Client::builder()
        // LAN peers must never be forwarded through system/HTTP proxies.
        .no_proxy()
        .timeout(std::time::Duration::from_secs(20))
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
    resp.json::<UsageExport>()
        .await
        .map_err(|e| AppError::Message(format!("解析 {url} 的响应失败: {e}")))
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

    match fetch_export(&host, port).await {
        Ok(export) => {
            // 对端的 device_id 才是权威：防止有人改了 URL 指向别的机器，
            // 导致数据串到错误的设备名下。
            if export.device.device_id != device_id {
                let error = "设备身份不匹配，请重新连接设备";
                storage::record_sync_result(db, device_id, 0, Some(error))?;
                return Err(AppError::Message(error.to_string()));
            }
            let written = storage::store_export(db, &export)?;
            storage::record_sync_result(db, device_id, written as i64, None)?;
            Ok(written)
        }
        Err(e) => {
            let _ = storage::record_sync_result(db, device_id, 0, Some(&e.to_string()));
            Err(e)
        }
    }
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
