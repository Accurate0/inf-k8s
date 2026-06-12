//! Postgres persistence using compile-time-checked `sqlx` queries. Reads assemble
//! the full [`Snapshot`]; writes are the gRPC Admin surface. The checked-in `.sqlx`
//! cache lets CI build with `SQLX_OFFLINE=true` (no database).

mod types;

use crate::error::{AppError, AppResult};
use crate::model::{
    Constraint, ConstraintGroup, Distribution, Flag, Rule, Segment, Snapshot, ValueType, Variant,
};
use serde_json::Value as Json;
use sqlx::PgPool;
use std::collections::{BTreeMap, HashMap, HashSet};
use types::{
    json_array, operator_from_str, operator_to_str, unique_violation, value_type_from_str,
    value_type_to_str,
};
use uuid::Uuid;

#[derive(Clone)]
pub struct Store {
    pool: PgPool,
}

/// One row of the `flag_changes` audit log, surfaced read-only to the admin UI.
pub struct FlagChange {
    pub id: Uuid,
    pub version: i64,
    pub actor: String,
    pub action: String,
    pub target_kind: String,
    pub target_key: String,
    pub detail: Json,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Upper bound on audit rows returned in a single request, regardless of the
/// caller-supplied limit, to keep responses bounded.
const MAX_CHANGES: i64 = 500;

impl Store {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn config_version(&self) -> AppResult<i64> {
        Ok(
            sqlx::query_scalar!("SELECT version FROM config_version WHERE id")
                .fetch_one(&self.pool)
                .await?,
        )
    }

    pub async fn load_snapshot(&self) -> AppResult<Snapshot> {
        let version = self.config_version().await?;

        let mut flags: HashMap<Uuid, Flag> = HashMap::new();

        for row in sqlx::query!(
            "SELECT id, key, value_type, enabled, default_variant_key, archived FROM flags"
        )
        .fetch_all(&self.pool)
        .await?
        {
            flags.insert(
                row.id,
                Flag {
                    key: row.key,
                    value_type: value_type_from_str(&row.value_type)?,
                    enabled: row.enabled,
                    default_variant_key: row.default_variant_key,
                    archived: row.archived,
                    variants: Vec::new(),
                    rules: Vec::new(),
                },
            );
        }

        for row in sqlx::query!(r#"SELECT flag_id, key, value as "value: Json" FROM variants"#)
            .fetch_all(&self.pool)
            .await?
        {
            if let Some(flag) = flags.get_mut(&row.flag_id) {
                flag.variants.push(Variant {
                    key: row.key,
                    value: row.value,
                });
            }
        }

        let mut distributions: HashMap<Uuid, Vec<Distribution>> = HashMap::new();
        for row in sqlx::query!("SELECT rule_id, variant_key, weight FROM rule_distributions")
            .fetch_all(&self.pool)
            .await?
        {
            distributions
                .entry(row.rule_id)
                .or_default()
                .push(Distribution {
                    variant_key: row.variant_key,
                    weight: row.weight as u32,
                });
        }

        // Constraints are grouped by `group_index` within a rule: rows sharing an
        // index form one OR-group, and the groups are AND-combined (CNF).
        let mut rule_groups: HashMap<Uuid, BTreeMap<i32, Vec<Constraint>>> = HashMap::new();
        for row in sqlx::query!(
            r#"SELECT rule_id, group_index, attribute, operator, values as "values: Json" FROM rule_constraints"#
        )
        .fetch_all(&self.pool)
        .await?
        {
            rule_groups
                .entry(row.rule_id)
                .or_default()
                .entry(row.group_index)
                .or_default()
                .push(Constraint {
                    attribute: row.attribute,
                    operator: operator_from_str(&row.operator)?,
                    values: json_array(row.values),
                });
        }

        for row in sqlx::query!(
            "SELECT id, flag_id, rank, segment_key, variant_key, bucket_salt FROM flag_rules ORDER BY rank"
        )
        .fetch_all(&self.pool)
        .await?
        {
            if let Some(flag) = flags.get_mut(&row.flag_id) {
                flag.rules.push(Rule {
                    rank: row.rank as u32,
                    segment_key: row.segment_key,
                    variant_key: row.variant_key,
                    bucket_salt: row.bucket_salt,
                    distributions: distributions.remove(&row.id).unwrap_or_default(),
                    constraint_groups: rule_groups
                        .remove(&row.id)
                        .unwrap_or_default()
                        .into_values()
                        .map(|constraints| ConstraintGroup { constraints })
                        .collect(),
                });
            }
        }

        let mut segments: HashMap<Uuid, Segment> = HashMap::new();
        for row in sqlx::query!("SELECT id, key, name FROM segments")
            .fetch_all(&self.pool)
            .await?
        {
            segments.insert(
                row.id,
                Segment {
                    key: row.key,
                    name: row.name,
                    constraints: Vec::new(),
                },
            );
        }

        for row in sqlx::query!(
            r#"SELECT segment_id, attribute, operator, values as "values: Json" FROM segment_constraints"#
        )
        .fetch_all(&self.pool)
        .await?
        {
            if let Some(segment) = segments.get_mut(&row.segment_id) {
                segment.constraints.push(Constraint {
                    attribute: row.attribute,
                    operator: operator_from_str(&row.operator)?,
                    values: json_array(row.values),
                });
            }
        }

        Ok(Snapshot {
            version,
            flags: flags.into_values().map(|f| (f.key.clone(), f)).collect(),
            segments: segments.into_values().map(|s| (s.key.clone(), s)).collect(),
        })
    }

    /// Append an audit row inside the caller's transaction, stamping the version the
    /// bump trigger has already advanced to.
    async fn record_change(
        tx: &mut sqlx::PgConnection,
        actor: &str,
        action: &str,
        target_kind: &str,
        target_key: &str,
        detail: Json,
    ) -> AppResult<()> {
        sqlx::query!(
            "INSERT INTO flag_changes (version, actor, action, target_kind, target_key, detail) \
             VALUES ((SELECT version FROM config_version), $1, $2, $3, $4, $5)",
            actor,
            action,
            target_kind,
            target_key,
            detail,
        )
        .execute(&mut *tx)
        .await?;
        Ok(())
    }

    /// Read the audit log newest-first, optionally filtered to a single target. Empty
    /// `target_kind`/`target_key` match any value; `limit` is clamped to [`MAX_CHANGES`].
    pub async fn list_changes(
        &self,
        target_kind: &str,
        target_key: &str,
        limit: i64,
    ) -> AppResult<Vec<FlagChange>> {
        let limit = if limit <= 0 {
            MAX_CHANGES
        } else {
            limit.min(MAX_CHANGES)
        };
        let kind = (!target_kind.is_empty()).then(|| target_kind.to_owned());
        let key = (!target_key.is_empty()).then(|| target_key.to_owned());
        let rows = sqlx::query_as!(
            FlagChange,
            r#"SELECT id, version, actor, action, target_kind, target_key,
                      detail as "detail: Json", created_at
               FROM flag_changes
               WHERE ($1::text IS NULL OR target_kind = $1)
                 AND ($2::text IS NULL OR target_key = $2)
               ORDER BY created_at DESC
               LIMIT $3"#,
            kind,
            key,
            limit,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn variant_keys(&self, flag_id: Uuid) -> AppResult<HashSet<String>> {
        let keys = sqlx::query_scalar!("SELECT key FROM variants WHERE flag_id = $1", flag_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(keys.into_iter().collect())
    }

    pub async fn flag_id(&self, key: &str) -> AppResult<Uuid> {
        sqlx::query_scalar!("SELECT id FROM flags WHERE key = $1", key)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("flag `{key}`")))
    }

    pub async fn create_flag(
        &self,
        actor: &str,
        key: &str,
        value_type: ValueType,
        enabled: bool,
        default_variant_key: &str,
        variants: &[Variant],
    ) -> AppResult<()> {
        if variants.iter().all(|v| v.key != default_variant_key) {
            return Err(AppError::Invalid(format!(
                "default variant `{default_variant_key}` is not among the provided variants"
            )));
        }
        for v in variants {
            check_variant_type(value_type, v)?;
        }

        let mut tx = self.pool.begin().await?;
        let flag_id = sqlx::query_scalar!(
            "INSERT INTO flags (key, value_type, enabled, default_variant_key) \
             VALUES ($1, $2, $3, $4) RETURNING id",
            key,
            value_type_to_str(value_type),
            enabled,
            default_variant_key,
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(unique_violation(format!("flag `{key}` already exists")))?;

        for v in variants {
            sqlx::query!(
                "INSERT INTO variants (flag_id, key, value) VALUES ($1, $2, $3)",
                flag_id,
                v.key,
                v.value,
            )
            .execute(&mut *tx)
            .await?;
        }
        Self::record_change(
            &mut tx,
            actor,
            "create_flag",
            "flag",
            key,
            serde_json::json!({
                "value_type": value_type_to_str(value_type),
                "enabled": enabled,
                "default_variant_key": default_variant_key,
            }),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn update_flag(
        &self,
        actor: &str,
        key: &str,
        enabled: bool,
        default_variant_key: &str,
    ) -> AppResult<()> {
        let flag_id = self.flag_id(key).await?;
        if !self
            .variant_keys(flag_id)
            .await?
            .contains(default_variant_key)
        {
            return Err(AppError::Invalid(format!(
                "default variant `{default_variant_key}` is not among flag `{key}`'s variants"
            )));
        }
        let mut tx = self.pool.begin().await?;
        let affected = sqlx::query!(
            "UPDATE flags SET enabled = $2, default_variant_key = $3, updated_at = now() \
             WHERE key = $1",
            key,
            enabled,
            default_variant_key,
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if affected == 0 {
            return Err(AppError::NotFound(format!("flag `{key}`")));
        }
        Self::record_change(
            &mut tx,
            actor,
            "update_flag",
            "flag",
            key,
            serde_json::json!({ "enabled": enabled, "default_variant_key": default_variant_key }),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn archive_flag(&self, actor: &str, key: &str, archived: bool) -> AppResult<()> {
        let mut tx = self.pool.begin().await?;
        let affected = sqlx::query!(
            "UPDATE flags SET archived = $2, updated_at = now() WHERE key = $1",
            key,
            archived,
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if affected == 0 {
            return Err(AppError::NotFound(format!("flag `{key}`")));
        }
        Self::record_change(
            &mut tx,
            actor,
            if archived {
                "archive_flag"
            } else {
                "unarchive_flag"
            },
            "flag",
            key,
            serde_json::json!({ "archived": archived }),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete_flag(&self, actor: &str, key: &str) -> AppResult<()> {
        let mut tx = self.pool.begin().await?;
        let affected = sqlx::query!("DELETE FROM flags WHERE key = $1", key)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if affected == 0 {
            return Err(AppError::NotFound(format!("flag `{key}`")));
        }
        Self::record_change(&mut tx, actor, "delete_flag", "flag", key, Json::Null).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn upsert_variant(
        &self,
        actor: &str,
        flag_key: &str,
        variant: &Variant,
    ) -> AppResult<()> {
        let flag = sqlx::query!("SELECT id, value_type FROM flags WHERE key = $1", flag_key)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("flag `{flag_key}`")))?;
        check_variant_type(value_type_from_str(&flag.value_type)?, variant)?;
        let flag_id = flag.id;
        let mut tx = self.pool.begin().await?;
        sqlx::query!(
            "INSERT INTO variants (flag_id, key, value) VALUES ($1, $2, $3) \
             ON CONFLICT (flag_id, key) DO UPDATE SET value = EXCLUDED.value",
            flag_id,
            variant.key,
            variant.value,
        )
        .execute(&mut *tx)
        .await?;
        Self::record_change(
            &mut tx,
            actor,
            "upsert_variant",
            "flag",
            flag_key,
            serde_json::json!({ "variant_key": variant.key, "value": variant.value }),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete_variant(
        &self,
        actor: &str,
        flag_key: &str,
        variant_key: &str,
    ) -> AppResult<()> {
        let flag_id = self.flag_id(flag_key).await?;
        let mut tx = self.pool.begin().await?;
        sqlx::query!(
            "DELETE FROM variants WHERE flag_id = $1 AND key = $2",
            flag_id,
            variant_key,
        )
        .execute(&mut *tx)
        .await?;
        Self::record_change(
            &mut tx,
            actor,
            "delete_variant",
            "flag",
            flag_key,
            serde_json::json!({ "variant_key": variant_key }),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn upsert_segment(&self, actor: &str, segment: &Segment) -> AppResult<()> {
        let mut tx = self.pool.begin().await?;
        let segment_id = sqlx::query_scalar!(
            "INSERT INTO segments (key, name) VALUES ($1, $2) \
             ON CONFLICT (key) DO UPDATE SET name = EXCLUDED.name RETURNING id",
            segment.key,
            segment.name,
        )
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query!(
            "DELETE FROM segment_constraints WHERE segment_id = $1",
            segment_id
        )
        .execute(&mut *tx)
        .await?;

        for c in &segment.constraints {
            sqlx::query!(
                "INSERT INTO segment_constraints (segment_id, attribute, operator, values) \
                 VALUES ($1, $2, $3, $4)",
                segment_id,
                c.attribute,
                operator_to_str(c.operator),
                Json::Array(c.values.clone()),
            )
            .execute(&mut *tx)
            .await?;
        }
        Self::record_change(
            &mut tx,
            actor,
            "upsert_segment",
            "segment",
            &segment.key,
            serde_json::json!({ "name": segment.name, "constraints": segment.constraints.len() }),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete_segment(&self, actor: &str, key: &str) -> AppResult<()> {
        let mut tx = self.pool.begin().await?;
        let affected = sqlx::query!("DELETE FROM segments WHERE key = $1", key)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if affected == 0 {
            return Err(AppError::NotFound(format!("segment `{key}`")));
        }
        Self::record_change(&mut tx, actor, "delete_segment", "segment", key, Json::Null).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn set_flag_rules(
        &self,
        actor: &str,
        flag_key: &str,
        rules: &[Rule],
    ) -> AppResult<()> {
        let flag_id = self.flag_id(flag_key).await?;
        validate_rules(flag_key, rules, &self.variant_keys(flag_id).await?)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query!("DELETE FROM flag_rules WHERE flag_id = $1", flag_id)
            .execute(&mut *tx)
            .await?;

        for (rank, rule) in rules.iter().enumerate() {
            let rule_id = sqlx::query_scalar!(
                "INSERT INTO flag_rules (flag_id, rank, segment_key, variant_key, bucket_salt) \
                 VALUES ($1, $2, $3, $4, $5) RETURNING id",
                flag_id,
                rank as i32,
                rule.segment_key.as_deref(),
                rule.variant_key.as_deref(),
                rule.bucket_salt,
            )
            .fetch_one(&mut *tx)
            .await?;

            for d in &rule.distributions {
                sqlx::query!(
                    "INSERT INTO rule_distributions (rule_id, variant_key, weight) \
                     VALUES ($1, $2, $3)",
                    rule_id,
                    d.variant_key,
                    d.weight as i32,
                )
                .execute(&mut *tx)
                .await?;
            }

            for (group_index, group) in rule.constraint_groups.iter().enumerate() {
                for c in &group.constraints {
                    sqlx::query!(
                        "INSERT INTO rule_constraints (rule_id, group_index, attribute, operator, values) \
                         VALUES ($1, $2, $3, $4, $5)",
                        rule_id,
                        group_index as i32,
                        c.attribute,
                        operator_to_str(c.operator),
                        Json::Array(c.values.clone()),
                    )
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }
        Self::record_change(
            &mut tx,
            actor,
            "set_flag_rules",
            "flag",
            flag_key,
            serde_json::json!({ "rule_count": rules.len() }),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

/// Reject a variant whose JSON value doesn't match the flag's declared type, so the
/// mismatch surfaces at write time rather than as a `TYPE_MISMATCH` during evaluation.
fn check_variant_type(value_type: ValueType, variant: &Variant) -> AppResult<()> {
    let ok = match value_type {
        ValueType::Boolean => variant.value.is_boolean(),
        ValueType::String => variant.value.is_string(),
        // Numbers arrive as protobuf doubles, so accept any whole-valued number.
        ValueType::Integer => variant.value.as_f64().is_some_and(|f| f.fract() == 0.0),
        ValueType::Float => variant.value.is_number(),
        ValueType::Object => variant.value.is_object(),
    };
    if !ok {
        return Err(AppError::Invalid(format!(
            "variant `{}` value does not match flag type `{}`",
            variant.key,
            value_type_to_str(value_type)
        )));
    }
    Ok(())
}

/// Reject rules that reference unknown variants or whose percentage split does not
/// sum to exactly 100, before any of them are written.
fn validate_rules(flag_key: &str, rules: &[Rule], variants: &HashSet<String>) -> AppResult<()> {
    for (rank, rule) in rules.iter().enumerate() {
        if let Some(variant) = &rule.variant_key
            && !variants.contains(variant)
        {
            return Err(AppError::Invalid(format!(
                "rule references variant `{variant}` not defined on flag `{flag_key}`"
            )));
        }

        if !rule.distributions.is_empty() {
            let mut total = 0u32;
            for d in &rule.distributions {
                if !variants.contains(&d.variant_key) {
                    return Err(AppError::Invalid(format!(
                        "rule references variant `{}` not defined on flag `{flag_key}`",
                        d.variant_key
                    )));
                }
                total += d.weight;
            }
            if total != 100 {
                return Err(AppError::Invalid(format!(
                    "rule {rank} distribution weights sum to {total}, expected 100"
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Distribution;

    fn variants(keys: &[&str]) -> HashSet<String> {
        keys.iter().map(|k| k.to_string()).collect()
    }

    fn rule(variant_key: Option<&str>, distributions: Vec<Distribution>) -> Rule {
        Rule {
            rank: 0,
            segment_key: None,
            variant_key: variant_key.map(String::from),
            distributions,
            constraint_groups: vec![],
            bucket_salt: String::new(),
        }
    }

    fn dist(variant_key: &str, weight: u32) -> Distribution {
        Distribution {
            variant_key: variant_key.into(),
            weight,
        }
    }

    #[test]
    fn accepts_known_variant_and_full_split() {
        let vs = variants(&["on", "off"]);
        let rules = vec![
            rule(Some("on"), vec![]),
            rule(None, vec![dist("on", 30), dist("off", 70)]),
        ];
        assert!(validate_rules("f", &rules, &vs).is_ok());
    }

    #[test]
    fn rejects_unknown_target_variant() {
        let vs = variants(&["on", "off"]);
        let err = validate_rules("f", &[rule(Some("maybe"), vec![])], &vs).unwrap_err();
        assert!(matches!(err, AppError::Invalid(_)));
    }

    #[test]
    fn rejects_unknown_distribution_variant() {
        let vs = variants(&["on", "off"]);
        let rules = vec![rule(None, vec![dist("on", 50), dist("ghost", 50)])];
        assert!(matches!(
            validate_rules("f", &rules, &vs),
            Err(AppError::Invalid(_))
        ));
    }

    fn variant(value: serde_json::Value) -> Variant {
        Variant {
            key: "v".into(),
            value,
        }
    }

    #[test]
    fn variant_type_check_accepts_matching_and_rejects_mismatched() {
        use serde_json::json;
        assert!(check_variant_type(ValueType::Boolean, &variant(json!(true))).is_ok());
        assert!(check_variant_type(ValueType::Integer, &variant(json!(7))).is_ok());
        assert!(check_variant_type(ValueType::Float, &variant(json!(1.5))).is_ok());
        assert!(check_variant_type(ValueType::Float, &variant(json!(2))).is_ok());
        assert!(check_variant_type(ValueType::String, &variant(json!("x"))).is_ok());
        assert!(check_variant_type(ValueType::Object, &variant(json!({"a": 1}))).is_ok());

        assert!(check_variant_type(ValueType::Integer, &variant(json!("hello"))).is_err());
        assert!(check_variant_type(ValueType::Integer, &variant(json!(1.5))).is_err());
        assert!(check_variant_type(ValueType::Boolean, &variant(json!(1))).is_err());
        assert!(check_variant_type(ValueType::Object, &variant(json!([1, 2]))).is_err());
    }

    #[test]
    fn rejects_split_not_summing_to_100() {
        let vs = variants(&["on", "off"]);
        let rules = vec![rule(None, vec![dist("on", 30), dist("off", 60)])];
        let err = validate_rules("f", &rules, &vs).unwrap_err();
        assert!(matches!(err, AppError::Invalid(m) if m.contains("sum to 90")));
    }
}
