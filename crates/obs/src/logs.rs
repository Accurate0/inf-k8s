use std::collections::BTreeMap;

use clap::Args;
use serde::Deserialize;

use crate::grafana::{Grafana, parse_duration};

#[derive(Args, Debug)]
pub struct LogsArgs {
    /// LogQL query; built from --ns/--app/--pod/--grep when omitted
    pub query: Option<String>,
    #[arg(short, long)]
    pub ns: Option<String>,
    #[arg(long)]
    pub app: Option<String>,
    #[arg(long)]
    pub pod: Option<String>,
    #[arg(short, long, help = "Only lines containing this text")]
    pub grep: Option<String>,
    #[arg(long, default_value = "1h")]
    pub since: String,
    #[arg(long, default_value_t = 200)]
    pub limit: u32,
    #[arg(short, long, help = "Keep polling for new lines")]
    pub follow: bool,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Deserialize)]
struct Response {
    data: Data,
}

#[derive(Debug, Deserialize)]
struct Data {
    result: Vec<Stream>,
}

#[derive(Debug, Deserialize)]
struct Stream {
    stream: BTreeMap<String, String>,
    values: Vec<(String, String)>,
}

#[derive(Debug, serde::Serialize)]
pub struct Entry {
    pub ts: i64,
    pub labels: BTreeMap<String, String>,
    pub line: String,
}

fn escape(value: &str) -> String {
    value.replace('\\', r"\\").replace('"', "\\\"")
}

pub fn build_query(args: &LogsArgs) -> anyhow::Result<String> {
    if let Some(query) = &args.query {
        return Ok(query.clone());
    }
    let mut matchers = Vec::new();
    if let Some(ns) = &args.ns {
        matchers.push(format!("namespace=\"{}\"", escape(ns)));
    }
    if let Some(app) = &args.app {
        matchers.push(format!("app=\"{}\"", escape(app)));
    }
    if let Some(pod) = &args.pod {
        matchers.push(format!("pod=~\"{}.*\"", escape(pod)));
    }
    if matchers.is_empty() {
        anyhow::bail!("pass a LogQL query or at least one of --ns, --app, --pod");
    }
    let mut query = format!("{{{}}}", matchers.join(","));
    if let Some(grep) = &args.grep {
        query.push_str(&format!(" |= \"{}\"", escape(grep)));
    }
    Ok(query)
}

fn flatten(response: Response) -> Vec<Entry> {
    let mut entries: Vec<Entry> = response
        .data
        .result
        .into_iter()
        .flat_map(|stream| {
            let labels = stream.stream;
            stream.values.into_iter().map(move |(ts, line)| Entry {
                ts: ts.parse().unwrap_or_default(),
                labels: labels.clone(),
                line,
            })
        })
        .collect();
    entries.sort_by_key(|e| e.ts);
    entries
}

fn print(entry: &Entry) {
    let time = chrono::DateTime::from_timestamp_nanos(entry.ts)
        .with_timezone(&chrono::Local)
        .format("%m-%d %H:%M:%S");
    let source = match (entry.labels.get("namespace"), entry.labels.get("pod")) {
        (Some(ns), Some(pod)) => format!("{ns}/{pod}"),
        (Some(ns), None) => ns.clone(),
        _ => entry
            .labels
            .get("job")
            .cloned()
            .unwrap_or_else(|| "-".to_owned()),
    };
    println!("{time} {source} {}", entry.line);
}

async fn fetch(
    grafana: &Grafana,
    query: &str,
    start: i64,
    end: i64,
    limit: u32,
    direction: &str,
) -> anyhow::Result<Vec<Entry>> {
    let response: Response = grafana
        .datasource(
            "loki",
            "/loki/api/v1/query_range",
            &[
                ("query", query.to_owned()),
                ("start", start.to_string()),
                ("end", end.to_string()),
                ("limit", limit.to_string()),
                ("direction", direction.to_owned()),
            ],
        )
        .await?;
    Ok(flatten(response))
}

fn now_ns() -> i64 {
    chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
}

pub async fn run(grafana: &Grafana, args: LogsArgs) -> anyhow::Result<()> {
    let query = build_query(&args)?;
    let since = parse_duration(&args.since)?;
    let end = now_ns();
    let start = end - since.num_nanoseconds().unwrap_or(i64::MAX / 2);

    let entries = fetch(grafana, &query, start, end, args.limit, "backward").await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }
    entries.iter().for_each(print);

    if !args.follow {
        return Ok(());
    }
    let mut last = entries.last().map_or(end, |e| e.ts);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let entries = fetch(grafana, &query, last + 1, now_ns(), 5000, "forward").await?;
        if let Some(newest) = entries.last() {
            last = newest.ts;
        }
        entries.iter().for_each(print);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> LogsArgs {
        LogsArgs {
            query: None,
            ns: None,
            app: None,
            pod: None,
            grep: None,
            since: "1h".into(),
            limit: 10,
            follow: false,
            json: false,
        }
    }

    #[test]
    fn builds_selectors_from_flags() {
        let mut a = args();
        a.ns = Some("waf-manager".into());
        a.grep = Some("say \"hi\"".into());
        assert_eq!(
            build_query(&a).unwrap(),
            r#"{namespace="waf-manager"} |= "say \"hi\"""#
        );
        a.app = Some("api".into());
        a.pod = Some("api-7f".into());
        assert_eq!(
            build_query(&a).unwrap(),
            r#"{namespace="waf-manager",app="api",pod=~"api-7f.*"} |= "say \"hi\"""#
        );
    }

    #[test]
    fn explicit_query_wins_and_empty_is_rejected() {
        let mut a = args();
        assert!(build_query(&a).is_err());
        a.query = Some(r#"{job="x"}"#.into());
        a.ns = Some("ignored".into());
        assert_eq!(build_query(&a).unwrap(), r#"{job="x"}"#);
    }

    #[test]
    fn streams_flatten_oldest_first() {
        let response: Response = serde_json::from_str(
            r#"{"data":{"result":[
              {"stream":{"namespace":"a"},"values":[["30","c"],["10","a"]]},
              {"stream":{"namespace":"b"},"values":[["20","b"]]}]}}"#,
        )
        .unwrap();
        let lines: Vec<_> = flatten(response).into_iter().map(|e| e.line).collect();
        assert_eq!(lines, ["a", "b", "c"]);
    }
}
