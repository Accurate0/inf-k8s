use crate::metrics::Metrics;
use hickory_resolver::TokioResolver;
use hickory_resolver::proto::rr::RData;
use moka::future::Cache;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

const LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);
const CACHE_TTL: Duration = Duration::from_secs(3600);
const CACHE_LIMIT: u64 = 4096;

const ORIGIN_V4: &str = "origin.asn.cymru.com.";
const ORIGIN_V6: &str = "origin6.asn.cymru.com.";

#[derive(Debug, Clone, Default)]
pub struct Enrichment {
    pub hostname: Option<String>,
    pub asn: Option<String>,
    pub org: Option<String>,
    pub prefix: Option<String>,
    pub country: Option<String>,
}

impl Enrichment {
    pub fn is_empty(&self) -> bool {
        self.hostname.is_none() && self.asn.is_none()
    }

    pub fn as_number(&self) -> Option<String> {
        self.asn.as_ref().map(|asn| format!("AS{asn}"))
    }
}

#[derive(Clone)]
pub struct Enricher {
    inner: Arc<EnricherInner>,
}

struct EnricherInner {
    resolver: TokioResolver,
    cache: Cache<IpAddr, Arc<Enrichment>>,
}

impl Enricher {
    pub fn new() -> Option<Self> {
        let resolver = match TokioResolver::builder_tokio() {
            Ok(builder) => builder.build(),
            Err(e) => {
                tracing::warn!("no system resolver, ip enrichment is disabled: {e}");
                return None;
            }
        };

        let resolver = match resolver {
            Ok(resolver) => resolver,
            Err(e) => {
                tracing::warn!("building the resolver failed, ip enrichment is disabled: {e}");
                return None;
            }
        };

        Some(Self {
            inner: Arc::new(EnricherInner {
                resolver,
                cache: Cache::builder()
                    .max_capacity(CACHE_LIMIT)
                    .time_to_live(CACHE_TTL)
                    .build(),
            }),
        })
    }

    pub async fn lookup(&self, addr: IpAddr) -> Arc<Enrichment> {
        self.inner
            .cache
            .get_with(addr, async move {
                let (hostname, origin) = tokio::join!(self.hostname(addr), self.origin(addr));

                Arc::new(Enrichment { hostname, ..origin })
            })
            .await
    }

    async fn hostname(&self, addr: IpAddr) -> Option<String> {
        let name = hickory_resolver::proto::rr::Name::from(addr);
        let records = self.query(&name.to_string(), "ptr").await?;

        records.into_iter().find_map(|data| match data {
            RData::PTR(ptr) => Some(ptr.to_string().trim_end_matches('.').to_string()),
            _ => None,
        })
    }

    async fn origin(&self, addr: IpAddr) -> Enrichment {
        let Some(records) = self.query(&Self::origin_name(addr), "asn").await else {
            return Enrichment::default();
        };

        let Some(answer) = records.into_iter().find_map(Self::txt) else {
            return Enrichment::default();
        };

        let mut enrichment = Self::parse_origin(&answer);

        if let Some(asn) = &enrichment.asn {
            enrichment.org = self.org(asn).await;
        }

        enrichment
    }

    async fn org(&self, asn: &str) -> Option<String> {
        let records = self
            .query(&format!("AS{asn}.asn.cymru.com."), "asn-org")
            .await?;
        let answer = records.into_iter().find_map(Self::txt)?;

        Self::field(&answer, 4)
    }

    async fn query(&self, name: &str, kind: &'static str) -> Option<Vec<RData>> {
        let lookup = match kind {
            "ptr" => {
                tokio::time::timeout(LOOKUP_TIMEOUT, self.inner.resolver.reverse_lookup(name)).await
            }
            _ => tokio::time::timeout(LOOKUP_TIMEOUT, self.inner.resolver.txt_lookup(name)).await,
        };

        match lookup {
            Ok(Ok(lookup)) => {
                Metrics::record_enrichment_lookup(kind, "success");
                Some(
                    lookup
                        .answers()
                        .iter()
                        .map(|record| record.data.clone())
                        .collect(),
                )
            }
            Ok(Err(e)) => {
                tracing::debug!("{kind} lookup for {name} failed: {e}");
                Metrics::record_enrichment_lookup(kind, "error");
                None
            }
            Err(_) => {
                tracing::debug!("{kind} lookup for {name} timed out");
                Metrics::record_enrichment_lookup(kind, "timeout");
                None
            }
        }
    }

    fn txt(data: RData) -> Option<String> {
        match data {
            RData::TXT(txt) => Some(txt.to_string()),
            _ => None,
        }
    }

    fn origin_name(addr: IpAddr) -> String {
        match addr {
            IpAddr::V4(v4) => {
                let octets = v4.octets();
                format!(
                    "{}.{}.{}.{}.{ORIGIN_V4}",
                    octets[3], octets[2], octets[1], octets[0]
                )
            }
            IpAddr::V6(v6) => {
                let nibbles: Vec<String> = v6
                    .octets()
                    .iter()
                    .flat_map(|byte| [byte >> 4, byte & 0x0f])
                    .rev()
                    .map(|nibble| format!("{nibble:x}"))
                    .collect();

                format!("{}.{ORIGIN_V6}", nibbles.join("."))
            }
        }
    }

    fn parse_origin(answer: &str) -> Enrichment {
        Enrichment {
            hostname: None,
            asn: Self::field(answer, 0).and_then(|asn| {
                asn.split_whitespace()
                    .next()
                    .filter(|first| first.chars().all(|c| c.is_ascii_digit()))
                    .map(str::to_string)
            }),
            prefix: Self::field(answer, 1),
            country: Self::field(answer, 2),
            org: None,
        }
    }

    fn field(answer: &str, index: usize) -> Option<String> {
        answer
            .split('|')
            .nth(index)
            .map(str::trim)
            .filter(|field| !field.is_empty())
            .map(str::to_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_name_reverses_ipv4_octets() {
        assert_eq!(
            Enricher::origin_name("216.90.108.31".parse().unwrap()),
            format!("31.108.90.216.{ORIGIN_V4}")
        );
    }

    #[test]
    fn origin_name_nibble_reverses_ipv6() {
        let name = Enricher::origin_name("2001:db8::1".parse().unwrap());

        assert!(name.ends_with(ORIGIN_V6));
        assert!(name.starts_with("1.0.0.0."));
        assert_eq!(
            name.trim_end_matches(&format!(".{ORIGIN_V6}"))
                .split('.')
                .count(),
            32
        );
    }

    #[test]
    fn an_origin_answer_yields_asn_prefix_and_country() {
        let parsed = Enricher::parse_origin("23028 | 216.90.108.0/24 | US | arin | 1998-09-25");

        assert_eq!(parsed.asn.as_deref(), Some("23028"));
        assert_eq!(parsed.prefix.as_deref(), Some("216.90.108.0/24"));
        assert_eq!(parsed.country.as_deref(), Some("US"));
    }

    #[test]
    fn an_origin_answer_with_several_asns_takes_the_first() {
        let parsed =
            Enricher::parse_origin("23028 8103 | 216.90.108.0/24 | US | arin | 1998-09-25");

        assert_eq!(parsed.asn.as_deref(), Some("23028"));
    }

    #[test]
    fn a_junk_origin_answer_yields_nothing() {
        let parsed = Enricher::parse_origin("no route to host");

        assert!(parsed.asn.is_none());
        assert!(parsed.prefix.is_none());
    }

    #[test]
    fn the_org_name_is_the_fifth_field() {
        let answer = "23028 | US | arin | 2002-01-04 | TEAM-CYMRU, US";

        assert_eq!(
            Enricher::field(answer, 4).as_deref(),
            Some("TEAM-CYMRU, US")
        );
    }
}
