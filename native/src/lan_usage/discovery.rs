//! Probe every broadcast-capable IPv4 interface independently. A VPN/TUN default
//! route must not decide which LAN is searched or which source address is used.

use super::interfaces::{broadcast_interfaces, BroadcastInterface};
use super::{DeviceInfo, DiscoveredDevice, DISCOVERY_MAGIC};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::Instant;

pub async fn discover(port: u16, timeout_ms: u64) -> Result<Vec<DiscoveredDevice>, String> {
    let interfaces = tokio::task::spawn_blocking(broadcast_interfaces)
        .await
        .map_err(|e| format!("读取局域网网卡失败: {e}"))?;
    let interfaces = match interfaces {
        Ok(interfaces) => interfaces,
        Err(error) => {
            log::warn!("读取局域网网卡失败，尝试受限广播: {error}");
            Vec::new()
        }
    };
    let probes = probe_targets(&interfaces);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.clamp(200, 5000));
    let responses = futures::future::join_all(
        probes
            .into_iter()
            .map(|(address, targets)| probe_interface(address, targets, port, deadline)),
    )
    .await;
    merge_responses(responses)
}

fn probe_targets(interfaces: &[BroadcastInterface]) -> Vec<(Ipv4Addr, Vec<Ipv4Addr>)> {
    if interfaces.is_empty() {
        return vec![(Ipv4Addr::UNSPECIFIED, vec![Ipv4Addr::BROADCAST])];
    }
    interfaces
        .iter()
        .map(|interface| {
            (
                interface.address,
                vec![interface.broadcast, Ipv4Addr::BROADCAST],
            )
        })
        .collect()
}

async fn probe_interface(
    address: Ipv4Addr,
    targets: Vec<Ipv4Addr>,
    port: u16,
    deadline: Instant,
) -> Result<Vec<DiscoveredDevice>, String> {
    let socket = UdpSocket::bind(SocketAddrV4::new(address, 0))
        .await
        .map_err(|e| format!("打开发现端口 {address} 失败: {e}"))?;
    socket
        .set_broadcast(true)
        .map_err(|e| format!("开启广播失败: {e}"))?;
    let probe = format!("{DISCOVERY_MAGIC}\n");
    for target in targets {
        if let Err(error) = socket
            .send_to(probe.as_bytes(), SocketAddrV4::new(target, port))
            .await
        {
            log::debug!("局域网探测 {address} -> {target}:{port} 失败: {error}");
        }
    }

    let mut buf = [0u8; 2048];
    let mut found = Vec::new();
    loop {
        let Ok(Ok((len, peer))) =
            tokio::time::timeout_at(deadline, socket.recv_from(&mut buf)).await
        else {
            break;
        };
        let Ok(info) = serde_json::from_slice::<DeviceInfo>(&buf[..len]) else {
            continue;
        };
        if info.device_id.trim().is_empty() {
            continue;
        }
        found.push(DiscoveredDevice {
            device_id: info.device_id,
            device_name: info.device_name,
            app_version: info.app_version,
            host: peer.ip().to_string(),
            port,
        });
    }
    Ok(found)
}

fn merge_responses(
    responses: Vec<Result<Vec<DiscoveredDevice>, String>>,
) -> Result<Vec<DiscoveredDevice>, String> {
    let mut found = HashMap::new();
    let mut opened = false;
    let mut errors = Vec::new();
    for response in responses {
        match response {
            Ok(devices) => {
                opened = true;
                for device in devices {
                    found.entry(device.device_id.clone()).or_insert(device);
                }
            }
            Err(error) => {
                log::debug!("局域网网卡探测失败: {error}");
                errors.push(error);
            }
        }
    }
    if !opened {
        return Err(errors.join("; "));
    }
    let mut devices: Vec<DiscoveredDevice> = found.into_values().collect();
    devices.sort_by(|a, b| {
        a.device_name
            .to_lowercase()
            .cmp(&b.device_name.to_lowercase())
            .then_with(|| a.device_id.cmp(&b.device_id))
    });
    Ok(devices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vpn_does_not_replace_wifi_or_ethernet_probe_sources() {
        let interfaces = [
            BroadcastInterface {
                address: "198.18.0.1".parse().unwrap(),
                broadcast: "198.18.0.3".parse().unwrap(),
            },
            BroadcastInterface {
                address: "198.51.101.187".parse().unwrap(),
                broadcast: "198.51.101.255".parse().unwrap(),
            },
            BroadcastInterface {
                address: "198.18.2.7".parse().unwrap(),
                broadcast: "198.18.255.255".parse().unwrap(),
            },
        ];
        let probes = probe_targets(&interfaces);
        assert_eq!(probes.len(), 3);
        for (interface, (source, targets)) in interfaces.iter().zip(probes) {
            assert_eq!(source, interface.address);
            assert!(targets.contains(&interface.broadcast));
            assert!(targets.contains(&Ipv4Addr::BROADCAST));
        }
        assert_eq!(
            probe_targets(&[]),
            vec![(Ipv4Addr::UNSPECIFIED, vec![Ipv4Addr::BROADCAST])]
        );
    }

    #[tokio::test]
    async fn receives_only_valid_identity_packets_using_the_bound_interface() {
        let responder = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = responder.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let mut buf = [0u8; 512];
            let (len, peer) = responder.recv_from(&mut buf).await.unwrap();
            assert_eq!(peer.ip(), Ipv4Addr::LOCALHOST);
            assert_eq!(&buf[..len], format!("{DISCOVERY_MAGIC}\n").as_bytes());
            for payload in [b"not-json".as_slice(), br#"{"deviceId":" ","deviceName":"Invalid","appVersion":"test","platform":"macos","startedAt":0}"#, br#"{"deviceId":"mac","deviceName":"Mac","appVersion":"test","platform":"macos","startedAt":0}"#] {
                responder.send_to(payload, peer).await.unwrap();
            }
        });
        let found = probe_interface(
            Ipv4Addr::LOCALHOST,
            vec![Ipv4Addr::LOCALHOST],
            port,
            Instant::now() + Duration::from_millis(300),
        )
        .await
        .unwrap();
        server.await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].device_id, "mac");
        assert_eq!(found[0].host, "127.0.0.1");
        assert_eq!(found[0].port, port);
    }

    #[test]
    fn deduplicates_peers_across_interfaces_and_tolerates_one_failed_interface() {
        let device = |id: &str, name: &str| DiscoveredDevice {
            device_id: id.into(),
            device_name: name.into(),
            app_version: "test".into(),
            host: "203.0.113.23".into(),
            port: 17890,
        };
        let found = merge_responses(vec![
            Err("disconnected".into()),
            Ok(vec![device("b", "Zulu"), device("a", "Mac")]),
            Ok(vec![device("a", "Mac")]),
        ])
        .unwrap();
        assert_eq!(
            found
                .iter()
                .map(|d| d.device_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert!(merge_responses(vec![Err("no socket".into())]).is_err());
        assert!(merge_responses(vec![Ok(vec![])]).unwrap().is_empty());
    }
}
