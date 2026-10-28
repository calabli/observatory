use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub async fn international_ipv4() -> Result<Value, crate::error::AppError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| crate::error::AppError::Message(format!("无法创建查询请求: {e}")))?;
    let response = client
        .get("https://api.ipify.org?format=json")
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| crate::error::AppError::Message(format!("无法连接 ipify: {e}")))?;
    let body: Value = response
        .json()
        .await
        .map_err(|e| crate::error::AppError::Message(format!("ipify 响应无效: {e}")))?;
    let address = parse_ipv4_response(&body)?;
    Ok(json!({"ip": address.to_string(), "checkedAt": chrono::Utc::now().timestamp_millis()}))
}

pub fn parse_ipv4_response(body: &Value) -> Result<std::net::Ipv4Addr, crate::error::AppError> {
    body.get("ip")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<std::net::Ipv4Addr>().ok())
        .ok_or_else(|| crate::error::AppError::Message("ipify 未返回有效的 IPv4 地址".into()))
}

struct Cache {
    refreshed: Instant,
    interfaces: Vec<netdev::Interface>,
}
/// Cache configuration briefly; refresh counters without repeatedly enumerating routes.
pub fn snapshot(force: bool) -> Value {
    static CACHE: OnceLock<Mutex<Option<Cache>>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if force
        || cache
            .as_ref()
            .is_none_or(|c| c.refreshed.elapsed() >= Duration::from_secs(30))
    {
        // The OS may resolve a local gateway neighbour during interface enumeration.
        *cache = Some(Cache {
            refreshed: Instant::now(),
            interfaces: netdev::get_interfaces(),
        });
    }
    let cached = cache.as_mut().expect("cache initialized");
    for interface in &mut cached.interfaces {
        if interface.update_stats().is_err() {
            interface.stats = None;
        }
    }
    let interfaces = cached.interfaces.clone();
    let rows: Vec<Value> = interfaces
        .into_iter()
        .filter(|i| !i.is_loopback())
        .map(|i| {
            let gateway = i
                .gateway
                .as_ref()
                .map(|g| {
                    g.ipv4
                        .iter()
                        .map(ToString::to_string)
                        .chain(g.ipv6.iter().map(ToString::to_string))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            json!({
                "id": format!("{}:{}", i.index, i.name),
                "name": i.friendly_name.as_deref().unwrap_or(&i.name),
                "systemName": i.name,
                "defaultRoute": i.default,
                "state": i.oper_state.as_str(),
                "ipv4": i.ipv4.iter().map(|ip| ip.addr().to_string()).collect::<Vec<_>>(),
                "ipv6": i.ipv6.iter().map(|ip| ip.addr().to_string()).collect::<Vec<_>>(),
                "gateways": gateway,
                "dns": i.dns_servers.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "received": i.stats.as_ref().map(|s| s.rx_bytes),
                "transmitted": i.stats.as_ref().map(|s| s.tx_bytes),
            })
        })
        .collect();
    json!({"interfaces": rows})
}
