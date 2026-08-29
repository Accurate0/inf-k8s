use crate::error::{Error, Result};
use serde::Deserialize;

/// Must be `coraza/config.go` (the `caller` field), not `Coraza:` - the latter also
/// matches the JSON audit lines, which carry no top-level `msg`, so `line_format`
/// yields an empty line and every downstream regexp produces empty labels.
const SELECTOR: &str =
    r#"{namespace="envoy-gateway-system", container="envoy"} |= `coraza/config.go`"#;

const PARSE: &str = r"| json | line_format `{{.msg}}`";
const RE_CLIENT: &str = r#"| regexp `\[client "(?P<client_ip>[^"]*)"\]`"#;
const RE_ID: &str = r#"| regexp `\[id "(?P<rule_id>\d+)"\]`"#;
const RE_MSG: &str = r#"| regexp `\[msg "(?P<rule_msg>[^"]*)"\]`"#;
const RE_URI: &str = r#"| regexp `\[uri "(?P<uri>[^"]*)"\]`"#;

/// Anomaly-score messages embed a varying score and would otherwise fan out into
/// one series per score.
const STRIP_SCORE: &str =
    r#"| label_format rule_msg=`{{ regexReplaceAll " \\(Total Score: [0-9]+\\)" .rule_msg "" }}`"#;

/// The proxy's JSON access log, a different stream from the Coraza detections
/// above. Reaches requests that tripped no rule at all - directory brute-forcing
/// being the case worth blocking.
const ACCESS_SELECTOR: &str = r#"{namespace="envoy-gateway-system", container="envoy"} |= `x-forwarded-for` | json | __error__=``"#;

/// Cloudflare appends itself to `x-forwarded-for`, so the first entry is the real
/// client. Stopping at a comma takes it without needing a template function.
const RE_XFF: &str = r#"| regexp `"x-forwarded-for":"(?P<client_ip>[^",]*)"`"#;

/// The requested path, whose JSON key is not a valid label name on its own.
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

/// Reads Coraza detections out of Loki, which runs with `auth_enabled: false`
/// in-cluster, so there is no tenant header to set.
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

    /// Any instant query whose series carry a `client_ip` label, so a workflow can
    /// bring its own definition of "worth looking at".
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
        let ip = Self::sanitise(client_ip);
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

    /// The paths an IP requested while tripping rules, which is what identifies a
    /// credential-file probe: the rule id says "restricted file access", the URI
    /// says it was `/.env`.
    pub async fn uris_for_ip(&self, client_ip: &str, window: &str) -> Result<Vec<UriHit>> {
        let ip = Self::sanitise(client_ip);
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
        let ip = Self::sanitise(client_ip);
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

    /// Runs an operator-written instant query and folds it to a single number,
    /// summing every series so a query that forgets to aggregate still yields
    /// something sensible. An empty result is zero, not an error: "this IP did
    /// nothing" is a normal answer.
    pub async fn scalar(&self, query: &str) -> Result<u64> {
        Ok(self.instant(query).await?.iter().map(|s| s.count).sum())
    }

    /// Reusable LogQL pieces, so a workflow's custom query does not have to
    /// restate the Coraza selector and its regexps. Values here are the same
    /// consts the built-in queries use, so the two cannot drift.
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

    /// The line prefilter and the exact label match for one address, both already
    /// escaped. A workflow query interpolates these rather than splicing a raw
    /// address into LogQL itself.
    pub fn client_filters(client_ip: &str) -> (String, String, String) {
        let ip = Self::sanitise(client_ip);
        (Self::prefilter(&ip), Self::exact_client(&ip), ip)
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

    /// Cheap prune against the raw line. It cannot match `[client "ip"]`, because
    /// a line filter placed before the parser sees the original JSON, where the
    /// quotes are backslash-escaped.
    fn prefilter(ip: &str) -> String {
        format!("|= `{ip}`")
    }

    /// The exact match, applied after `regexp` has extracted the label, so a
    /// substring of a longer address cannot pass.
    fn exact_client(ip: &str) -> String {
        format!("| client_ip = `{ip}`")
    }

    /// Backticks would terminate the LogQL raw strings the address is spliced
    /// into. An IP cannot contain one, but this value arrives from a URL path.
    fn sanitise(client_ip: &str) -> String {
        client_ip.chars().filter(|c| *c != '`').collect()
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
