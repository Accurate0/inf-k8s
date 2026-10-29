use std::collections::BTreeMap;

use anyhow::bail;
use clap::Args;
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};
use serde::Deserialize;

use crate::grafana::{Grafana, parse_duration};

#[derive(Args, Debug)]
pub struct MetricsArgs {
    /// PromQL expression
    pub query: String,
    #[arg(long, help = "Evaluate over this window instead of instantly, e.g. 1h")]
    pub range: Option<String>,
    #[arg(long, default_value = "1m")]
    pub step: String,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Deserialize)]
struct Response {
    data: Data,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Data {
    result_type: String,
    result: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct Vector {
    metric: BTreeMap<String, String>,
    value: (f64, String),
}

#[derive(Debug, Deserialize)]
struct Matrix {
    metric: BTreeMap<String, String>,
    values: Vec<(f64, String)>,
}

pub fn labels(metric: &BTreeMap<String, String>) -> String {
    let name = metric.get("__name__").cloned().unwrap_or_default();
    let rest: Vec<String> = metric
        .iter()
        .filter(|(k, _)| *k != "__name__")
        .map(|(k, v)| format!("{k}=\"{v}\""))
        .collect();
    match (name.is_empty(), rest.is_empty()) {
        (true, true) => "{}".to_owned(),
        (false, true) => name,
        (_, false) => format!("{name}{{{}}}", rest.join(", ")),
    }
}

pub struct Summary {
    pub last: f64,
    pub min: f64,
    pub max: f64,
    pub avg: f64,
}

pub fn summarise(values: &[(f64, String)]) -> Option<Summary> {
    let numbers: Vec<f64> = values
        .iter()
        .filter_map(|(_, v)| v.parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .collect();
    let last = *numbers.last()?;
    Some(Summary {
        last,
        min: numbers.iter().copied().fold(f64::INFINITY, f64::min),
        max: numbers.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        avg: numbers.iter().sum::<f64>() / numbers.len() as f64,
    })
}

fn number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        format!("{value:.4}")
    }
}

pub async fn run(grafana: &Grafana, args: MetricsArgs) -> anyhow::Result<()> {
    let now = chrono::Utc::now();
    let response: Response = match &args.range {
        None => {
            grafana
                .datasource(
                    "prometheus",
                    "/api/v1/query",
                    &[
                        ("query", args.query.clone()),
                        ("time", now.timestamp().to_string()),
                    ],
                )
                .await?
        }
        Some(range) => {
            let start = now - parse_duration(range)?;
            let step = parse_duration(&args.step)?.num_seconds().max(1);
            grafana
                .datasource(
                    "prometheus",
                    "/api/v1/query_range",
                    &[
                        ("query", args.query.clone()),
                        ("start", start.timestamp().to_string()),
                        ("end", now.timestamp().to_string()),
                        ("step", step.to_string()),
                    ],
                )
                .await?
        }
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response.data.result)?);
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    match response.data.result_type.as_str() {
        "vector" => {
            let mut rows: Vec<Vector> = serde_json::from_value(response.data.result)?;
            rows.sort_by_key(|r| labels(&r.metric));
            table.set_header(["series", "value"]);
            for row in rows {
                table.add_row([labels(&row.metric), row.value.1]);
            }
        }
        "matrix" => {
            let mut rows: Vec<Matrix> = serde_json::from_value(response.data.result)?;
            rows.sort_by_key(|r| labels(&r.metric));
            table.set_header(["series", "last", "min", "max", "avg"]);
            for row in rows {
                if let Some(s) = summarise(&row.values) {
                    table.add_row([
                        labels(&row.metric),
                        number(s.last),
                        number(s.min),
                        number(s.max),
                        number(s.avg),
                    ]);
                }
            }
        }
        "scalar" | "string" => {
            let (_, value): (f64, String) = serde_json::from_value(response.data.result)?;
            println!("{value}");
            return Ok(());
        }
        other => bail!("unexpected result type {other}"),
    }
    if table.row_count() == 0 {
        println!("no data");
    } else {
        println!("{table}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_sets_render_like_promql() {
        let mut metric = BTreeMap::new();
        assert_eq!(labels(&metric), "{}");
        metric.insert("__name__".into(), "up".into());
        assert_eq!(labels(&metric), "up");
        metric.insert("job".into(), "api".into());
        assert_eq!(labels(&metric), r#"up{job="api"}"#);
    }

    #[test]
    fn summaries_skip_nan() {
        let values = vec![
            (1.0, "2".into()),
            (2.0, "NaN".into()),
            (3.0, "4".into()),
            (4.0, "0".into()),
        ];
        let s = summarise(&values).unwrap();
        assert_eq!((s.last, s.min, s.max, s.avg), (0.0, 0.0, 4.0, 2.0));
        assert!(summarise(&[]).is_none());
    }
}
