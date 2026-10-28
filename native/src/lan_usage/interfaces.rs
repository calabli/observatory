//! Enumerate IPv4 broadcast interfaces without consulting the default route.

use std::io;
use std::net::Ipv4Addr;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct BroadcastInterface {
    pub address: Ipv4Addr,
    pub broadcast: Ipv4Addr,
}

impl BroadcastInterface {
    fn from_netmask(address: Ipv4Addr, netmask: Ipv4Addr) -> Option<Self> {
        let mask = u32::from(netmask);
        let prefix = mask.leading_ones();
        // /31 and /32 have no broadcast host; reject non-contiguous masks.
        if !(1..=30).contains(&prefix) || mask != u32::MAX << (32 - prefix) {
            return None;
        }
        if address.is_unspecified()
            || address.is_loopback()
            || address.is_multicast()
            || address.is_broadcast()
        {
            return None;
        }
        let ip = u32::from(address);
        if ip & !mask == 0 || ip & !mask == !mask {
            return None;
        }
        Some(Self {
            address,
            broadcast: Ipv4Addr::from(ip | !mask),
        })
    }
}

pub(super) fn broadcast_interfaces() -> io::Result<Vec<BroadcastInterface>> {
    let mut interfaces = platform_interfaces()?;
    interfaces.sort_unstable();
    interfaces.dedup();
    Ok(interfaces)
}

#[cfg(unix)]
fn platform_interfaces() -> io::Result<Vec<BroadcastInterface>> {
    let mut head = std::ptr::null_mut();
    // SAFETY: getifaddrs initializes head; the list stays alive until freeifaddrs.
    // Check each sockaddr for null and AF_INET before casting it.
    unsafe {
        if libc::getifaddrs(&mut head) != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut interfaces = Vec::new();
        let mut cursor = head;
        while let Some(interface) = cursor.as_ref() {
            let flags = interface.ifa_flags;
            if flags & libc::IFF_UP as u32 != 0
                && flags & libc::IFF_BROADCAST as u32 != 0
                && flags & (libc::IFF_LOOPBACK | libc::IFF_POINTOPOINT) as u32 == 0
                && !interface.ifa_addr.is_null()
                && !interface.ifa_netmask.is_null()
                && (*interface.ifa_addr).sa_family as i32 == libc::AF_INET
                && (*interface.ifa_netmask).sa_family as i32 == libc::AF_INET
            {
                let address = &*interface.ifa_addr.cast::<libc::sockaddr_in>();
                let netmask = &*interface.ifa_netmask.cast::<libc::sockaddr_in>();
                if let Some(interface) = BroadcastInterface::from_netmask(
                    Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes()),
                    Ipv4Addr::from(netmask.sin_addr.s_addr.to_ne_bytes()),
                ) {
                    interfaces.push(interface);
                }
            }
            cursor = interface.ifa_next;
        }
        libc::freeifaddrs(head);
        Ok(interfaces)
    }
}

#[cfg(windows)]
fn platform_interfaces() -> io::Result<Vec<BroadcastInterface>> {
    use std::mem::size_of;
    use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_NO_DATA, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
        GAA_FLAG_SKIP_MULTICAST, IF_TYPE_SOFTWARE_LOOPBACK, IF_TYPE_TUNNEL,
        IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

    let mut size = 15_000u32;
    for _ in 0..3 {
        // Typed storage provides the alignment required by Windows structs.
        let count = (size as usize).div_ceil(size_of::<IP_ADAPTER_ADDRESSES_LH>());
        let mut buffer = vec![IP_ADAPTER_ADDRESSES_LH::default(); count];
        let head = buffer.as_mut_ptr();
        size = (count * size_of::<IP_ADAPTER_ADDRESSES_LH>()) as u32;
        // SAFETY: buffer is aligned, initialized and at least size bytes long.
        // Returned nodes are consumed before the buffer is dropped.
        let status = unsafe {
            GetAdaptersAddresses(
                AF_INET as u32,
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
                std::ptr::null(),
                head,
                &mut size,
            )
        };
        if status == ERROR_BUFFER_OVERFLOW {
            continue;
        }
        if status == ERROR_NO_DATA {
            return Ok(Vec::new());
        }
        if status != NO_ERROR {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        let mut interfaces = Vec::new();
        let mut cursor = head;
        // SAFETY: enumeration succeeded; validate address length and family
        // before casting. All pointers remain backed by buffer.
        unsafe {
            while let Some(adapter) = cursor.as_ref() {
                if adapter.OperStatus == IfOperStatusUp
                    && adapter.IfType != IF_TYPE_SOFTWARE_LOOPBACK
                    && adapter.IfType != IF_TYPE_TUNNEL
                {
                    let mut unicast = adapter.FirstUnicastAddress;
                    while let Some(address) = unicast.as_ref() {
                        let socket = address.Address;
                        let prefix = address.OnLinkPrefixLength;
                        if !socket.lpSockaddr.is_null()
                            && socket.iSockaddrLength as usize >= size_of::<SOCKADDR_IN>()
                            && (*socket.lpSockaddr).sa_family == AF_INET
                            && (1..=30).contains(&prefix)
                        {
                            let ip = &*socket.lpSockaddr.cast::<SOCKADDR_IN>();
                            if let Some(interface) = BroadcastInterface::from_netmask(
                                Ipv4Addr::from(ip.sin_addr.S_un.S_addr.to_ne_bytes()),
                                Ipv4Addr::from(u32::MAX << (32 - prefix)),
                            ) {
                                interfaces.push(interface);
                            }
                        }
                        unicast = address.Next;
                    }
                }
                cursor = adapter.Next;
            }
        }
        return Ok(interfaces);
    }
    Err(io::Error::other(
        "Network interfaces changed during enumeration",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_actual_subnet_masks_instead_of_assuming_24() {
        for (ip, mask, expected) in [
            ("198.51.101.187", "255.255.255.0", "198.51.101.255"),
            ("198.51.100.23", "255.255.254.0", "198.51.101.255"),
            ("198.18.192.1", "255.255.240.0", "198.18.207.255"),
            ("198.19.5.9", "255.255.0.0", "198.19.255.255"),
        ] {
            let interface =
                BroadcastInterface::from_netmask(ip.parse().unwrap(), mask.parse().unwrap())
                    .unwrap();
            assert_eq!(interface.address, ip.parse::<Ipv4Addr>().unwrap());
            assert_eq!(interface.broadcast, expected.parse::<Ipv4Addr>().unwrap());
        }
    }

    #[test]
    fn ignores_non_broadcast_addresses_and_invalid_masks() {
        for (ip, mask) in [
            ("127.0.0.1", "255.0.0.0"),
            ("0.0.0.0", "255.255.255.0"),
            ("224.0.0.1", "255.255.255.0"),
            ("255.255.255.255", "255.255.255.0"),
            ("203.0.113.0", "255.255.255.0"),
            ("203.0.113.255", "255.255.255.0"),
            ("203.0.113.1", "0.0.0.0"),
            ("203.0.113.1", "255.0.255.0"),
            ("203.0.113.1", "255.255.255.254"),
            ("203.0.113.1", "255.255.255.255"),
        ] {
            assert!(
                BroadcastInterface::from_netmask(ip.parse().unwrap(), mask.parse().unwrap())
                    .is_none()
            );
        }
    }
}
