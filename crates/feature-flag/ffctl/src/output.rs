use crate::convert::Convert;
use clap::ValueEnum;
use comfy_table::presets::UTF8_BORDERS_ONLY;
use comfy_table::{ContentArrangement, Table};
use feature_flag_proto as pb;
use serde_json::Value as Json;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Json,
    Yaml,
    Table,
}

#[derive(Debug, Clone, Copy)]
pub struct Output {
    format: Format,
}

impl Output {
    pub fn new(format: Format) -> Self {
        Self { format }
    }

    fn json(value: &Json) -> anyhow::Result<()> {
        println!("{}", serde_json::to_string_pretty(value)?);

        Ok(())
    }

    fn yaml(value: &Json) -> anyhow::Result<()> {
        print!("{}", serde_yaml::to_string(value)?);

        Ok(())
    }

    fn table(header: &[&str]) -> Table {
        let mut table = Table::new();
        table
            .load_preset(UTF8_BORDERS_ONLY)
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_header(header.to_vec());

        table
    }

    fn compact(value: &Json) -> String {
        match value {
            Json::String(text) => text.clone(),
            other => other.to_string(),
        }
    }

    pub fn flag(&self, flag: &pb::Flag) -> anyhow::Result<()> {
        match self.format {
            Format::Json => Self::json(&Convert::flag_to_json(flag)),
            Format::Yaml | Format::Table => {
                print!("{}", Convert::flag_yaml(flag));
                Ok(())
            }
        }
    }

    pub fn flags(&self, flags: &[pb::Flag]) -> anyhow::Result<()> {
        let json = || Json::Array(flags.iter().map(Convert::flag_to_json).collect());

        match self.format {
            Format::Json => Self::json(&json()),
            Format::Yaml => Self::yaml(&json()),
            Format::Table => {
                let mut table = Self::table(&[
                    "KEY", "TYPE", "ENABLED", "DEFAULT", "VARIANTS", "RULES", "ARCHIVED",
                ]);

                for flag in flags {
                    table.add_row(vec![
                        flag.key.clone(),
                        Convert::value_type_name(flag.value_type).to_owned(),
                        flag.enabled.to_string(),
                        flag.default_variant_key.clone(),
                        flag.variants.len().to_string(),
                        flag.rules.len().to_string(),
                        flag.archived.to_string(),
                    ]);
                }

                println!("{table}");
                Ok(())
            }
        }
    }

    pub fn segment(&self, segment: &pb::Segment) -> anyhow::Result<()> {
        match self.format {
            Format::Json => Self::json(&Convert::segment_to_json(segment)),
            Format::Yaml | Format::Table => {
                print!("{}", Convert::segment_yaml(segment));
                Ok(())
            }
        }
    }

    pub fn segments(&self, segments: &[pb::Segment]) -> anyhow::Result<()> {
        let json = || Json::Array(segments.iter().map(Convert::segment_to_json).collect());

        match self.format {
            Format::Json => Self::json(&json()),
            Format::Yaml => Self::yaml(&json()),
            Format::Table => {
                let mut table = Self::table(&["KEY", "NAME", "CONSTRAINTS"]);

                for segment in segments {
                    table.add_row(vec![
                        segment.key.clone(),
                        segment.name.clone(),
                        segment.constraints.len().to_string(),
                    ]);
                }

                println!("{table}");
                Ok(())
            }
        }
    }

    pub fn changes(&self, changes: &[pb::FlagChange]) -> anyhow::Result<()> {
        let json = || Json::Array(changes.iter().map(Convert::change_to_json).collect());

        match self.format {
            Format::Json => Self::json(&json()),
            Format::Yaml => Self::yaml(&json()),
            Format::Table => {
                let mut table = Self::table(&["WHEN", "VERSION", "ACTOR", "ACTION", "TARGET"]);

                for change in changes {
                    table.add_row(vec![
                        change.created_at.clone(),
                        change.version.to_string(),
                        change.actor.clone(),
                        change.action.clone(),
                        format!("{}/{}", change.target_kind, change.target_key),
                    ]);
                }

                println!("{table}");
                Ok(())
            }
        }
    }

    pub fn evaluated(&self, flags: &[pb::EvaluatedFlag]) -> anyhow::Result<()> {
        let json = || Json::Array(flags.iter().map(Convert::evaluated_to_json).collect());

        match self.format {
            Format::Json => Self::json(&json()),
            Format::Yaml => Self::yaml(&json()),
            Format::Table => {
                let mut table =
                    Self::table(&["FLAG", "TYPE", "VALUE", "VARIANT", "REASON", "ERROR"]);

                for flag in flags {
                    let row = Convert::evaluated_to_json(flag);
                    let text = |field: &str| Self::compact(&row[field]);

                    table.add_row(vec![
                        text("flag_key"),
                        text("value_type"),
                        text("value"),
                        text("variant"),
                        text("reason"),
                        text("error_code"),
                    ]);
                }

                println!("{table}");
                Ok(())
            }
        }
    }
}
