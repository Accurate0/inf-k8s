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
use std::collections::{BTreeMap, HashMap};
use types::{
    json_array, operator_from_str, operator_to_str, unique_violation, value_type_from_str,
    value_type_to_str,
};
use uuid::Uuid;

#[derive(Clone)]
pub struct Store {
    pool: PgPool,
}

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
            "SELECT id, flag_id, rank, segment_key, variant_key FROM flag_rules ORDER BY rank"
        )
        .fetch_all(&self.pool)
        .await?
        {
            if let Some(flag) = flags.get_mut(&row.flag_id) {
                flag.rules.push(Rule {
                    rank: row.rank as u32,
                    segment_key: row.segment_key,
                    variant_key: row.variant_key,
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

    pub async fn flag_id(&self, key: &str) -> AppResult<Uuid> {
        sqlx::query_scalar!("SELECT id FROM flags WHERE key = $1", key)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("flag `{key}`")))
    }

    pub async fn get_flag(&self, key: &str) -> AppResult<Flag> {
        self.load_snapshot()
            .await?
            .flags
            .get(key)
            .cloned()
            .ok_or_else(|| AppError::NotFound(format!("flag `{key}`")))
    }

    pub async fn list_flags(&self, include_archived: bool) -> AppResult<Vec<Flag>> {
        let mut flags: Vec<Flag> = self
            .load_snapshot()
            .await?
            .flags
            .into_values()
            .filter(|f| include_archived || !f.archived)
            .collect();
        flags.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(flags)
    }

    pub async fn create_flag(
        &self,
        key: &str,
        value_type: ValueType,
        enabled: bool,
        default_variant_key: &str,
        variants: &[Variant],
    ) -> AppResult<Flag> {
        if variants.iter().all(|v| v.key != default_variant_key) {
            return Err(AppError::Invalid(format!(
                "default variant `{default_variant_key}` is not among the provided variants"
            )));
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
        tx.commit().await?;
        self.get_flag(key).await
    }

    pub async fn update_flag(
        &self,
        key: &str,
        enabled: bool,
        default_variant_key: &str,
    ) -> AppResult<Flag> {
        let affected = sqlx::query!(
            "UPDATE flags SET enabled = $2, default_variant_key = $3, updated_at = now() \
             WHERE key = $1",
            key,
            enabled,
            default_variant_key,
        )
        .execute(&self.pool)
        .await?
        .rows_affected();
        if affected == 0 {
            return Err(AppError::NotFound(format!("flag `{key}`")));
        }
        self.get_flag(key).await
    }

    pub async fn archive_flag(&self, key: &str, archived: bool) -> AppResult<Flag> {
        let affected = sqlx::query!(
            "UPDATE flags SET archived = $2, updated_at = now() WHERE key = $1",
            key,
            archived,
        )
        .execute(&self.pool)
        .await?
        .rows_affected();
        if affected == 0 {
            return Err(AppError::NotFound(format!("flag `{key}`")));
        }
        self.get_flag(key).await
    }

    pub async fn delete_flag(&self, key: &str) -> AppResult<()> {
        let affected = sqlx::query!("DELETE FROM flags WHERE key = $1", key)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if affected == 0 {
            return Err(AppError::NotFound(format!("flag `{key}`")));
        }
        Ok(())
    }

    pub async fn upsert_variant(&self, flag_key: &str, variant: &Variant) -> AppResult<Flag> {
        let flag_id = self.flag_id(flag_key).await?;
        sqlx::query!(
            "INSERT INTO variants (flag_id, key, value) VALUES ($1, $2, $3) \
             ON CONFLICT (flag_id, key) DO UPDATE SET value = EXCLUDED.value",
            flag_id,
            variant.key,
            variant.value,
        )
        .execute(&self.pool)
        .await?;
        self.get_flag(flag_key).await
    }

    pub async fn delete_variant(&self, flag_key: &str, variant_key: &str) -> AppResult<Flag> {
        let flag_id = self.flag_id(flag_key).await?;
        sqlx::query!(
            "DELETE FROM variants WHERE flag_id = $1 AND key = $2",
            flag_id,
            variant_key,
        )
        .execute(&self.pool)
        .await?;
        self.get_flag(flag_key).await
    }

    pub async fn upsert_segment(&self, segment: &Segment) -> AppResult<Segment> {
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
        tx.commit().await?;
        self.get_segment(&segment.key).await
    }

    pub async fn get_segment(&self, key: &str) -> AppResult<Segment> {
        self.load_snapshot()
            .await?
            .segments
            .get(key)
            .cloned()
            .ok_or_else(|| AppError::NotFound(format!("segment `{key}`")))
    }

    pub async fn list_segments(&self) -> AppResult<Vec<Segment>> {
        let mut segments: Vec<Segment> =
            self.load_snapshot().await?.segments.into_values().collect();
        segments.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(segments)
    }

    pub async fn delete_segment(&self, key: &str) -> AppResult<()> {
        let affected = sqlx::query!("DELETE FROM segments WHERE key = $1", key)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if affected == 0 {
            return Err(AppError::NotFound(format!("segment `{key}`")));
        }
        Ok(())
    }

    pub async fn set_flag_rules(&self, flag_key: &str, rules: &[Rule]) -> AppResult<Flag> {
        let flag_id = self.flag_id(flag_key).await?;
        let mut tx = self.pool.begin().await?;
        sqlx::query!("DELETE FROM flag_rules WHERE flag_id = $1", flag_id)
            .execute(&mut *tx)
            .await?;

        for (rank, rule) in rules.iter().enumerate() {
            let rule_id = sqlx::query_scalar!(
                "INSERT INTO flag_rules (flag_id, rank, segment_key, variant_key) \
                 VALUES ($1, $2, $3, $4) RETURNING id",
                flag_id,
                rank as i32,
                rule.segment_key.as_deref(),
                rule.variant_key.as_deref(),
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
        tx.commit().await?;
        self.get_flag(flag_key).await
    }
}
