use crate::cli::{Command, ConfigAction, FlagAction, RulesAction, SegmentAction, VariantAction};
use crate::client::{Api, EvalInput};
use crate::convert::Convert;
use crate::editor::Editor;
use crate::flag_config::{FlagDoc, SegmentsFile};
use crate::output::Output;
use crate::plan::{Diff, LiveState, Plan};
use crate::watch::SnapshotTracker;
use anyhow::{Context as _, bail};
use feature_flag_proto as pb;
use futures::StreamExt;
use serde_json::{Map, Value as Json};
use std::fs;
use std::path::Path;
use std::time::Duration;

pub struct ConfigDir<'a> {
    dir: &'a str,
}

impl<'a> ConfigDir<'a> {
    pub fn new(dir: &'a str) -> Self {
        Self { dir }
    }

    pub fn load(&self) -> anyhow::Result<(Vec<pb::Flag>, Vec<pb::Segment>)> {
        let flags_dir = Path::new(self.dir).join("flags");
        let mut flags = Vec::new();

        if flags_dir.is_dir() {
            let mut entries: Vec<_> = fs::read_dir(&flags_dir)
                .with_context(|| format!("reading {}", flags_dir.display()))?
                .collect::<Result<_, _>>()?;
            entries.sort_by_key(|entry| entry.path());

            for entry in entries {
                let path = entry.path();
                let is_yaml = matches!(
                    path.extension().and_then(|ext| ext.to_str()),
                    Some("yaml" | "yml")
                );

                if !is_yaml {
                    continue;
                }

                let text = fs::read_to_string(&path)?;
                let doc: FlagDoc = serde_yaml::from_str(&text)
                    .with_context(|| format!("parsing {}", path.display()))?;

                flags.push(
                    Convert::flag_doc_to_pb(&doc)
                        .with_context(|| format!("in {}", path.display()))?,
                );
            }
        }

        let mut segments = Vec::new();
        let segments_path = Path::new(self.dir).join("segments.yaml");

        if segments_path.is_file() {
            let text = fs::read_to_string(&segments_path)?;
            let file: SegmentsFile = serde_yaml::from_str(&text)
                .with_context(|| format!("parsing {}", segments_path.display()))?;

            for doc in &file.segments {
                segments.push(Convert::segment_doc_to_pb(doc)?);
            }
        }

        Ok((flags, segments))
    }

    pub fn export(&self, flags: &[pb::Flag], segments: &[pb::Segment]) -> anyhow::Result<()> {
        let flags_dir = Path::new(self.dir).join("flags");
        fs::create_dir_all(&flags_dir)?;

        for flag in flags {
            fs::write(
                flags_dir.join(format!("{}.yaml", flag.key)),
                Convert::flag_yaml(flag),
            )?;
        }

        let file = SegmentsFile {
            segments: segments.iter().map(Convert::segment_to_doc).collect(),
        };

        fs::write(
            Path::new(self.dir).join("segments.yaml"),
            serde_yaml::to_string(&file)?,
        )?;

        Ok(())
    }

    pub fn write_schema(&self) -> anyhow::Result<()> {
        let dir = Path::new(self.dir).join("schema");
        fs::create_dir_all(&dir)?;

        let schemas = [
            ("flag.json", schemars::schema_for!(FlagDoc)),
            ("segments.json", schemars::schema_for!(SegmentsFile)),
        ];

        for (name, schema) in schemas {
            let path = dir.join(name);
            fs::write(
                &path,
                format!("{}\n", serde_json::to_string_pretty(&schema)?),
            )?;
            eprintln!("wrote {}", path.display());
        }

        Ok(())
    }
}

pub struct Commands {
    api: Api,
    output: Output,
}

impl Commands {
    const WATCH_RETRY: Duration = Duration::from_secs(2);

    pub fn new(api: Api, output: Output) -> Self {
        Self { api, output }
    }

    pub async fn run(&self, command: Command) -> anyhow::Result<()> {
        match command {
            Command::Flag { action } => self.flag(action).await,
            Command::Variant { action } => self.variant(action).await,
            Command::Segment { action } => self.segment(action).await,
            Command::Rules { action } => self.rules(action).await,
            Command::Config { action } => self.config(action).await,
            Command::Audit { kind, key, limit } => {
                let changes = self.api.list_changes(&kind, &key, limit).await?;
                self.output.changes(&changes)
            }
            Command::Eval {
                targeting_key,
                flag,
                attrs,
                attributes,
            } => self.eval(targeting_key, flag, attrs, attributes).await,
            Command::Watch => self.watch().await,
            Command::Tui | Command::Login | Command::Logout | Command::Whoami => {
                bail!("command does not run against the api")
            }
        }
    }

    fn confirm(prompt: &str) -> anyhow::Result<bool> {
        use std::io::Write as _;

        print!("{prompt} [y/N]: ");
        std::io::stdout().flush()?;

        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;

        Ok(matches!(line.trim(), "y" | "Y" | "yes" | "Yes"))
    }

    async fn flag(&self, action: FlagAction) -> anyhow::Result<()> {
        match action {
            FlagAction::Create {
                key,
                r#type,
                enabled,
                default,
                variants,
            } => {
                let variants = variants
                    .iter()
                    .map(|spec| Convert::parse_variant(spec))
                    .collect::<anyhow::Result<_>>()?;

                let flag = self
                    .api
                    .create_flag(pb::CreateFlagRequest {
                        key,
                        value_type: pb::ValueType::from(r#type) as i32,
                        enabled,
                        default_variant_key: default,
                        variants,
                    })
                    .await?;

                self.output.flag(&flag)
            }
            FlagAction::Get { key } => self.output.flag(&self.api.get_flag(&key).await?),
            FlagAction::List { archived } => {
                self.output.flags(&self.api.list_flags(archived).await?)
            }
            FlagAction::Update {
                key,
                enabled,
                default,
            } => {
                let flag = self.api.update_flag(&key, enabled, &default).await?;
                self.output.flag(&flag)
            }
            FlagAction::Enable { key } => self.set_enabled(&key, true).await,
            FlagAction::Disable { key } => self.set_enabled(&key, false).await,
            FlagAction::Edit { key, yes } => self.edit_flag(&key, yes).await,
            FlagAction::Archive { key, restore } => {
                let flag = self.api.archive_flag(&key, !restore).await?;
                self.output.flag(&flag)
            }
            FlagAction::Delete { key } => {
                self.api.delete_flag(&key).await?;
                println!("ok");
                Ok(())
            }
        }
    }

    async fn set_enabled(&self, key: &str, enabled: bool) -> anyhow::Result<()> {
        let current = self.api.get_flag(key).await?;
        let flag = self
            .api
            .update_flag(key, enabled, &current.default_variant_key)
            .await?;

        self.output.flag(&flag)
    }

    async fn edit_flag(&self, key: &str, yes: bool) -> anyhow::Result<()> {
        let current = self.api.get_flag(key).await?;
        let before = Convert::flag_yaml(&current);
        let edited = Editor::edit(&before, key)?;
        let desired = Convert::flag_from_yaml(&edited)?;
        let after = Convert::flag_yaml(&desired);

        if before == after {
            println!("No changes.");
            return Ok(());
        }

        Diff::print(&before, &after);

        if !yes && !Self::confirm("Apply these changes?")? {
            println!("Aborted.");
            return Ok(());
        }

        let flag = self.api.apply_flag(Some(&current), &desired).await?;
        self.output.flag(&flag)
    }

    async fn variant(&self, action: VariantAction) -> anyhow::Result<()> {
        let flag = match action {
            VariantAction::Set {
                flag_key,
                variant_key,
                value,
            } => {
                let variant = pb::Variant {
                    key: variant_key,
                    value: Some(Convert::json_to_value(&Convert::parse_json(&value))),
                };

                self.api.upsert_variant(&flag_key, variant).await?
            }
            VariantAction::Delete {
                flag_key,
                variant_key,
            } => self.api.delete_variant(&flag_key, &variant_key).await?,
        };

        self.output.flag(&flag)
    }

    async fn segment(&self, action: SegmentAction) -> anyhow::Result<()> {
        match action {
            SegmentAction::Set { json } => {
                let segment = Convert::parse_segment(&Convert::parse_json(&json))?;
                self.output
                    .segment(&self.api.upsert_segment(segment).await?)
            }
            SegmentAction::Get { key } => self.output.segment(&self.api.get_segment(&key).await?),
            SegmentAction::List => self.output.segments(&self.api.list_segments().await?),
            SegmentAction::Edit { key, yes } => self.edit_segment(&key, yes).await,
            SegmentAction::Delete { key } => {
                self.api.delete_segment(&key).await?;
                println!("ok");
                Ok(())
            }
        }
    }

    async fn edit_segment(&self, key: &str, yes: bool) -> anyhow::Result<()> {
        let current = self.api.get_segment(key).await?;
        let before = Convert::segment_yaml(&current);
        let edited = Editor::edit(&before, key)?;
        let desired = Convert::segment_from_yaml(&edited)?;
        let after = Convert::segment_yaml(&desired);

        if before == after {
            println!("No changes.");
            return Ok(());
        }

        if desired.key != current.key {
            bail!("a segment's key cannot be changed; create a new segment instead");
        }

        Diff::print(&before, &after);

        if !yes && !Self::confirm("Apply these changes?")? {
            println!("Aborted.");
            return Ok(());
        }

        self.output
            .segment(&self.api.upsert_segment(desired).await?)
    }

    async fn rules(&self, action: RulesAction) -> anyhow::Result<()> {
        let RulesAction::Set { flag_key, json } = action;

        let Json::Array(items) = Convert::parse_json(&json) else {
            bail!("rules must be a JSON array");
        };

        let rules = items
            .iter()
            .map(Convert::parse_rule)
            .collect::<anyhow::Result<_>>()?;

        self.output
            .flag(&self.api.set_flag_rules(&flag_key, rules).await?)
    }

    async fn config(&self, action: ConfigAction) -> anyhow::Result<()> {
        match action {
            ConfigAction::Plan { dir } => {
                self.plan(&dir).await?;
                Ok(())
            }
            ConfigAction::Apply { dir, auto_approve } => {
                let (flags, segments, plan) = self.plan(&dir).await?;

                if plan.changes.is_empty() {
                    return Ok(());
                }

                if !auto_approve && !Self::confirm("Apply these changes?")? {
                    println!("Aborted.");
                    return Ok(());
                }

                let applied = self
                    .api
                    .apply_config(pb::ApplyConfigRequest {
                        flags,
                        segments,
                        dry_run: false,
                        expected_version: plan.from_version,
                    })
                    .await
                    .context("apply failed (config may have drifted; re-run plan)")?;

                println!(
                    "Applied {} change(s); config version {} -> {}.",
                    applied.changes.len(),
                    applied.from_version,
                    applied.to_version
                );

                Ok(())
            }
            ConfigAction::Export { dir } => {
                let flags = self.api.list_flags(false).await?;
                let segments = self.api.list_segments().await?;

                ConfigDir::new(&dir).export(&flags, &segments)?;

                println!(
                    "Wrote {} flag(s) and {} segment(s) to {dir}/.",
                    flags.len(),
                    segments.len()
                );

                Ok(())
            }
            ConfigAction::Schema { dir } => ConfigDir::new(&dir).write_schema(),
        }
    }

    async fn plan(
        &self,
        dir: &str,
    ) -> anyhow::Result<(Vec<pb::Flag>, Vec<pb::Segment>, pb::ApplyConfigResponse)> {
        let (flags, segments) = ConfigDir::new(dir).load()?;

        let plan = self
            .api
            .apply_config(pb::ApplyConfigRequest {
                flags: flags.clone(),
                segments: segments.clone(),
                dry_run: true,
                expected_version: 0,
            })
            .await?;

        let live = LiveState::fetch(&self.api).await?;

        Plan {
            changes: &plan.changes,
            desired_flags: &flags,
            desired_segments: &segments,
            live: &live,
        }
        .render();

        Ok((flags, segments, plan))
    }

    pub fn eval_attributes(
        attrs: &[String],
        attributes: Option<&str>,
    ) -> anyhow::Result<Map<String, Json>> {
        let mut merged = Map::new();

        if let Some(text) = attributes.filter(|text| !text.trim().is_empty()) {
            let Json::Object(fields) =
                serde_json::from_str::<Json>(text).context("attributes must be valid JSON")?
            else {
                bail!("attributes must be a JSON object");
            };

            merged.extend(fields);
        }

        for attr in attrs {
            let (key, value) = attr
                .split_once('=')
                .with_context(|| format!("attribute must be key=value, got `{attr}`"))?;

            merged.insert(key.to_owned(), Convert::parse_json(value));
        }

        Ok(merged)
    }

    async fn eval(
        &self,
        targeting_key: String,
        flag: Option<String>,
        attrs: Vec<String>,
        attributes: Option<String>,
    ) -> anyhow::Result<()> {
        let input = EvalInput {
            targeting_key,
            attributes: Self::eval_attributes(&attrs, attributes.as_deref())?,
        };

        let evaluated = match flag {
            Some(key) => vec![self.api.resolve(&key, &input).await?],
            None => self.api.resolve_all(&input).await?,
        };

        self.output.evaluated(&evaluated)
    }

    async fn watch(&self) -> anyhow::Result<()> {
        let mut tracker = SnapshotTracker::default();

        loop {
            let mut stream = match self.api.stream_snapshot().await {
                Ok(stream) => stream,
                Err(e) => {
                    eprintln!("{} stream unavailable: {e}", Self::now());
                    tokio::time::sleep(Self::WATCH_RETRY).await;
                    continue;
                }
            };

            while let Some(message) = stream.next().await {
                match message {
                    Ok(snapshot) => {
                        println!("{} {}", Self::now(), tracker.observe(snapshot).summary())
                    }
                    Err(status) => {
                        eprintln!("{} stream error: {}", Self::now(), status.message());
                        break;
                    }
                }
            }

            eprintln!("{} stream closed, reconnecting", Self::now());
            tokio::time::sleep(Self::WATCH_RETRY).await;
        }
    }

    fn now() -> String {
        chrono::Local::now().format("%H:%M:%S").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn attributes_merge_the_json_object_and_key_value_pairs() {
        let attrs = vec!["plan=pro".to_owned(), "age=42".to_owned()];
        let merged =
            Commands::eval_attributes(&attrs, Some(r#"{"plan":"free","beta":true}"#)).unwrap();

        assert_eq!(merged["plan"], json!("pro"));
        assert_eq!(merged["age"], json!(42));
        assert_eq!(merged["beta"], json!(true));
    }

    #[test]
    fn attributes_must_be_an_object() {
        assert!(Commands::eval_attributes(&[], Some("[1,2]")).is_err());
        assert!(Commands::eval_attributes(&["novalue".to_owned()], None).is_err());
        assert!(Commands::eval_attributes(&[], None).unwrap().is_empty());
    }

    #[test]
    fn repository_config_round_trips_through_the_wire_types() {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../platform-services/feature-flags/config"
        );
        let (flags, segments) = ConfigDir::new(dir).load().unwrap();

        assert!(!flags.is_empty());

        for flag in &flags {
            let reparsed = Convert::flag_from_yaml(&Convert::flag_yaml(flag)).unwrap();
            assert_eq!(&reparsed, flag);
        }

        for segment in &segments {
            let reparsed = Convert::segment_from_yaml(&Convert::segment_yaml(segment)).unwrap();
            assert_eq!(&reparsed, segment);
        }
    }
}
