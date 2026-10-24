//! Shared interpretation of model connection settings.
//!
//! Credential requirements and transport permission are different decisions.

/// The CLI's locality rule for optional authentication on window-owned routers.
/// This does not authorize HTTP transport or resolve a hostname through DNS.
#[must_use]
pub fn router_base_is_local(base_url: &str) -> bool {
    let Some(host) = url::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
    else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return ip_is_local(ip);
    }
    let name = host.trim_end_matches('.');
    !name.contains('.')
        || [".localhost", ".local", ".internal", ".home.arpa", ".lan"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

fn ip_is_local(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => {
            let [first, second, ..] = ip.octets();
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                // 100.64.0.0/10, the shared space a carrier NAT or a tailnet hands out.
                || (first == 100 && (second & 0xc0) == 64)
        }
        std::net::IpAddr::V6(ip) => {
            let first = ip.segments()[0];
            ip.to_ipv4_mapped()
                .is_some_and(|mapped| ip_is_local(mapped.into()))
                || ip.is_loopback()
                || ip.is_unspecified()
                // fc00::/7 unique local, fe80::/10 link-local.
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
        }
    }
}
