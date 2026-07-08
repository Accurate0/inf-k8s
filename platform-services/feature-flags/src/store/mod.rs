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

/// Whether a diffed flag or segment is being created, updated, or deleted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeOp {
    Create,
    Update,
    Delete,
}

/// One planned (or applied) change to a single flag or segment, as computed by
/// [`Store::apply_config`].
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigChange {
    pub target_kind: &'static str,
    pub target_key: String,
    pub op: ChangeOp,
    pub detail: Json,
}

/// The result of an [`Store::apply_config`] call: the diff, plus the versions it
/// moved between and whether it was actually written.
#[derive(Debug)]
pub struct ApplyOutcome {
    pub changes: Vec<ConfigChange>,
    pub from_version: i64,
    pub to_version: i64,
    pub applied: bool,
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

    /// Reconcile live state to the declared desired set of `flags` and `segments`.
    ///
    /// The diff is computed against the current snapshot; flags and segments not in
    /// the desired set are pruned. With `dry_run` the diff is returned without
    /// writing. Otherwise every change is applied in one transaction: the version row
    /// is locked and re-checked against `expected_version` (0 skips the check) so a
    /// stale plan aborts rather than clobbering a concurrent write, and the config
    /// version bumps once for the whole batch.
    pub async fn apply_config(
        &self,
        actor: &str,
        flags: &[Flag],
        segments: &[Segment],
        dry_run: bool,
        expected_version: i64,
    ) -> AppResult<ApplyOutcome> {
        validate_desired(flags, segments)?;

        let current = self.load_snapshot().await?;
        let changes = diff_config(&current, flags, segments);

        if dry_run || changes.is_empty() {
            return Ok(ApplyOutcome {
                from_version: current.version,
                to_version: current.version,
                applied: !dry_run,
                changes,
            });
        }

        let mut tx = self.pool.begin().await?;
        let locked =
            sqlx::query_scalar!("SELECT version FROM config_version WHERE id FOR UPDATE")
                .fetch_one(&mut *tx)
                .await?;
        if locked != current.version || (expected_version != 0 && locked != expected_version) {
            return Err(AppError::Aborted(
                "configuration changed since the plan was computed; re-run plan".into(),
            ));
        }

        // Apply in dependency-safe order: upsert segments before flags (rules
        // reference segment keys), prune flags before segments.
        for change in &changes {
            let desired_flag = || flags.iter().find(|f| f.key == change.target_key);
            let desired_segment = || segments.iter().find(|s| s.key == change.target_key);
            match (change.target_kind, change.op) {
                ("segment", ChangeOp::Create | ChangeOp::Update) => {
                    Self::upsert_segment_tx(&mut tx, desired_segment().unwrap()).await?;
                }
                ("flag", ChangeOp::Create | ChangeOp::Update) => {
                    Self::upsert_flag_tx(&mut tx, desired_flag().unwrap()).await?;
                }
                _ => {}
            }
        }
        for change in &changes {
            if change.target_kind == "flag" && change.op == ChangeOp::Delete {
                sqlx::query!("DELETE FROM flags WHERE key = $1", change.target_key)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        for change in &changes {
            if change.target_kind == "segment" && change.op == ChangeOp::Delete {
                sqlx::query!("DELETE FROM segments WHERE key = $1", change.target_key)
                    .execute(&mut *tx)
                    .await?;
            }
        }

        for change in &changes {
            Self::record_change(
                &mut tx,
                actor,
                audit_action(change.target_kind, change.op),
                change.target_kind,
                &change.target_key,
                change.detail.clone(),
            )
            .await?;
        }
        tx.commit().await?;

        let to_version = self.config_version().await?;
        Ok(ApplyOutcome {
            from_version: current.version,
            to_version,
            applied: true,
            changes,
        })
    }

    /// Upsert a flag and reconcile its variants and rules within the caller's
    /// transaction: the flag row is inserted-or-updated, variants absent from the
    /// desired set are dropped, and the rule set is replaced wholesale.
    async fn upsert_flag_tx(tx: &mut sqlx::PgConnection, flag: &Flag) -> AppResult<()> {
        let flag_id = sqlx::query_scalar!(
            "INSERT INTO flags (key, value_type, enabled, default_variant_key, archived) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (key) DO UPDATE SET \
               value_type = EXCLUDED.value_type, enabled = EXCLUDED.enabled, \
               default_variant_key = EXCLUDED.default_variant_key, \
               archived = EXCLUDED.archived, updated_at = now() \
             RETURNING id",
            flag.key,
            value_type_to_str(flag.value_type),
            flag.enabled,
            flag.default_variant_key,
            flag.archived,
        )
        .fetch_one(&mut *tx)
        .await?;

        let desired_keys: Vec<String> = flag.variants.iter().map(|v| v.key.clone()).collect();
        sqlx::query!(
            "DELETE FROM variants WHERE flag_id = $1 AND key <> ALL($2::text[])",
            flag_id,
            &desired_keys,
        )
        .execute(&mut *tx)
        .await?;
        for v in &flag.variants {
            sqlx::query!(
                "INSERT INTO variants (flag_id, key, value) VALUES ($1, $2, $3) \
                 ON CONFLICT (flag_id, key) DO UPDATE SET value = EXCLUDED.value",
                flag_id,
                v.key,
                v.value,
            )
            .execute(&mut *tx)
            .await?;
        }

        Self::replace_rules_tx(tx, flag_id, &flag.rules).await
    }

    /// Replace a flag's entire ordered rule set within the caller's transaction.
    async fn replace_rules_tx(
        tx: &mut sqlx::PgConnection,
        flag_id: Uuid,
        rules: &[Rule],
    ) -> AppResult<()> {
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
        Ok(())
    }

    /// Upsert a segment and replace its constraints within the caller's transaction.
    async fn upsert_segment_tx(tx: &mut sqlx::PgConnection, segment: &Segment) -> AppResult<()> {
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
        Ok(())
    }
}

/// The audit-log action name recorded for a config-driven change, reusing the same
/// vocabulary as the per-operation admin writes.
fn audit_action(target_kind: &str, op: ChangeOp) -> &'static str {
    match (target_kind, op) {
        ("flag", ChangeOp::Create) => "create_flag",
        ("flag", ChangeOp::Update) => "update_flag",
        ("flag", ChangeOp::Delete) => "delete_flag",
        ("segment", ChangeOp::Delete) => "delete_segment",
        _ => "upsert_segment",
    }
}

/// Validate the full desired set the way the per-operation writers would, so a plan
/// surfaces bad input (unknown default variant, type mismatch, malformed split)
/// before anything is written.
fn validate_desired(flags: &[Flag], _segments: &[Segment]) -> AppResult<()> {
    for flag in flags {
        if flag.variants.iter().all(|v| v.key != flag.default_variant_key) {
            return Err(AppError::Invalid(format!(
                "flag `{}` default variant `{}` is not among its variants",
                flag.key, flag.default_variant_key
            )));
        }
        for v in &flag.variants {
            check_variant_type(flag.value_type, v)?;
        }
        let variant_keys: HashSet<String> = flag.variants.iter().map(|v| v.key.clone()).collect();
        validate_rules(&flag.key, &flag.rules, &variant_keys)?;
    }
    Ok(())
}

/// Compute the ordered set of changes that turn `current` into the desired
/// `flags`/`segments`. Segment changes are emitted before flag changes.
fn diff_config(current: &Snapshot, flags: &[Flag], segments: &[Segment]) -> Vec<ConfigChange> {
    let mut changes = Vec::new();

    for segment in segments {
        match current.segments.get(&segment.key) {
            None => changes.push(ConfigChange {
                target_kind: "segment",
                target_key: segment.key.clone(),
                op: ChangeOp::Create,
                detail: serde_json::json!({ "name": segment.name }),
            }),
            Some(live) if !segments_equal(live, segment) => changes.push(ConfigChange {
                target_kind: "segment",
                target_key: segment.key.clone(),
                op: ChangeOp::Update,
                detail: serde_json::json!({ "fields": segment_diff_fields(live, segment) }),
            }),
            Some(_) => {}
        }
    }
    for key in current.segments.keys() {
        if !segments.iter().any(|s| &s.key == key) {
            changes.push(ConfigChange {
                target_kind: "segment",
                target_key: key.clone(),
                op: ChangeOp::Delete,
                detail: Json::Null,
            });
        }
    }

    for flag in flags {
        match current.flags.get(&flag.key) {
            None => changes.push(ConfigChange {
                target_kind: "flag",
                target_key: flag.key.clone(),
                op: ChangeOp::Create,
                detail: serde_json::json!({
                    "value_type": value_type_to_str(flag.value_type),
                    "enabled": flag.enabled,
                    "default_variant_key": flag.default_variant_key,
                }),
            }),
            Some(live) => {
                let fields = flag_diff_fields(live, flag);
                if !fields.is_empty() {
                    changes.push(ConfigChange {
                        target_kind: "flag",
                        target_key: flag.key.clone(),
                        op: ChangeOp::Update,
                        detail: serde_json::json!({ "fields": fields }),
                    });
                }
            }
        }
    }
    for key in current.flags.keys() {
        if !flags.iter().any(|f| &f.key == key) {
            changes.push(ConfigChange {
                target_kind: "flag",
                target_key: key.clone(),
                op: ChangeOp::Delete,
                detail: Json::Null,
            });
        }
    }

    changes
}

/// Variants compared order-independently (the store has no variant ordering).
fn variants_equal(a: &[Variant], b: &[Variant]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut a: Vec<&Variant> = a.iter().collect();
    let mut b: Vec<&Variant> = b.iter().collect();
    a.sort_by(|x, y| x.key.cmp(&y.key));
    b.sort_by(|x, y| x.key.cmp(&y.key));
    a == b
}

/// Rules compared as ordered lists with `rank` normalised to position, since rank is
/// assigned by position on write and is not part of the declared config.
fn rules_equal(a: &[Rule], b: &[Rule]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).all(|(x, y)| {
        Rule { rank: 0, ..x.clone() } == Rule { rank: 0, ..y.clone() }
    })
}

fn flag_diff_fields(live: &Flag, desired: &Flag) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if live.value_type != desired.value_type {
        fields.push("value_type");
    }
    if live.enabled != desired.enabled {
        fields.push("enabled");
    }
    if live.default_variant_key != desired.default_variant_key {
        fields.push("default_variant_key");
    }
    if live.archived != desired.archived {
        fields.push("archived");
    }
    if !variants_equal(&live.variants, &desired.variants) {
        fields.push("variants");
    }
    if !rules_equal(&live.rules, &desired.rules) {
        fields.push("rules");
    }
    fields
}

fn segments_equal(live: &Segment, desired: &Segment) -> bool {
    segment_diff_fields(live, desired).is_empty()
}

fn segment_diff_fields(live: &Segment, desired: &Segment) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if live.name != desired.name {
        fields.push("name");
    }
    if live.constraints != desired.constraints {
        fields.push("constraints");
    }
    fields
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
