//! DNS answers and URL checks that keep the model and MCP endpoints off private networks.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// Resolves names through the system resolver and drops every non-public answer; a name left with
/// no address fails, so a DNS answer that changes after configuration is filtered as well.
#[derive(Debug, Clone, Copy, Default)]
pub struct SafeDns {
    /// Local tests only: allow private addresses through instead of filtering them.
    pub allow_private: bool,
}

impl Resolve for SafeDns {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        let allow_private = self.allow_private;
        Box::pin(async move {
            let resolved = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?;
            let addresses: Vec<SocketAddr> = resolved
                .filter(|address| allow_private || is_public_ip(address.ip()))
                .collect();
            if addresses.is_empty() {
                let error = io::Error::new(io::ErrorKind::NotFound, "不允许访问内网地址");
                return Err(Box::new(error) as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

pub fn is_public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => is_public_v6(ip),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    if ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_multicast()
    {
        return false;
    }
    let octets = ip.octets();
    // 100.64.0.0/10, shared address space (carrier-grade NAT).
    !(octets[0] == 100 && (64..=127).contains(&octets[1]))
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    // ::ffff:a.b.c.d and ::a.b.c.d reach the same IPv4 host. Classify the embedded address.
    let octets = ip.octets();
    let suffix = u32::from_be_bytes([octets[12], octets[13], octets[14], octets[15]]);
    if segments[0] == 0
        && segments[1] == 0
        && segments[2] == 0
        && segments[3] == 0
        && segments[4] == 0
        && (segments[5] == 0xffff || (segments[5] == 0 && suffix > 1))
    {
        return is_public_v4(Ipv4Addr::from(suffix));
    }
    if ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_unicast_link_local()
        || ip.is_unique_local()
    {
        return false;
    }
    // fec0::/10, deprecated site-local addresses.
    let segments = ip.segments();
    (segments[0] & 0xffc0) != 0xfec0
}

/// True when `url` names an http(s) endpoint whose host resolves to at least one public address.
pub async fn is_public_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return false;
    }
    match parsed.host() {
        Some(url::Host::Ipv4(ip)) => is_public_ip(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => is_public_ip(IpAddr::V6(ip)),
        Some(url::Host::Domain(domain)) => {
            let port = parsed.port_or_known_default().unwrap_or(443);
            match tokio::net::lookup_host((domain, port)).await {
                Ok(addresses) => addresses
                    .into_iter()
                    .any(|address| is_public_ip(address.ip())),
                Err(_) => false,
            }
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_public_addresses() {
        for text in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.1.1",
            "0.0.0.0",
            "224.0.0.1",
            "100.64.0.1",
            "100.127.255.254",
            "::1",
            "::",
            "fc00::1",
            "fdff::1",
            "fe80::1",
            "ff02::1",
            "fec0::1",
            "::ffff:127.0.0.1",
            "::ffff:10.1.2.3",
            "::ffff:169.254.169.254",
            "::127.0.0.1",
            "::10.1.2.3",
        ] {
            let address: IpAddr = text.parse().expect(text);
            assert!(!is_public_ip(address), "{text}");
        }
    }

    #[test]
    fn allows_public_addresses() {
        for text in ["8.8.8.8", "2606:4700:4700::1111", "100.128.0.1"] {
            let address: IpAddr = text.parse().expect(text);
            assert!(is_public_ip(address), "{text}");
        }
    }

    #[tokio::test]
    async fn private_urls_are_rejected_without_dns() {
        assert!(!is_public_url("http://127.0.0.1:12345").await);
        assert!(!is_public_url("http://[::1]:12345").await);
        assert!(!is_public_url("http://10.1.2.3").await);
        assert!(!is_public_url("http://169.254.169.254").await);
        assert!(!is_public_url("http://[::ffff:127.0.0.1]:12345").await);
        assert!(!is_public_url("http://[::ffff:10.1.2.3]:12345").await);
        assert!(!is_public_url("npx").await);
        assert!(!is_public_url("ftp://example.com").await);
    }
}
