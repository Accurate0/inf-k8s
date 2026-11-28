use crate::convert::Convert;
use crate::graphql as gql;
use anyhow::{Context as _, bail};
use cynic::{MutationBuilder, QueryBuilder, SubscriptionBuilder};
use feature_flag_proto as pb;
use futures::{Stream, StreamExt};
use graph_client::GraphClient;
use serde_json::{Map, Value as Json};
use std::pin::Pin;

#[derive(Debug, Clone, Default)]
pub struct EvalInput {
    pub targeting_key: String,
    pub attributes: Map<String, Json>,
}

impl EvalInput {
    fn context(&self) -> gql::ContextInput {
        gql::ContextInput {
            targeting_key: self.targeting_key.clone(),
            attributes: Some(Json::Object(self.attributes.clone())),
        }
    }
}

pub type Snapshots = Pin<Box<dyn Stream<Item = anyhow::Result<pb::SnapshotResponse>> + Send>>;

/// The feature-flags admin and evaluation API, through the graph; it speaks the gRPC wire types
/// so the commands and TUI don't care which transport carries them.
#[derive(Clone)]
pub struct Api {
    graph: GraphClient,
    user: String,
}

impl Api {
    pub fn new(graph: GraphClient, user: String) -> Self {
        Self { graph, user }
    }

    pub fn url(&self) -> &str {
        self.graph.url()
    }

    pub fn user(&self) -> String {
        self.user.clone()
    }

    /// The graph client refreshes its session before every request.
    pub async fn keep_fresh(&self) -> anyhow::Result<()> {
        Ok(())
    }

    pub async fn list_flags(&self, include_archived: bool) -> anyhow::Result<Vec<pb::Flag>> {
        let mut flags: Vec<pb::Flag> = self
            .graph
            .run(gql::FlagsQuery::build(gql::FlagsVariables {
                include_archived,
            }))
            .await?
            .feature_flags
            .context("the graph returned no featureFlags")?
            .into_iter()
            .map(flag)
            .collect();
        flags.sort_by(|a, b| a.key.cmp(&b.key));

        Ok(flags)
    }

    pub async fn get_flag(&self, key: &str) -> anyhow::Result<pb::Flag> {
        let found = self
            .graph
            .run(gql::FlagQuery::build(gql::KeyVariables {
                key: key.to_owned(),
            }))
            .await?
            .feature_flag
            .context("the graph returned no featureFlag")?;

        Ok(flag(found))
    }

    pub async fn create_flag(&self, request: pb::CreateFlagRequest) -> anyhow::Result<pb::Flag> {
        let input = gql::CreateFlagInput {
            key: request.key,
            value_type: value_type_in(request.value_type)?,
            enabled: request.enabled,
            default_variant_key: request.default_variant_key,
            variants: request.variants.iter().map(variant_in).collect(),
        };
        let created = self
            .graph
            .run(gql::CreateFlag::build(gql::CreateFlagVariables { input }))
            .await?
            .feature_flags_create_flag;

        Ok(flag(created))
    }

    pub async fn update_flag(
        &self,
        key: &str,
        enabled: bool,
        default_variant_key: &str,
    ) -> anyhow::Result<pb::Flag> {
        let updated = self
            .graph
            .run(gql::UpdateFlag::build(gql::UpdateFlagVariables {
                key: key.to_owned(),
                enabled,
                default_variant_key: default_variant_key.to_owned(),
            }))
            .await?
            .feature_flags_update_flag;

        Ok(flag(updated))
    }

    pub async fn archive_flag(&self, key: &str, archived: bool) -> anyhow::Result<pb::Flag> {
        let archived = self
            .graph
            .run(gql::ArchiveFlag::build(gql::ArchiveFlagVariables {
                key: key.to_owned(),
                archived,
            }))
            .await?
            .feature_flags_archive_flag;

        Ok(flag(archived))
    }

    /// Returns the graph id of what was deleted.
    pub async fn delete_flag(&self, key: &str) -> anyhow::Result<String> {
        let deleted = self
            .graph
            .run(gql::DeleteFlag::build(gql::KeyVariables {
                key: key.to_owned(),
            }))
            .await?
            .feature_flags_delete_flag;

        Ok(deleted.into_inner())
    }

    pub async fn upsert_variant(
        &self,
        flag_key: &str,
        variant: pb::Variant,
    ) -> anyhow::Result<pb::Flag> {
        let updated = self
            .graph
            .run(gql::UpsertVariant::build(gql::UpsertVariantVariables {
                flag_key: flag_key.to_owned(),
                variant: variant_in(&variant),
            }))
            .await?
            .feature_flags_upsert_variant;

        Ok(flag(updated))
    }

    pub async fn delete_variant(
        &self,
        flag_key: &str,
        variant_key: &str,
    ) -> anyhow::Result<pb::Flag> {
        let updated = self
            .graph
            .run(gql::DeleteVariant::build(gql::DeleteVariantVariables {
                flag_key: flag_key.to_owned(),
                variant_key: variant_key.to_owned(),
            }))
            .await?
            .feature_flags_delete_variant;

        Ok(flag(updated))
    }

    pub async fn set_flag_rules(
        &self,
        flag_key: &str,
        rules: Vec<pb::Rule>,
    ) -> anyhow::Result<pb::Flag> {
        let updated = self
            .graph
            .run(gql::SetFlagRules::build(gql::SetRulesVariables {
                flag_key: flag_key.to_owned(),
                rules: rules.iter().map(rule_in).collect::<anyhow::Result<_>>()?,
            }))
            .await?
            .feature_flags_set_flag_rules;

        Ok(flag(updated))
    }

    pub async fn list_segments(&self) -> anyhow::Result<Vec<pb::Segment>> {
        let mut segments: Vec<pb::Segment> = self
            .graph
            .run(gql::SegmentsQuery::build(()))
            .await?
            .feature_flags_segments
            .context("the graph returned no featureFlagsSegments")?
            .into_iter()
            .map(segment)
            .collect();
        segments.sort_by(|a, b| a.key.cmp(&b.key));

        Ok(segments)
    }

    pub async fn get_segment(&self, key: &str) -> anyhow::Result<pb::Segment> {
        let found = self
            .graph
            .run(gql::SegmentQuery::build(gql::KeyVariables {
                key: key.to_owned(),
            }))
            .await?
            .feature_flags_segment
            .context("the graph returned no featureFlagsSegment")?;

        Ok(segment(found))
    }

    pub async fn upsert_segment(&self, upserted: pb::Segment) -> anyhow::Result<pb::Segment> {
        let updated = self
            .graph
            .run(gql::UpdateSegment::build(gql::SegmentVariables {
                input: segment_in(&upserted)?,
            }))
            .await?
            .feature_flags_update_segment;

        Ok(segment(updated))
    }

    /// Returns the graph id of what was deleted.
    pub async fn delete_segment(&self, key: &str) -> anyhow::Result<String> {
        let deleted = self
            .graph
            .run(gql::DeleteSegment::build(gql::KeyVariables {
                key: key.to_owned(),
            }))
            .await?
            .feature_flags_delete_segment;

        Ok(deleted.into_inner())
    }

    pub async fn apply_config(
        &self,
        request: pb::ApplyConfigRequest,
    ) -> anyhow::Result<pb::ApplyConfigResponse> {
        let flags = request
            .flags
            .iter()
            .map(flag_in)
            .collect::<anyhow::Result<_>>()?;
        let segments = request
            .segments
            .iter()
            .map(segment_in)
            .collect::<anyhow::Result<_>>()?;
        let result = self
            .graph
            .run(gql::ApplyConfig::build(gql::ApplyConfigVariables {
                flags,
                segments,
                dry_run: request.dry_run,
                expected_version: i32::try_from(request.expected_version)?,
            }))
            .await?
            .feature_flags_apply_config;

        Ok(pb::ApplyConfigResponse {
            changes: result
                .changes
                .into_iter()
                .map(|change| pb::ConfigChange {
                    target_kind: change.target_kind,
                    target_key: change.target_key,
                    op: change.op as i32,
                    detail: detail(change.detail),
                })
                .collect(),
            from_version: result.from_version.into(),
            to_version: result.to_version.into(),
            applied: result.applied,
        })
    }

    pub async fn list_changes(
        &self,
        target_kind: &str,
        target_key: &str,
        limit: u32,
    ) -> anyhow::Result<Vec<pb::FlagChange>> {
        let given = |value: &str| (!value.is_empty()).then(|| value.to_owned());
        let changes = self
            .graph
            .run(gql::HistoryQuery::build(gql::HistoryVariables {
                target_kind: given(target_kind),
                target_key: given(target_key),
                limit: i32::try_from(limit)?,
            }))
            .await?
            .feature_flags_history
            .context("the graph returned no featureFlagsHistory")?;

        Ok(changes
            .into_iter()
            .map(|change| pb::FlagChange {
                id: change.id.into_inner(),
                version: change.version.into(),
                actor: change.actor,
                action: change.action,
                target_kind: change.target_kind,
                target_key: change.target_key,
                detail: detail(change.detail),
                created_at: change.created_at.to_rfc3339(),
            })
            .collect())
    }

    pub async fn apply_flag(
        &self,
        current: Option<&pb::Flag>,
        desired: &pb::Flag,
    ) -> anyhow::Result<pb::Flag> {
        let Some(current) = current else {
            let created = self
                .create_flag(pb::CreateFlagRequest {
                    key: desired.key.clone(),
                    value_type: desired.value_type,
                    enabled: desired.enabled,
                    default_variant_key: desired.default_variant_key.clone(),
                    variants: desired.variants.clone(),
                })
                .await?;

            if desired.rules.is_empty() {
                return Ok(created);
            }

            return self
                .set_flag_rules(&desired.key, desired.rules.clone())
                .await;
        };

        if current.key != desired.key {
            bail!("a flag's key cannot be changed; create a new flag instead");
        }

        if current.value_type != desired.value_type {
            bail!("a flag's type cannot be changed; create a new flag instead");
        }

        for variant in &desired.variants {
            let unchanged = current.variants.iter().any(|existing| existing == variant);

            if !unchanged {
                self.upsert_variant(&desired.key, variant.clone()).await?;
            }
        }

        if Self::unranked(&current.rules) != Self::unranked(&desired.rules) {
            self.set_flag_rules(&desired.key, desired.rules.clone())
                .await?;
        }

        let settings_changed = current.enabled != desired.enabled
            || current.default_variant_key != desired.default_variant_key;

        if settings_changed {
            self.update_flag(&desired.key, desired.enabled, &desired.default_variant_key)
                .await?;
        }

        for variant in &current.variants {
            let kept = desired
                .variants
                .iter()
                .any(|wanted| wanted.key == variant.key);

            if !kept {
                self.delete_variant(&desired.key, &variant.key).await?;
            }
        }

        self.get_flag(&desired.key).await
    }

    fn unranked(rules: &[pb::Rule]) -> Vec<pb::Rule> {
        rules
            .iter()
            .cloned()
            .map(|mut rule| {
                rule.rank = 0;
                rule
            })
            .collect()
    }

    pub async fn resolve_all(&self, input: &EvalInput) -> anyhow::Result<Vec<pb::EvaluatedFlag>> {
        let mut flags: Vec<pb::EvaluatedFlag> = self
            .graph
            .run(gql::EvaluateAllQuery::build(gql::EvaluateAllVariables {
                context: input.context(),
            }))
            .await?
            .feature_flags_evaluate_all
            .context("the graph returned no featureFlagsEvaluateAll")?
            .into_iter()
            .map(evaluated)
            .collect();
        flags.sort_by(|a, b| a.flag_key.cmp(&b.flag_key));

        Ok(flags)
    }

    pub async fn resolve(
        &self,
        flag_key: &str,
        input: &EvalInput,
    ) -> anyhow::Result<pb::EvaluatedFlag> {
        let evaluation = self
            .graph
            .run(gql::EvaluateQuery::build(gql::EvaluateVariables {
                flag_key: flag_key.to_owned(),
                context: input.context(),
            }))
            .await?
            .feature_flags_evaluate
            .context("the graph returned no featureFlagsEvaluate")?;

        Ok(evaluated(evaluation))
    }

    async fn snapshot(&self) -> anyhow::Result<pb::SnapshotResponse> {
        let snapshot = self.graph.run(gql::SnapshotQuery::build(())).await?;

        Ok(pb::SnapshotResponse {
            version: snapshot
                .feature_flags_config_version
                .context("the graph returned no featureFlagsConfigVersion")?
                .into(),
            flags: snapshot
                .feature_flags
                .context("the graph returned no featureFlags")?
                .into_iter()
                .map(flag)
                .collect(),
            segments: snapshot
                .feature_flags_segments
                .context("the graph returned no featureFlagsSegments")?
                .into_iter()
                .map(segment)
                .collect(),
        })
    }

    /// A snapshot when the stream is ready, then a fresh one whenever the config version moves
    /// or the server says events were dropped.
    pub async fn stream_snapshot(&self) -> anyhow::Result<Snapshots> {
        let events = self
            .graph
            .subscribe(gql::ChangesSubscription::build(()))
            .await
            .context("subscribing to feature flag changes")?;
        let api = self.clone();

        let snapshots = futures::stream::unfold(
            (events, api, None::<i32>),
            |(mut events, api, mut seen)| async move {
                loop {
                    let event = match events.next().await? {
                        Ok(event) => event.feature_flags_changes,
                        Err(e) => return Some((Err(e), (events, api, seen))),
                    };

                    let forced = matches!(
                        event.event_type,
                        gql::EventType::Ready | gql::EventType::Resync
                    );
                    if !forced && seen == Some(event.config_version) {
                        continue;
                    }

                    seen = Some(event.config_version);
                    let snapshot = api.snapshot().await;
                    return Some((snapshot, (events, api, seen)));
                }
            },
        );

        Ok(Box::pin(snapshots))
    }
}

fn detail(detail: Json) -> String {
    match detail {
        Json::String(text) => text,
        other => other.to_string(),
    }
}

fn value(json: Json) -> Option<prost_types::Value> {
    Some(Convert::json_to_value(&json))
}

fn json(value: Option<&prost_types::Value>) -> Json {
    value.map(Convert::value_to_json).unwrap_or(Json::Null)
}

fn flag(flag: gql::Flag) -> pb::Flag {
    pb::Flag {
        key: flag.key,
        value_type: flag.value_type as i32,
        enabled: flag.enabled,
        default_variant_key: flag.default_variant_key,
        archived: flag.archived,
        variants: flag
            .variants
            .into_iter()
            .map(|variant| pb::Variant {
                key: variant.key,
                value: value(variant.value),
            })
            .collect(),
        rules: flag.rules.into_iter().map(rule).collect(),
    }
}

fn rule(rule: gql::Rule) -> pb::Rule {
    pb::Rule {
        rank: u32::try_from(rule.rank).unwrap_or_default(),
        segment_key: rule.segment_key.unwrap_or_default(),
        variant_key: rule.variant_key.unwrap_or_default(),
        distributions: rule
            .distributions
            .into_iter()
            .map(|d| pb::Distribution {
                variant_key: d.variant_key,
                weight: u32::try_from(d.weight).unwrap_or_default(),
            })
            .collect(),
        constraint_groups: rule
            .constraint_groups
            .into_iter()
            .map(|group| pb::ConstraintGroup {
                constraints: group.into_iter().map(constraint).collect(),
            })
            .collect(),
        bucket_salt: rule.bucket_salt,
    }
}

fn constraint(constraint: gql::Constraint) -> pb::Constraint {
    pb::Constraint {
        attribute: constraint.attribute,
        operator: constraint.operator as i32,
        values: constraint
            .values
            .into_iter()
            .map(|v| Convert::json_to_value(&v))
            .collect(),
    }
}

fn segment(segment: gql::Segment) -> pb::Segment {
    pb::Segment {
        key: segment.key,
        name: segment.name,
        constraints: segment.constraints.into_iter().map(constraint).collect(),
    }
}

fn evaluated(evaluation: gql::Evaluation) -> pb::EvaluatedFlag {
    pb::EvaluatedFlag {
        flag_key: evaluation.flag_key,
        value_type: evaluation.value_type as i32,
        value: value(evaluation.value),
        meta: Some(pb::ResolutionMeta {
            variant: evaluation.variant.unwrap_or_default(),
            reason: evaluation.reason as i32,
            error_code: evaluation.error_code.unwrap_or_default(),
            error_message: evaluation.error_message.unwrap_or_default(),
        }),
    }
}

fn value_type_in(value_type: i32) -> anyhow::Result<gql::ValueType> {
    Ok(match pb::ValueType::try_from(value_type)? {
        pb::ValueType::Unspecified => gql::ValueType::Unspecified,
        pb::ValueType::Boolean => gql::ValueType::Boolean,
        pb::ValueType::String => gql::ValueType::String,
        pb::ValueType::Integer => gql::ValueType::Integer,
        pb::ValueType::Float => gql::ValueType::Float,
        pb::ValueType::Object => gql::ValueType::Object,
    })
}

fn operator_in(operator: i32) -> anyhow::Result<gql::Operator> {
    use pb::ConstraintOperator as Op;

    Ok(match Op::try_from(operator)? {
        Op::Unspecified => gql::Operator::Unspecified,
        Op::Eq => gql::Operator::Eq,
        Op::Neq => gql::Operator::Neq,
        Op::In => gql::Operator::In,
        Op::NotIn => gql::Operator::NotIn,
        Op::Contains => gql::Operator::Contains,
        Op::StartsWith => gql::Operator::StartsWith,
        Op::EndsWith => gql::Operator::EndsWith,
        Op::Gt => gql::Operator::Gt,
        Op::Gte => gql::Operator::Gte,
        Op::Lt => gql::Operator::Lt,
        Op::Lte => gql::Operator::Lte,
        Op::Exists => gql::Operator::Exists,
        Op::Regex => gql::Operator::Regex,
        Op::FlagMatches => gql::Operator::FlagMatches,
    })
}

fn variant_in(variant: &pb::Variant) -> gql::VariantInput {
    gql::VariantInput {
        key: variant.key.clone(),
        value: json(variant.value.as_ref()),
    }
}

fn constraint_in(constraint: &pb::Constraint) -> anyhow::Result<gql::ConstraintInput> {
    Ok(gql::ConstraintInput {
        attribute: constraint.attribute.clone(),
        operator: operator_in(constraint.operator)?,
        values: constraint
            .values
            .iter()
            .map(Convert::value_to_json)
            .collect(),
    })
}

fn rule_in(rule: &pb::Rule) -> anyhow::Result<gql::RuleInput> {
    let given = |value: &str| (!value.is_empty()).then(|| value.to_owned());

    Ok(gql::RuleInput {
        rank: i32::try_from(rule.rank)?,
        segment_key: given(&rule.segment_key),
        variant_key: given(&rule.variant_key),
        distributions: rule
            .distributions
            .iter()
            .map(|d| {
                Ok(gql::DistributionInput {
                    variant_key: d.variant_key.clone(),
                    weight: i32::try_from(d.weight)?,
                })
            })
            .collect::<anyhow::Result<_>>()?,
        constraint_groups: rule
            .constraint_groups
            .iter()
            .map(|group| group.constraints.iter().map(constraint_in).collect())
            .collect::<anyhow::Result<_>>()?,
        bucket_salt: rule.bucket_salt.clone(),
    })
}

fn flag_in(flag: &pb::Flag) -> anyhow::Result<gql::FlagInput> {
    Ok(gql::FlagInput {
        key: flag.key.clone(),
        value_type: value_type_in(flag.value_type)?,
        enabled: flag.enabled,
        default_variant_key: flag.default_variant_key.clone(),
        archived: flag.archived,
        variants: flag.variants.iter().map(variant_in).collect(),
        rules: flag
            .rules
            .iter()
            .map(rule_in)
            .collect::<anyhow::Result<_>>()?,
    })
}

fn segment_in(segment: &pb::Segment) -> anyhow::Result<gql::SegmentInput> {
    Ok(gql::SegmentInput {
        key: segment.key.clone(),
        name: segment.name.clone(),
        constraints: segment
            .constraints
            .iter()
            .map(constraint_in)
            .collect::<anyhow::Result<_>>()?,
    })
}
