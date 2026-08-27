use crate::error::{Error, Result};
use ipnet::IpNet;
use std::net::IpAddr;
use std::str::FromStr;

/// Never blockable. Checked for overlap in either direction, so neither a block
/// inside one of these nor a supernet swallowing one is accepted.
///
/// The Cloudflare entries are load-bearing: `public/client-policy/policy.yaml` reads
/// the client IP from `CF-Connecting-IP` with `failClosed: false`, so a request
/// without that header falls back to the socket address - a Cloudflare edge IP.
/// Blocking one takes the whole site off the internet.
const PROTECTED: &[(&str, &str)] = &[
    ("100.64.0.0/10", "tailnet"),
    ("10.0.0.0/8", "private (cluster)"),
    ("172.16.0.0/12", "private"),
    ("192.168.0.0/16", "private"),
    ("127.0.0.0/8", "loopback"),
    ("169.254.0.0/16", "link-local"),
    ("::1/128", "loopback"),
    ("fc00::/7", "unique local"),
    ("fe80::/10", "link-local"),
    // Cloudflare IPv4 - https://www.cloudflare.com/ips/
    ("173.245.48.0/20", "cloudflare"),
    ("103.21.244.0/22", "cloudflare"),
    ("103.22.200.0/22", "cloudflare"),
    ("103.31.4.0/22", "cloudflare"),
    ("141.101.64.0/18", "cloudflare"),
    ("108.162.192.0/18", "cloudflare"),
    ("190.93.240.0/20", "cloudflare"),
    ("188.114.96.0/20", "cloudflare"),
    ("197.234.240.0/22", "cloudflare"),
    ("198.41.128.0/17", "cloudflare"),
    ("162.158.0.0/15", "cloudflare"),
    ("104.16.0.0/13", "cloudflare"),
    ("104.24.0.0/14", "cloudflare"),
    ("172.64.0.0/13", "cloudflare"),
    ("131.0.72.0/22", "cloudflare"),
    // Cloudflare IPv6
    ("2400:cb00::/32", "cloudflare"),
    ("2606:4700::/32", "cloudflare"),
    ("2803:f800::/32", "cloudflare"),
    ("2405:b500::/32", "cloudflare"),
    ("2405:8100::/32", "cloudflare"),
    ("2a06:98c0::/29", "cloudflare"),
    ("2c0f:f248::/32", "cloudflare"),
];

/// Guards every CIDR entering the blocklist; parses the protected ranges once.
pub struct Allowlist {
    protected: Vec<(IpNet, &'static str)>,
}

impl Default for Allowlist {
    fn default() -> Self {
        Self::new()
    }
}

impl Allowlist {
    pub fn new() -> Self {
        let protected = PROTECTED
            .iter()
            .map(|(raw, why)| {
                let net = raw
                    .parse::<IpNet>()
                    .expect("PROTECTED entries must be valid CIDRs");
                (net, *why)
            })
            .collect();

        Self { protected }
    }

    /// A bare address widens to a single-host prefix, as the UI submits.
    pub fn parse_cidr(input: &str) -> Result<IpNet> {
        let input = input.trim();
        if input.is_empty() {
            return Err(Error::InvalidCidr(
                input.to_string(),
                "empty value".to_string(),
            ));
        }

        if let Ok(net) = IpNet::from_str(input) {
            // 203.0.113.4/24 -> 203.0.113.0/24, so host bits never imply a
            // narrower block than is applied.
            return Ok(net.trunc());
        }

        let addr = IpAddr::from_str(input)
            .map_err(|e| Error::InvalidCidr(input.to_string(), e.to_string()))?;
        let prefix = match addr {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };

        IpNet::new(addr, prefix).map_err(|e| Error::InvalidCidr(input.to_string(), e.to_string()))
    }

    /// Also called from the reconciler, since a WafBlock can be applied from git.
    pub fn check(&self, net: &IpNet) -> Result<()> {
        for (protected, why) in &self.protected {
            if net.contains(protected) || protected.contains(net) {
                return Err(Error::ProtectedRange(
                    net.to_string(),
                    format!("{protected} ({why})"),
                ));
            }
        }

        Ok(())
    }

    pub fn parse_and_check(&self, input: &str) -> Result<IpNet> {
        let net = Self::parse_cidr(input)?;
        self.check(&net)?;
        Ok(net)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_entries_all_parse() {
        // new() panics on a malformed entry; fail here, not in a crash loop.
        let _ = Allowlist::new();
    }

    #[test]
    fn bare_addresses_widen_to_host_prefixes() {
        assert_eq!(
            Allowlist::parse_cidr("203.0.113.4").unwrap().to_string(),
            "203.0.113.4/32"
        );
        assert_eq!(
            Allowlist::parse_cidr("2402:21a0::1").unwrap().to_string(),
            "2402:21a0::1/128"
        );
    }

    #[test]
    fn host_bits_are_truncated() {
        assert_eq!(
            Allowlist::parse_cidr("203.0.113.4/24").unwrap().to_string(),
            "203.0.113.0/24"
        );
    }

    #[test]
    fn ordinary_public_addresses_are_allowed() {
        let allowlist = Allowlist::new();
        for ip in ["203.0.113.4", "209.87.162.138", "210.56.150.188"] {
            allowlist
                .parse_and_check(ip)
                .unwrap_or_else(|e| panic!("{ip} should be blockable: {e}"));
        }
    }

    #[test]
    fn cloudflare_edge_is_refused() {
        let err = Allowlist::new().parse_and_check("104.16.5.5").unwrap_err();
        assert!(matches!(err, Error::ProtectedRange(..)), "got {err}");
    }

    #[test]
    fn tailnet_is_refused() {
        assert!(Allowlist::new().parse_and_check("100.64.1.2").is_err());
    }

    #[test]
    fn supernet_swallowing_a_protected_range_is_refused() {
        // A wider network must not slip through for being the larger one.
        let allowlist = Allowlist::new();
        assert!(allowlist.parse_and_check("0.0.0.0/0").is_err());
        assert!(allowlist.parse_and_check("104.0.0.0/8").is_err());
    }

    #[test]
    fn ipv6_cloudflare_is_refused() {
        assert!(Allowlist::new().parse_and_check("2606:4700::1111").is_err());
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(Allowlist::parse_cidr("").is_err());
        assert!(Allowlist::parse_cidr("not-an-ip").is_err());
        assert!(Allowlist::parse_cidr("203.0.113.4/99").is_err());
    }
}
