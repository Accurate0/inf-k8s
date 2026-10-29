use std::time::Duration;

use anyhow::{Context, bail};
use serde::de::DeserializeOwned;

pub struct Grafana {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl Grafana {
    pub fn new(base: &str, token: String) -> anyhow::Result<Self> {
        Ok(Self {
            base: base.trim_end_matches('/').to_owned(),
            token,
            http: reqwest::Client::builder()
                .user_agent(concat!("obs/", env!("CARGO_PKG_VERSION")))
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(60))
                .build()?,
        })
    }

    pub async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> anyhow::Result<T> {
        let response = self
            .http
            .get(format!("{}{path}", self.base))
            .header("X-JWT-Assertion", &self.token)
            .query(query)
            .send()
            .await
            .with_context(|| format!("calling grafana at {}", self.base))?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            bail!("grafana returned {status}: {}", body.trim());
        }
        serde_json::from_str(&body).with_context(|| format!("unexpected response from {path}"))
    }

    pub async fn datasource<T: DeserializeOwned>(
        &self,
        uid: &str,
        path: &str,
        query: &[(&str, String)],
    ) -> anyhow::Result<T> {
        self.get(&format!("/api/datasources/proxy/uid/{uid}{path}"), query)
            .await
    }
}

pub fn parse_duration(value: &str) -> anyhow::Result<chrono::Duration> {
    let value = value.trim();
    let split = value
        .find(|c: char| !c.is_ascii_digit())
        .with_context(|| format!("{value:?} needs a unit (s, m, h, d, w)"))?;
    let (number, unit) = value.split_at(split);
    let number: i64 = number
        .parse()
        .with_context(|| format!("invalid duration {value:?}"))?;
    Ok(match unit {
        "s" => chrono::Duration::seconds(number),
        "m" => chrono::Duration::minutes(number),
        "h" => chrono::Duration::hours(number),
        "d" => chrono::Duration::days(number),
        "w" => chrono::Duration::weeks(number),
        _ => bail!("unknown duration unit {unit:?} in {value:?}; use s, m, h, d or w"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(
            parse_duration("30m").unwrap(),
            chrono::Duration::minutes(30)
        );
        assert_eq!(parse_duration("1h").unwrap(), chrono::Duration::hours(1));
        assert_eq!(parse_duration("7d").unwrap(), chrono::Duration::days(7));
        assert!(parse_duration("10").is_err());
        assert!(parse_duration("5y").is_err());
        assert!(parse_duration("h").is_err());
    }
}
