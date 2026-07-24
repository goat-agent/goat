use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

pub fn is_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map_or_else(|| is_blocked_v6(v6), is_blocked_v4),
    }
}

fn is_blocked_v4(v4: Ipv4Addr) -> bool {
    let [a, b, ..] = v4.octets();
    v4.is_private()
        || v4.is_loopback()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_documentation()
        || v4.is_unspecified()
        || v4.is_multicast()
        || a == 0
        || a >= 240
        || (a == 100 && (64..=127).contains(&b))
        || (a == 198 && (18..=19).contains(&b))
        || (a == 192 && b == 0 && v4.octets()[2] == 0)
}

fn is_blocked_v6(v6: Ipv6Addr) -> bool {
    if let Some(v4) = embedded_v4(v6) {
        return is_blocked_v4(v4);
    }
    v6.is_loopback()
        || v6.is_unspecified()
        || v6.is_multicast()
        || v6.is_unique_local()
        || v6.is_unicast_link_local()
}

fn embedded_v4(v6: Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = v6.segments();
    if segments[0] == 0x2002 {
        return Some(Ipv4Addr::new(
            (segments[1] >> 8) as u8,
            segments[1] as u8,
            (segments[2] >> 8) as u8,
            segments[2] as u8,
        ));
    }
    if segments[0] == 0x2001 && segments[1] == 0x0000 {
        let teredo = (u32::from(segments[6]) << 16) | u32::from(segments[7]);
        return Some(Ipv4Addr::from(!teredo));
    }
    if segments[0] == 0x0064 && segments[1] == 0xff9b && segments[2..6] == [0, 0, 0, 0] {
        return Some(Ipv4Addr::new(
            (segments[6] >> 8) as u8,
            segments[6] as u8,
            (segments[7] >> 8) as u8,
            segments[7] as u8,
        ));
    }
    if segments[..6] == [0, 0, 0, 0, 0, 0] {
        return v6.to_ipv4().filter(|v4| !v4.is_unspecified());
    }
    None
}

pub struct GuardedResolver;

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let resolved = tokio::net::lookup_host((host.as_str(), 0)).await?;
            let allowed: Vec<SocketAddr> = resolved.filter(|addr| !is_blocked(addr.ip())).collect();
            if allowed.is_empty() {
                let err: Box<dyn std::error::Error + Send + Sync> =
                    "host resolves to a blocked or private address".into();
                return Err(err);
            }
            let addrs: Addrs = Box::new(allowed.into_iter());
            Ok(addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::is_blocked;
    use std::net::IpAddr;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn blocks_private_and_local() {
        assert!(is_blocked(ip("127.0.0.1")));
        assert!(is_blocked(ip("10.0.0.5")));
        assert!(is_blocked(ip("192.168.1.1")));
        assert!(is_blocked(ip("169.254.1.1")));
        assert!(is_blocked(ip("100.64.0.1")));
        assert!(is_blocked(ip("198.18.0.1")));
        assert!(is_blocked(ip("0.0.0.0")));
        assert!(is_blocked(ip("::1")));
        assert!(is_blocked(ip("fc00::1")));
        assert!(is_blocked(ip("fe80::1")));
        assert!(is_blocked(ip("::ffff:127.0.0.1")));
    }

    #[test]
    fn blocks_ipv6_transition_ranges() {
        assert!(is_blocked(ip("2002:7f00:1::")));
        assert!(is_blocked(ip("2002:a00:5::")));
        assert!(is_blocked(ip("2002:c0a8:101::")));
        assert!(is_blocked(ip("64:ff9b::7f00:1")));
        assert!(is_blocked(ip("64:ff9b::a00:5")));
    }

    #[test]
    fn allows_public_6to4() {
        assert!(!is_blocked(ip("2002:808:808::")));
    }

    #[test]
    fn allows_public() {
        assert!(!is_blocked(ip("8.8.8.8")));
        assert!(!is_blocked(ip("1.1.1.1")));
        assert!(!is_blocked(ip("93.184.216.34")));
        assert!(!is_blocked(ip("2606:2800:220:1::1")));
    }
}
