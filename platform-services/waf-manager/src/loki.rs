use crate::error::{Error, Result};
use ipnet::IpNet;
use serde::Deserialize;
use std::str::FromStr;

const SELECTOR: &str =
    r#"{namespace="envoy-gateway-system", container="envoy"} |= `coraza/config.go`"#;

const PARSE: &str = r"| json | line_format `{{.msg}}`";
const RE_CLIENT: &str = r#"| regexp `\[client "(?P<client_ip>[^"]*)"\]`"#;
const RE_ID: &str = r#"| regexp `\[id "(?P<rule_id>\d+)"\]`"#;
const RE_MSG: &str = r#"| regexp `\[msg "(?P<rule_msg>[^"]*)"\]`"#;
const RE_URI: &str = r#"| regexp `\[uri "(?P<uri>[^"]*)"\]`"#;

const STRIP_SCORE: &str =
    r#"| label_format rule_msg=`{{ regexReplaceAll " \\(Total Score: [0-9]+\\)" .rule_msg "" }}`"#;

const ACCESS_SELECTOR: &str = r#"{namespace="envoy-gateway-system", container="envoy"} |= `x-forwarded-for` | json | __error__=``"#;

const RE_XFF: &str = r#"| regexp `"x-forwarded-for":"(?P<client_ip>[^",]*)"`"#;

const FMT_PATH: &str = r"| label_format path=`{{ .x_envoy_origin_path }}`";

#[derive(Debug, Clone)]
pub struct Candidate {
    pub client_ip: String,
    pub detections: u64,
}

#[derive(Debug, Clone)]
pub struct UriHit {
    pub uri: String,
    pub count: u64,
}

#[derive(Debug, Clone)]
pub struct RuleHit {
    pub rule_id: String,
    pub rule_msg: String,
    pub severity: String,
    pub count: u64,
}

pub struct Loki {
    client: reqwest::Client,
    base_url: String,
}

impl Loki {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    pub async fn top_client_ips(&self, window: &str, limit: usize) -> Result<Vec<Candidate>> {
        let query = format!(
            "topk({limit}, sum by (client_ip) (count_over_time({SELECTOR} {PARSE} {RE_CLIENT} [{window}])))"
        );

        self.candidates_from(&query).await
    }

    pub async fn top_client_ips_by_distinct_rules(
        &self,
        window: &str,
        limit: usize,
    ) -> Result<Vec<Candidate>> {
        let ranked = format!(
            "topk({limit}, count by (client_ip) (sum by (client_ip, rule_id) \
             (count_over_time({SELECTOR} {PARSE} {RE_CLIENT} {RE_ID} [{window}]))))"
        );

        let selected = self.candidates_from(&ranked).await?;

        if selected.is_empty() {
            return Ok(Vec::new());
        }

        let totals = format!(
            "sum by (client_ip) (count_over_time({SELECTOR} {PARSE} {RE_CLIENT} [{window}]))"
        );

        let totals: std::collections::BTreeMap<String, u64> = self
            .candidates_from(&totals)
            .await?
            .into_iter()
            .map(|c| (c.client_ip, c.detections))
            .collect();

        let mut candidates: Vec<Candidate> = selected
            .into_iter()
            .map(|c| Candidate {
                detections: totals.get(&c.client_ip).copied().unwrap_or(c.detections),
                client_ip: c.client_ip,
            })
            .collect();

        candidates.sort_by_key(|c| std::cmp::Reverse(c.detections));
        Ok(candidates)
    }

    pub async fn candidates_from(&self, query: &str) -> Result<Vec<Candidate>> {
        let mut candidates: Vec<Candidate> = self
            .instant(query)
            .await?
            .into_iter()
            .filter_map(|s| {
                let client_ip = s.metric.get("client_ip")?.clone();
                if client_ip.is_empty() {
                    return None;
                }

                Some(Candidate {
                    client_ip,
                    detections: s.count,
                })
            })
            .collect();

        candidates.sort_by_key(|c| std::cmp::Reverse(c.detections));
        Ok(candidates)
    }

    pub async fn rules_for_ip(&self, client_ip: &str, window: &str) -> Result<Vec<RuleHit>> {
        let ip = Self::sanitise(client_ip)?;
        let query = format!(
            "topk(20, sum by (rule_id, rule_msg, severity) (count_over_time(\
             {SELECTOR} {} {PARSE} {RE_CLIENT} {} {RE_ID} {RE_MSG} {STRIP_SCORE} [{window}])))",
            Self::prefilter(&ip),
            Self::exact_client(&ip),
        );

        let mut hits: Vec<RuleHit> = self
            .instant(&query)
            .await?
            .into_iter()
            .filter_map(|s| {
                let rule_id = s.metric.get("rule_id")?.clone();
                if rule_id.is_empty() {
                    return None;
                }

                Some(RuleHit {
                    rule_id,
                    rule_msg: s.metric.get("rule_msg").cloned().unwrap_or_default(),
                    severity: s.metric.get("severity").cloned().unwrap_or_default(),
                    count: s.count,
                })
            })
            .collect();

        hits.sort_by_key(|h| std::cmp::Reverse(h.count));
        Ok(hits)
    }

    pub async fn uris_for_ip(&self, client_ip: &str, window: &str) -> Result<Vec<UriHit>> {
        let ip = Self::sanitise(client_ip)?;
        let query = format!(
            "topk(20, sum by (uri) (count_over_time(\
             {SELECTOR} {} {PARSE} {RE_CLIENT} {} {RE_URI} [{window}])))",
            Self::prefilter(&ip),
            Self::exact_client(&ip),
        );

        let mut hits: Vec<UriHit> = self
            .instant(&query)
            .await?
            .into_iter()
            .filter_map(|s| {
                let uri = s.metric.get("uri")?.clone();
                if uri.is_empty() {
                    return None;
                }

                Some(UriHit {
                    uri,
                    count: s.count,
                })
            })
            .collect();

        hits.sort_by_key(|h| std::cmp::Reverse(h.count));
        Ok(hits)
    }

    pub async fn recent_lines(
        &self,
        client_ip: &str,
        window: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
        let ip = Self::sanitise(client_ip)?;
        let query = format!(
            "{SELECTOR} {} {PARSE} {RE_CLIENT} {} {RE_ID} {RE_MSG} {RE_URI} \
             | line_format `[{{{{.severity}}}}] {{{{.rule_id}}}} {{{{.rule_msg}}}} {{{{.uri}}}}`",
            Self::prefilter(&ip),
            Self::exact_client(&ip),
        );

        let url = self.url(
            "query_range",
            &[
                ("query", query.as_str()),
                ("since", window),
                ("limit", &limit.to_string()),
                ("direction", "backward"),
            ],
        );

        let body: Envelope = self
            .client
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let Data::Streams(streams) = body.data else {
            return Err(Error::Loki(
                "expected a streams result for a log query".to_string(),
            ));
        };

        let mut lines: Vec<(String, String)> = streams
            .into_iter()
            .flat_map(|s| s.values)
            .map(|entry| (entry.0, entry.1))
            .collect();

        lines.sort_by(|a, b| b.0.cmp(&a.0));
        Ok(lines
            .into_iter()
            .map(|(_, line)| line)
            .take(limit)
            .collect())
    }

    pub async fn scalar(&self, query: &str) -> Result<u64> {
        Ok(self.instant(query).await?.iter().map(|s| s.count).sum())
    }

    pub fn fragments() -> std::collections::BTreeMap<&'static str, &'static str> {
        [
            ("selector", SELECTOR),
            ("parse", PARSE),
            ("re_client", RE_CLIENT),
            ("re_id", RE_ID),
            ("re_msg", RE_MSG),
            ("re_uri", RE_URI),
            ("strip_score", STRIP_SCORE),
            ("access_selector", ACCESS_SELECTOR),
            ("re_xff", RE_XFF),
            ("fmt_path", FMT_PATH),
        ]
        .into_iter()
        .collect()
    }

    pub fn client_filters(client_ip: &str) -> Result<(String, String, String)> {
        let ip = Self::sanitise(client_ip)?;
        Ok((Self::prefilter(&ip), Self::exact_client(&ip), ip))
    }

    async fn instant(&self, query: &str) -> Result<Vec<Sample>> {
        let url = self.url("query", &[("query", query)]);
        let body: Envelope = self
            .client
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let Data::Vector(samples) = body.data else {
            return Err(Error::Loki(format!(
                "expected a vector result for {query:?}"
            )));
        };

        Ok(samples
            .into_iter()
            .map(|s| Sample {
                count: s.value.1.parse().unwrap_or(0),
                metric: s.metric,
            })
            .collect())
    }

    fn url(&self, path: &str, params: &[(&str, &str)]) -> String {
        let query = serde_urlencoded::to_string(params).unwrap_or_default();
        format!("{}/loki/api/v1/{path}?{query}", self.base_url)
    }

    fn prefilter(ip: &str) -> String {
        format!("|= `{ip}`")
    }

    fn exact_client(ip: &str) -> String {
        format!("| client_ip = `{ip}`")
    }

    pub fn sanitise(client_ip: &str) -> Result<String> {
        let trimmed = client_ip.trim();

        if let Ok(addr) = std::net::IpAddr::from_str(trimmed) {
            return Ok(addr.to_string());
        }

        if let Ok(net) = IpNet::from_str(trimmed) {
            return Ok(net.trunc().to_string());
        }

        Err(Error::InvalidCidr(
            client_ip.to_string(),
            "not an ip address or cidr".to_string(),
        ))
    }
}

struct Sample {
    metric: std::collections::HashMap<String, String>,
    count: u64,
}

#[derive(Deserialize)]
struct Envelope {
    data: Data,
}

#[derive(Deserialize)]
#[serde(tag = "resultType", content = "result")]
enum Data {
    #[serde(rename = "vector")]
    Vector(Vec<VectorSample>),
    #[serde(rename = "streams")]
    Streams(Vec<Stream>),
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct VectorSample {
    metric: std::collections::HashMap<String, String>,
    value: (serde_json::Value, String),
}

#[derive(Deserialize)]
struct Stream {
    values: Vec<(String, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitise_accepts_plain_addresses_and_cidrs() {
        assert_eq!(Loki::sanitise("203.0.113.4").unwrap(), "203.0.113.4");
        assert_eq!(Loki::sanitise("  203.0.113.4  ").unwrap(), "203.0.113.4");
        assert_eq!(Loki::sanitise("2001:db8::1").unwrap(), "2001:db8::1");
        assert_eq!(Loki::sanitise("203.0.113.0/24").unwrap(), "203.0.113.0/24");
        assert_eq!(Loki::sanitise("203.0.113.4/24").unwrap(), "203.0.113.0/24");
    }

    #[test]
    fn sanitise_rejects_anything_that_could_reshape_a_query() {
        for hostile in [
            "",
            "   ",
            "203.0.113.4`",
            "`",
            "203.0.113.4` | drop client_ip | `",
            "203.0.113.4\n| json",
            "{namespace=\"kube-system\"}",
            "}",
            "not-an-ip",
        ] {
            assert!(
                Loki::sanitise(hostile).is_err(),
                "expected {hostile:?} to be rejected"
            );
        }
    }
}
