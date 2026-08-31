use crate::config::{AllowlistSource, Config, SourceFormat, Span};
use crate::error::{Error, Result};
use crate::metrics::Metrics;
use ipnet::IpNet;
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;

const EXTRA_ENV: &str = "WAF_MANAGER_ALLOWLIST";

pub const MANUAL_SOURCE: &str = "manual";

type Entries = Vec<(IpNet, String)>;

#[derive(Clone)]
pub struct Allowlist {
    inner: Arc<AllowlistInner>,
}

struct AllowlistInner {
    client: reqwest::Client,
    sources: Vec<AllowlistSource>,
    base: Entries,
    manual: RwLock<Entries>,
    fetched: RwLock<BTreeMap<String, Entries>>,
    current: RwLock<Arc<Entries>>,
    ready: tokio::sync::watch::Sender<bool>,
}

impl Allowlist {
    pub fn new(base: Entries, sources: Vec<AllowlistSource>) -> Self {
        let ready = tokio::sync::watch::Sender::new(sources.is_empty());

        Self {
            inner: Arc::new(AllowlistInner {
                ready,
                client: reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(20))
                    .user_agent("waf-manager")
                    .build()
                    .unwrap_or_default(),
                sources,
                current: RwLock::new(Arc::new(base.clone())),
                base,
                manual: RwLock::new(Vec::new()),
                fetched: RwLock::new(BTreeMap::new()),
            }),
        }
    }

    pub fn from_config(config: &Config) -> Self {
        let mut base = Self::parse_list(&config.never_block.join(" "), "never_block");

        match std::env::var(EXTRA_ENV) {
            Ok(raw) => base.extend(Self::parse_list(&raw, EXTRA_ENV)),
            Err(_) => tracing::info!("{EXTRA_ENV} unset"),
        }

        Self::new(base, config.allowlist_sources.clone())
    }

    pub fn parse_list(raw: &str, source: &str) -> Entries {
        raw.split([',', ' ', '\t', '\n', '\r'])
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .filter_map(|entry| match Self::parse_cidr(entry) {
                Ok(net) => Some((net, source.to_string())),
                Err(e) => {
                    tracing::error!("ignoring unparsable {source} entry {entry:?}: {e}");
                    None
                }
            })
            .collect()
    }

    pub async fn entries(&self) -> Arc<Entries> {
        self.inner.current.read().await.clone()
    }

    pub async fn check(&self, net: &IpNet) -> Result<()> {
        match Self::overlap(&self.entries().await, net) {
            Some(why) => Err(Error::ProtectedRange(net.to_string(), why)),
            None => Ok(()),
        }
    }

    pub fn overlap(entries: &[(IpNet, String)], net: &IpNet) -> Option<String> {
        Self::matching(entries, net).map(|(protected, why)| format!("{protected} ({why})"))
    }

    pub fn matching<'a>(
        entries: &'a [(IpNet, String)],
        net: &IpNet,
    ) -> Option<&'a (IpNet, String)> {
        entries
            .iter()
            .find(|(protected, _)| net.contains(protected) || protected.contains(net))
    }

    pub async fn parse_and_check(&self, input: &str) -> Result<IpNet> {
        let net = Self::parse_cidr(input)?;
        self.check(&net).await?;
        Ok(net)
    }

    pub async fn refresh(&self) -> Vec<String> {
        let mut failed = Vec::new();

        for source in &self.inner.sources {
            match self.fetch(source).await {
                Ok(entries) => {
                    tracing::info!(
                        source = source.name,
                        entries = entries.len(),
                        "refreshed allowlist source"
                    );

                    Metrics::record_allowlist_refresh(&source.name, "success");
                    Metrics::set_allowlist_entries(&source.name, entries.len());
                    self.inner
                        .fetched
                        .write()
                        .await
                        .insert(source.name.clone(), entries);
                }
                Err(e) => {
                    let held = self
                        .inner
                        .fetched
                        .read()
                        .await
                        .get(&source.name)
                        .map(Vec::len)
                        .unwrap_or(0);

                    tracing::warn!(
                        source = source.name,
                        held,
                        "allowlist refresh failed, keeping previous entries: {e}"
                    );
                    Metrics::record_allowlist_refresh(&source.name, "error");
                    failed.push(source.name.clone());
                }
            }
        }

        self.rebuild().await;
        failed
    }

    pub fn ready(&self) -> bool {
        *self.inner.ready.borrow()
    }

    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<bool> {
        self.inner.ready.subscribe()
    }

    pub async fn run(self, interval: Span) {
        let mut ticker = tokio::time::interval(interval.0);

        loop {
            ticker.tick().await;

            let failed = self.refresh().await;

            if failed.is_empty() {
                self.inner.ready.send_replace(true);
                continue;
            }

            if !self.ready() {
                tracing::warn!(
                    "allowlist sources {failed:?} have never loaded; \
                     automated blocking stays disabled until they do"
                );
            }
        }
    }

    pub async fn set_manual(&self, entries: Entries) {
        Metrics::set_allowlist_entries(MANUAL_SOURCE, entries.len());
        *self.inner.manual.write().await = entries;
        self.rebuild().await;
    }

    async fn rebuild(&self) {
        let mut entries = self.inner.base.clone();
        entries.extend(self.inner.manual.read().await.iter().cloned());
        entries.extend(self.inner.fetched.read().await.values().flatten().cloned());

        Metrics::set_allowlist_entries("total", entries.len());
        *self.inner.current.write().await = Arc::new(entries);
    }

    async fn fetch(&self, source: &AllowlistSource) -> Result<Entries> {
        let body: serde_json::Value = self
            .inner
            .client
            .get(&source.url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let raw = match source.format {
            SourceFormat::GithubMeta => Self::github_meta(&body, &source.fields),
            SourceFormat::CloudflareIps => Self::cloudflare_ips(&body),
        };

        if raw.is_empty() {
            return Err(Error::Loki(format!(
                "allowlist source {} returned no CIDRs",
                source.name
            )));
        }

        Ok(Self::parse_list(&raw.join(" "), &source.name))
    }

    fn github_meta(body: &serde_json::Value, fields: &[String]) -> Vec<String> {
        fields
            .iter()
            .filter_map(|field| body.get(field)?.as_array())
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    }

    fn cloudflare_ips(body: &serde_json::Value) -> Vec<String> {
        let Some(result) = body.get("result") else {
            return Vec::new();
        };

        ["ipv4_cidrs", "ipv6_cidrs"]
            .iter()
            .filter_map(|key| result.get(*key)?.as_array())
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    }

    pub fn parse_cidr(input: &str) -> Result<IpNet> {
        let input = input.trim();
        if input.is_empty() {
            return Err(Error::InvalidCidr(
                input.to_string(),
                "empty value".to_string(),
            ));
        }

        if let Ok(net) = IpNet::from_str(input) {
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
}

#[derive(Debug, Clone)]
pub struct ManualEntry {
    pub cidr: String,
    pub note: Option<String>,
    pub created_by: String,
    pub at: chrono::DateTime<chrono::Utc>,
}

impl ManualEntry {
    pub fn host(&self) -> Option<&str> {
        self.cidr
            .strip_suffix("/32")
            .or_else(|| self.cidr.strip_suffix("/128"))
    }
}

#[derive(Clone)]
pub struct ManualAllowlist {
    inner: Arc<ManualAllowlistInner>,
}

struct ManualAllowlistInner {
    pool: sqlx::postgres::PgPool,
    allowlist: Allowlist,
}

impl ManualAllowlist {
    pub fn new(pool: sqlx::postgres::PgPool, allowlist: Allowlist) -> Self {
        Self {
            inner: Arc::new(ManualAllowlistInner { pool, allowlist }),
        }
    }

    pub async fn list(&self) -> Result<Vec<ManualEntry>> {
        let rows = sqlx::query!("select cidr, note, created_by, at from allowlist order by cidr")
            .fetch_all(&self.inner.pool)
            .await?;

        Ok(rows
            .into_iter()
            .map(|row| ManualEntry {
                cidr: row.cidr,
                note: row.note,
                created_by: row.created_by,
                at: row.at,
            })
            .collect())
    }

    pub async fn add(&self, net: &IpNet, note: Option<&str>, created_by: &str) -> Result<()> {
        sqlx::query!(
            "insert into allowlist (cidr, note, created_by, at) values ($1, $2, $3, $4)
             on conflict (cidr) do update
             set note = excluded.note, created_by = excluded.created_by, at = excluded.at",
            net.to_string(),
            note,
            created_by,
            chrono::Utc::now(),
        )
        .execute(&self.inner.pool)
        .await?;

        self.reload().await
    }

    pub async fn remove(&self, cidr: &str) -> Result<()> {
        sqlx::query!("delete from allowlist where cidr = $1", cidr)
            .execute(&self.inner.pool)
            .await?;

        self.reload().await
    }

    pub async fn reload(&self) -> Result<()> {
        let entries = self
            .list()
            .await?
            .into_iter()
            .filter_map(|entry| match Allowlist::parse_cidr(&entry.cidr) {
                Ok(net) => Some((net, MANUAL_SOURCE.to_string())),
                Err(e) => {
                    tracing::error!(
                        "ignoring unparsable manual allowlist entry {:?}: {e}",
                        entry.cidr
                    );
                    None
                }
            })
            .collect();

        self.inner.allowlist.set_manual(entries).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowlist(raw: &str) -> Allowlist {
        Allowlist::new(Allowlist::parse_list(raw, "test"), Vec::new())
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
    fn garbage_is_rejected() {
        assert!(Allowlist::parse_cidr("").is_err());
        assert!(Allowlist::parse_cidr("not-an-ip").is_err());
        assert!(Allowlist::parse_cidr("203.0.113.4/99").is_err());
    }

    #[tokio::test]
    async fn allowlisted_entries_are_refused() {
        let allowlist = allowlist("203.0.113.4, 198.51.100.0/24\n2001:db8::1");

        assert!(allowlist.parse_and_check("203.0.113.4").await.is_err());
        assert!(allowlist.parse_and_check("198.51.100.7").await.is_err());
        assert!(allowlist.parse_and_check("2001:db8::1").await.is_err());
        assert!(allowlist.parse_and_check("203.0.113.5").await.is_ok());
    }

    #[tokio::test]
    async fn a_supernet_swallowing_a_protected_range_is_refused() {
        let allowlist = allowlist("104.16.0.0/13");

        assert!(allowlist.parse_and_check("0.0.0.0/0").await.is_err());
        assert!(allowlist.parse_and_check("104.0.0.0/8").await.is_err());
    }

    #[tokio::test]
    async fn a_malformed_entry_is_skipped_not_fatal() {
        let allowlist = allowlist("203.0.113.4, not-an-ip, 198.51.100.0/24");

        assert!(allowlist.parse_and_check("203.0.113.4").await.is_err());
        assert!(allowlist.parse_and_check("198.51.100.7").await.is_err());
    }

    #[tokio::test]
    async fn an_empty_allowlist_blocks_nothing_from_being_blocked() {
        let allowlist = allowlist("  ,, \n ");

        assert!(allowlist.parse_and_check("203.0.113.4").await.is_ok());
    }

    #[test]
    fn github_meta_takes_only_the_named_fields() {
        let body = serde_json::json!({
            "hooks": ["140.82.112.0/20", "2a0a:a440::/29"],
            "actions": ["4.148.0.0/16"],
        });

        let fields = ["hooks".to_string()];
        assert_eq!(
            Allowlist::github_meta(&body, &fields),
            ["140.82.112.0/20", "2a0a:a440::/29"]
        );
    }

    #[test]
    fn cloudflare_ips_takes_both_families() {
        let body = serde_json::json!({
            "result": {
                "ipv4_cidrs": ["104.16.0.0/13"],
                "ipv6_cidrs": ["2606:4700::/32"],
                "etag": "abc",
            },
            "success": true,
        });

        assert_eq!(
            Allowlist::cloudflare_ips(&body),
            ["104.16.0.0/13", "2606:4700::/32"]
        );
    }

    #[test]
    fn an_unexpected_body_yields_nothing_rather_than_panicking() {
        let body = serde_json::json!({ "message": "rate limited" });

        assert!(Allowlist::cloudflare_ips(&body).is_empty());
        assert!(Allowlist::github_meta(&body, &["hooks".to_string()]).is_empty());
    }

    #[tokio::test]
    async fn fetched_entries_join_the_base() {
        let allowlist = allowlist("10.0.0.0/8");
        assert!(allowlist.parse_and_check("140.82.115.33").await.is_ok());

        allowlist.inner.fetched.write().await.insert(
            "github-hooks".to_string(),
            Allowlist::parse_list("140.82.112.0/20", "github-hooks"),
        );
        allowlist.rebuild().await;

        assert!(allowlist.parse_and_check("140.82.115.33").await.is_err());
        assert!(allowlist.parse_and_check("10.1.2.3").await.is_err());
    }

    #[tokio::test]
    async fn manual_entries_survive_a_feed_refresh() {
        let allowlist = allowlist("10.0.0.0/8");

        allowlist
            .set_manual(vec![(
                "203.0.113.0/24".parse().unwrap(),
                MANUAL_SOURCE.to_string(),
            )])
            .await;

        allowlist.inner.fetched.write().await.insert(
            "github".to_string(),
            Allowlist::parse_list("192.0.2.0/24", "github"),
        );

        allowlist.rebuild().await;

        let entries = allowlist.entries().await;
        let sources: Vec<&str> = entries.iter().map(|(_, why)| why.as_str()).collect();

        assert!(sources.contains(&"test"));
        assert!(sources.contains(&MANUAL_SOURCE));
        assert!(sources.contains(&"github"));
        assert!(
            allowlist
                .check(&"203.0.113.9/32".parse().unwrap())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn removing_the_last_manual_entry_stops_protecting_it() {
        let allowlist = allowlist("10.0.0.0/8");
        let net: IpNet = "203.0.113.0/24".parse().unwrap();

        allowlist
            .set_manual(vec![(net, MANUAL_SOURCE.to_string())])
            .await;
        assert!(allowlist.check(&net).await.is_err());

        allowlist.set_manual(Vec::new()).await;
        assert!(allowlist.check(&net).await.is_ok());
    }

    #[tokio::test]
    async fn clones_share_one_snapshot() {
        let allowlist = allowlist("");
        let clone = allowlist.clone();

        allowlist.inner.fetched.write().await.insert(
            "github-hooks".to_string(),
            Allowlist::parse_list("140.82.112.0/20", "github-hooks"),
        );
        allowlist.rebuild().await;

        assert!(clone.parse_and_check("140.82.115.33").await.is_err());
    }
}

#[cfg(test)]
mod live {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn real_feeds_protect_github_and_cloudflare() {
        let allowlist = Allowlist::new(
            Vec::new(),
            vec![
                AllowlistSource {
                    name: "github-hooks".into(),
                    url: "https://api.github.com/meta".into(),
                    format: SourceFormat::GithubMeta,
                    fields: vec!["hooks".into()],
                },
                AllowlistSource {
                    name: "cloudflare".into(),
                    url: "https://api.cloudflare.com/client/v4/ips".into(),
                    format: SourceFormat::CloudflareIps,
                    fields: Vec::new(),
                },
            ],
        );

        assert!(allowlist.refresh().await.is_empty());

        assert!(allowlist.parse_and_check("140.82.115.33").await.is_err());
        assert!(allowlist.parse_and_check("104.16.5.5").await.is_err());
        assert!(allowlist.parse_and_check("203.0.113.4").await.is_ok());
    }
}
