//! The identity of this machine on the LAN: a stable device_id plus a human readable name.

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

/// Read the local device_id; generate and persist one if there is none.
///
/// The device_id has to stay stable across restarts, otherwise the peer treats the machine
/// as a new device every time and the historical usage is split in two.
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

/// The host name, falling back to "cc-switch" when it cannot be determined.
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

/// The name the user gave this machine (shown on the LAN). The host name is used when it is not set.
pub fn device_name(db: &Database) -> String {
    db.get_setting(SETTING_DEVICE_NAME)
        .ok()
        .flatten()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(hostname)
}

/// The name shared with others (the user can change it in the settings); empty means the local name is used.
pub fn share_device_name(db: &Database) -> String {
    db.get_setting(SETTING_DEVICE_NAME)
        .ok()
        .flatten()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(hostname)
}

/// Assemble the local identity, used to introduce this machine and to be recognized by peers.
pub fn device_info(db: &Database) -> Result<DeviceInfo, AppError> {
    Ok(DeviceInfo {
        device_id: device_id(db)?,
        device_name: share_device_name(db),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        platform: std::env::consts::OS.to_string(),
        started_at: process_started_at(),
    })
}

/// The moment this process first fetched the identity, which by field convention serves as the
/// "process start time". It used to be the current time on every call, which made the export
/// of the same data differ every time and the peer's "snapshot unchanged, skip it" short
/// circuit never hit.
fn process_started_at() -> i64 {
    static STARTED_AT: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *STARTED_AT.get_or_init(|| chrono::Utc::now().timestamp())
}

/// A short-TTL cache dedicated to UDP replies: discovery responses have no authentication and
/// no rate limiting, and the original implementation grabbed the connection lock for every
/// probe packet just to read the settings. Changes such as the device name show up in the
/// reply after at most 5 seconds.
static DEVICE_INFO_CACHE: std::sync::Mutex<Option<(std::time::Instant, DeviceInfo)>> =
    std::sync::Mutex::new(None);

pub fn device_info_cached(db: &Database) -> Result<DeviceInfo, AppError> {
    if let Ok(guard) = DEVICE_INFO_CACHE.lock() {
        if let Some((at, info)) = guard.as_ref() {
            if at.elapsed() < std::time::Duration::from_secs(5) {
                return Ok(info.clone());
            }
        }
    }
    let info = device_info(db)?;
    if let Ok(mut guard) = DEVICE_INFO_CACHE.lock() {
        *guard = Some((std::time::Instant::now(), info.clone()));
    }
    Ok(info)
}

/// Read the LAN sharing settings.
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

/// Save the LAN sharing settings.
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
