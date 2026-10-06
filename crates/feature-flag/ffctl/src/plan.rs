use crate::client::Api;
use crate::convert::Convert;
use console::style;
use feature_flag_proto as pb;
use similar::{ChangeTag, TextDiff};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffTag {
    Equal,
    Insert,
    Delete,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub tag: DiffTag,
    pub text: String,
}

pub struct Diff;

impl Diff {
    pub fn lines(before: &str, after: &str) -> Vec<DiffLine> {
        TextDiff::from_lines(before, after)
            .iter_all_changes()
            .map(|change| {
                let value = change.value();

                DiffLine {
                    tag: match change.tag() {
                        ChangeTag::Equal => DiffTag::Equal,
                        ChangeTag::Insert => DiffTag::Insert,
                        ChangeTag::Delete => DiffTag::Delete,
                    },
                    text: value.strip_suffix('\n').unwrap_or(value).to_owned(),
                }
            })
            .collect()
    }

    pub fn print(before: &str, after: &str) {
        for line in Self::lines(before, after) {
            let rendered = match line.tag {
                DiffTag::Delete => style(format!("  - {}", line.text)).red(),
                DiffTag::Insert => style(format!("  + {}", line.text)).green(),
                DiffTag::Equal => style(format!("    {}", line.text)).dim(),
            };

            println!("{rendered}");
        }
    }
}

pub struct LiveState {
    pub flags: BTreeMap<String, pb::Flag>,
    pub segments: BTreeMap<String, pb::Segment>,
}

impl LiveState {
    pub async fn fetch(api: &Api) -> anyhow::Result<Self> {
        let flags = api
            .list_flags(true)
            .await?
            .into_iter()
            .map(|flag| (flag.key.clone(), flag))
            .collect();

        let segments = api
            .list_segments()
            .await?
            .into_iter()
            .map(|segment| (segment.key.clone(), segment))
            .collect();

        Ok(Self { flags, segments })
    }
}

pub struct Plan<'a> {
    pub changes: &'a [pb::ConfigChange],
    pub desired_flags: &'a [pb::Flag],
    pub desired_segments: &'a [pb::Segment],
    pub live: &'a LiveState,
}

impl Plan<'_> {
    pub fn render(&self) {
        if self.changes.is_empty() {
            println!("No changes. Live state already matches the config.");
            return;
        }

        let desired_flags: BTreeMap<&str, &pb::Flag> = self
            .desired_flags
            .iter()
            .map(|flag| (flag.key.as_str(), flag))
            .collect();

        let desired_segments: BTreeMap<&str, &pb::Segment> = self
            .desired_segments
            .iter()
            .map(|segment| (segment.key.as_str(), segment))
            .collect();

        let (mut create, mut update, mut delete) = (0, 0, 0);

        for change in self.changes {
            let (symbol, count) = match change.op() {
                pb::ChangeOp::Create => ("+", &mut create),
                pb::ChangeOp::Update => ("~", &mut update),
                pb::ChangeOp::Delete => ("-", &mut delete),
                pb::ChangeOp::Unspecified => ("?", &mut update),
            };
            *count += 1;

            let key = change.target_key.as_str();

            let (before, after) = match change.target_kind.as_str() {
                "flag" => (
                    self.live
                        .flags
                        .get(key)
                        .map(Convert::flag_yaml)
                        .unwrap_or_default(),
                    desired_flags
                        .get(key)
                        .map(|flag| Convert::flag_yaml(flag))
                        .unwrap_or_default(),
                ),
                _ => (
                    self.live
                        .segments
                        .get(key)
                        .map(Convert::segment_yaml)
                        .unwrap_or_default(),
                    desired_segments
                        .get(key)
                        .map(|segment| Convert::segment_yaml(segment))
                        .unwrap_or_default(),
                ),
            };

            let header = format!("{symbol} {} {}", change.target_kind, change.target_key);
            let header = match change.op() {
                pb::ChangeOp::Create => style(header).green(),
                pb::ChangeOp::Delete => style(header).red(),
                _ => style(header).yellow(),
            };

            println!("\n{}", header.bold());
            Diff::print(&before, &after);
        }

        println!(
            "\nPlan: {} to create, {} to update, {} to delete.",
            style(create).green(),
            style(update).yellow(),
            style(delete).red(),
        );
    }
}
