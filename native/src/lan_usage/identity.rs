//! 本机在局域网里的身份：稳定的 device_id + 一个给人看的名字。

use super::{DeviceInfo, LanShareSettings, DEFAULT_PORT};
use crate::database::Database;
use crate::error::AppError;

const SETTING_DEVICE_ID: &str = "lan_device_id";
const SETTING_DEVICE_NAME: &str = "lan_device_name";
const SETTING_SHARED_APP_TYPES: &str = "lan_shared_app_types";
const SETTING_SHARE_ENABLED: &str = "lan_share_enabled";
const SETTING_SHARE_PORT: &str = "lan_share_port";
const SETTING_AUTO_SYNC: &str = "lan_auto_sync";
const SETTING_AUTO_SYNC_INTERVAL: &str = "lan_auto_sync_interval_secs";

/// 读取本机 device_id；没有就生成一个并落库。
///
/// device_id 必须跨重启稳定，否则对端每次都会当成新设备，历史用量会断成两截。
pub fn device_id(db: &Database) -> Result<String, AppError> {
    if let Some(existing) = db.get_setting(SETTING_DEVICE_ID)? {
        let trimmed = existing.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }
    let generated = uuid::Uuid::new_v4().to_string();
    db.set_setting(SETTING_DEVICE_ID, &generated)?;
    Ok(generated)
}

/// 主机名，拿不到就退回 "cc-switch"。
fn hostname() -> String {
    for key in ["HOSTNAME", "COMPUTERNAME"] {
        if let Ok(v) = std::env::var(key) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return v;
            }
        }
    }
    "Observatory".to_string()
}

/// 用户给本机起的名字（局域网里显示用）。没设置过时用主机名。
pub fn device_name(db: &Database) -> String {
    db.get_setting(SETTING_DEVICE_NAME)
        .ok()
        .flatten()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(hostname)
}

/// 对外分享的名字（可被用户在设置里改），空表示沿用本机名。
pub fn share_device_name(db: &Database) -> String {
    db.get_setting(SETTING_DEVICE_NAME)
        .ok()
        .flatten()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(hostname)
}

/// 组装本机的身份信息，用于自我介绍和被对端识别。
pub fn device_info(db: &Database) -> Result<DeviceInfo, AppError> {
    Ok(DeviceInfo {
        device_id: device_id(db)?,
        device_name: share_device_name(db),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        platform: std::env::consts::OS.to_string(),
        started_at: chrono::Utc::now().timestamp(),
    })
}

/// 读取局域网共享设置。
pub fn share_settings(db: &Database) -> LanShareSettings {
    let mut settings = LanShareSettings::default();
    if let Ok(Some(v)) = db.get_setting(SETTING_SHARE_ENABLED) {
        settings.enabled = matches!(v.trim(), "true" | "1");
    }
    if let Ok(Some(v)) = db.get_setting(SETTING_SHARE_PORT) {
        if let Ok(port) = v.trim().parse::<u16>() {
            if port > 0 {
                settings.port = port;
            }
        }
    }
    if let Ok(Some(value)) = db.get_setting(SETTING_SHARED_APP_TYPES) {
        // Malformed persisted selections fail closed rather than sharing defaults.
        let selected = serde_json::from_str::<Vec<String>>(&value).unwrap_or_default();
        settings.shared_app_types = super::normalize_shared_app_types(&selected);
    }
    settings.device_name = share_device_name(db);
    if let Ok(Some(v)) = db.get_setting(SETTING_AUTO_SYNC) {
        settings.auto_sync = matches!(v.trim(), "true" | "1");
    }
    if let Ok(Some(v)) = db.get_setting(SETTING_AUTO_SYNC_INTERVAL) {
        if let Ok(secs) = v.trim().parse::<u64>() {
            if secs >= 30 {
                settings.auto_sync_interval_secs = secs;
            }
        }
    }
    settings
}

/// 保存局域网共享设置。
pub fn save_share_settings(db: &Database, settings: &LanShareSettings) -> Result<(), AppError> {
    let selected = super::normalize_shared_app_types(&settings.shared_app_types);
    let json = serde_json::to_string(&selected)
        .map_err(|e| AppError::Message(format!("保存共享来源失败: {e}")))?;
    db.set_setting(SETTING_SHARED_APP_TYPES, &json)?;
    db.set_setting(
        SETTING_SHARE_ENABLED,
        if settings.enabled { "true" } else { "false" },
    )?;
    let port = if settings.port == 0 {
        DEFAULT_PORT
    } else {
        settings.port
    };
    db.set_setting(SETTING_SHARE_PORT, &port.to_string())?;
    db.set_setting(SETTING_DEVICE_NAME, settings.device_name.trim())?;
    db.set_setting(
        SETTING_AUTO_SYNC,
        if settings.auto_sync { "true" } else { "false" },
    )?;
    db.set_setting(
        SETTING_AUTO_SYNC_INTERVAL,
        &settings.auto_sync_interval_secs.to_string(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_is_stable_across_calls() {
        let db = Database::memory().unwrap();
        let first = device_id(&db).expect("生成 device_id");
        let second = device_id(&db).expect("再次读取 device_id");
        assert_eq!(first, second);
        assert_eq!(first.len(), 36);
    }

    #[test]
    fn share_settings_round_trip() {
        let db = Database::memory().unwrap();
        let mut settings = share_settings(&db);
        assert!(!settings.enabled, "默认不共享");
        settings.enabled = true;
        settings.port = 18999;
        settings.device_name = "工位机".into();
        save_share_settings(&db, &settings).unwrap();

        let loaded = share_settings(&db);
        assert!(loaded.enabled);
        assert_eq!(loaded.port, 18999);
        assert_eq!(loaded.device_name, "工位机");
    }

    #[test]
    fn invalid_port_falls_back_to_default() {
        let db = Database::memory().unwrap();
        db.set_setting(SETTING_SHARE_PORT, "0").unwrap();
        assert_eq!(share_settings(&db).port, DEFAULT_PORT);
    }
    #[test]
    fn shared_sources_default_to_all_and_support_empty_selection() {
        let db = Database::memory().unwrap();
        let mut settings = share_settings(&db);
        assert_eq!(
            settings.shared_app_types,
            super::super::default_shared_app_types()
        );
        settings.shared_app_types = vec!["mcode".into(), "unknown".into(), "mcode".into()];
        save_share_settings(&db, &settings).unwrap();
        assert_eq!(share_settings(&db).shared_app_types, vec!["mcode"]);
        settings.shared_app_types.clear();
        save_share_settings(&db, &settings).unwrap();
        assert!(share_settings(&db).shared_app_types.is_empty());
        db.set_setting(SETTING_SHARED_APP_TYPES, "invalid json")
            .unwrap();
        assert!(share_settings(&db).shared_app_types.is_empty());
    }

    #[test]
    fn older_settings_without_sources_default_to_all() {
        let settings: LanShareSettings = serde_json::from_value(serde_json::json!({
            "enabled": false, "port": 17890, "deviceName": "old",
            "autoSync": true, "autoSyncIntervalSecs": 300
        }))
        .unwrap();
        assert_eq!(
            settings.shared_app_types,
            super::super::default_shared_app_types()
        );
    }
}
