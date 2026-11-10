//! Simple one-use pairing codes for trusted LANs. This is not an encrypted protocol.
use crate::{
    database::{lock_conn, Database},
    error::AppError,
};
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

const CODE_KEY: &str = "lan_pairing_code_v1";

/// Generate a new one-use code, valid for five minutes. Calling this replaces
/// the previous code; the UI must ask the user to share it deliberately.
pub fn generate(db: &Database) -> Result<(String, i64), AppError> {
    let random = uuid::Uuid::new_v4();
    let number = u32::from_be_bytes(random.as_bytes()[..4].try_into().unwrap()) % 1_000_000;
    let code = format!("{number:06}");
    let expiry = chrono::Utc::now().timestamp() + 300;
    db.set_setting(CODE_KEY, &format!("{code}:{expiry}:0"))?;
    Ok((code, expiry))
}

fn digest(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

pub fn new_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

pub fn store_grant(db: &Database, peer: &str, token: &str) -> Result<(), AppError> {
    db.set_setting(&format!("lan_grant_v1:{peer}"), &digest(token))
}

pub fn restore_grant(
    db: &Database,
    peer: &str,
    token: &str,
    old: Option<&str>,
) -> Result<(), AppError> {
    let conn = lock_conn!(db.conn);
    let key = format!("lan_grant_v1:{peer}");
    match old {
        Some(value) => {
            conn.execute(
                "UPDATE settings SET value=?3 WHERE key=?1 AND value=?2",
                params![key, digest(token), value],
            )?;
        }
        None => {
            conn.execute(
                "DELETE FROM settings WHERE key=?1 AND value=?2",
                params![key, digest(token)],
            )?;
        }
    }
    Ok(())
}

/// A correct code grants a single remote device access. Code consumption and
/// credential creation are atomic and concurrent reuse cannot succeed.
pub fn accept(db: &Database, code: &str, peer: &str) -> Result<String, AppError> {
    accept_inner(db, code, peer, None)
}

pub fn accept_bidirectional(
    db: &Database,
    code: &str,
    peer: &str,
    name: &str,
    host: &str,
    port: u16,
    reverse_token: &str,
) -> Result<String, AppError> {
    if name.is_empty()
        || name.len() > 256
        || port == 0
        || reverse_token.len() != 64
        || !reverse_token.bytes().all(|v| v.is_ascii_hexdigit())
    {
        return Err(AppError::InvalidInput("对方设备信息无效".into()));
    }
    accept_inner(db, code, peer, Some((name, host, port, reverse_token)))
}

fn accept_inner(
    db: &Database,
    code: &str,
    peer: &str,
    reverse: Option<(&str, &str, u16, &str)>,
) -> Result<String, AppError> {
    if peer.is_empty()
        || peer.len() > 128
        || code.len() != 6
        || !code.bytes().all(|c| c.is_ascii_digit())
    {
        return Err(AppError::InvalidInput("配对码无效".into()));
    }
    let mut conn = lock_conn!(db.conn);
    let tx = conn.transaction()?;
    let raw: Option<String> = tx
        .query_row("SELECT value FROM settings WHERE key=?1", [CODE_KEY], |r| {
            r.get(0)
        })
        .optional()?;
    let Some(raw) = raw else {
        return Err(AppError::InvalidInput("配对码无效或已过期".into()));
    };
    let parts: Vec<_> = raw.split(':').collect();
    let expires = parts
        .get(1)
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);
    let attempts = parts
        .get(2)
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(5);
    if expires < chrono::Utc::now().timestamp() || attempts >= 5 {
        tx.execute("DELETE FROM settings WHERE key=?1", [CODE_KEY])?;
        tx.commit()?;
        return Err(AppError::InvalidInput("配对码无效或已过期".into()));
    }
    if parts.first().copied() != Some(code) {
        tx.execute(
            "UPDATE settings SET value=?2 WHERE key=?1",
            params![CODE_KEY, format!("{}:{expires}:{}", parts[0], attempts + 1)],
        )?;
        tx.commit()?;
        return Err(AppError::InvalidInput("配对码无效或已过期".into()));
    }
    let token = new_token();
    tx.execute("DELETE FROM settings WHERE key=?1", [CODE_KEY])?;
    tx.execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![format!("lan_grant_v1:{peer}"), digest(&token)])?;
    if let Some((name, host, port, reverse_token)) = reverse {
        let now = chrono::Utc::now().timestamp();
        tx.execute("INSERT INTO lan_devices (device_id,device_name,host,port,enabled,auto_sync,last_seen_at,created_at) VALUES (?1,?2,?3,?4,1,1,?5,?5) ON CONFLICT(device_id) DO UPDATE SET device_name=excluded.device_name,host=excluded.host,port=excluded.port,enabled=1,last_seen_at=excluded.last_seen_at", params![peer,name,host,port,now])?;
        tx.execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![format!("lan_outgoing_token_v1:{peer}"), reverse_token])?;
    }
    tx.commit()?;
    Ok(token)
}

pub fn authorized(db: &Database, peer: &str, token: &str) -> Result<bool, AppError> {
    if peer.is_empty() || peer.len() > 128 || token.len() != 64 {
        return Ok(false);
    }
    Ok(db
        .get_setting(&format!("lan_grant_v1:{peer}"))?
        .is_some_and(|saved| saved == digest(token)))
}

pub fn revoke(db: &Database, peer: &str) -> Result<(), AppError> {
    db.delete_setting(&format!("lan_grant_v1:{peer}"))
}

pub fn incoming(db: &Database) -> Result<Vec<String>, AppError> {
    let conn = lock_conn!(db.conn);
    let mut stmt = conn.prepare("SELECT substr(key, 14) FROM settings WHERE substr(key, 1, 13) = 'lan_grant_v1:' ORDER BY key")?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}
