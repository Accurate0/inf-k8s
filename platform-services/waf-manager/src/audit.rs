use crate::error::Result;
use sqlx::postgres::PgPool;
use std::sync::Arc;

pub const ACTION_BLOCK: &str = "block";
pub const ACTION_UNBLOCK: &str = "unblock";
pub const ACTION_ALLOWLIST_ADD: &str = "allowlist-add";
pub const ACTION_ALLOWLIST_REMOVE: &str = "allowlist-remove";

#[derive(Debug, Clone)]
pub struct Entry {
    pub at: chrono::DateTime<chrono::Utc>,
    pub actor: String,
    pub action: String,
    pub target: String,
    pub detail: Option<String>,
}

impl Entry {
    pub fn host(&self) -> Option<&str> {
        self.target
            .strip_suffix("/32")
            .or_else(|| self.target.strip_suffix("/128"))
    }
}

#[derive(Clone)]
pub struct Audit {
    inner: Arc<AuditInner>,
}

struct AuditInner {
    pool: PgPool,
}

impl Audit {
    pub fn new(pool: PgPool) -> Self {
        Self {
            inner: Arc::new(AuditInner { pool }),
        }
    }

    pub async fn record(&self, actor: &str, action: &str, target: &str, detail: Option<&str>) {
        let result = sqlx::query!(
            "insert into audit (at, actor, action, target, detail) values ($1, $2, $3, $4, $5)",
            chrono::Utc::now(),
            actor,
            action,
            target,
            detail,
        )
        .execute(&self.inner.pool)
        .await;

        if let Err(e) = result {
            tracing::warn!("recording audit entry {action} on {target} failed: {e}");
        }
    }

    pub async fn recent(&self, limit: i64, offset: i64) -> Result<(Vec<Entry>, i64)> {
        let rows = sqlx::query!(
            "select at, actor, action, target, detail from audit
             order by at desc, id desc limit $1 offset $2",
            limit,
            offset,
        )
        .fetch_all(&self.inner.pool)
        .await?;

        let total = sqlx::query!("select count(*) as total from audit")
            .fetch_one(&self.inner.pool)
            .await?
            .total
            .unwrap_or(0);

        Ok((
            rows.into_iter()
                .map(|row| Entry {
                    at: row.at,
                    actor: row.actor,
                    action: row.action,
                    target: row.target,
                    detail: row.detail,
                })
                .collect(),
            total,
        ))
    }

    pub async fn for_target(&self, target: &str, limit: i64) -> Result<Vec<Entry>> {
        let rows = sqlx::query!(
            "select at, actor, action, target, detail from audit
             where target = $1 order by at desc, id desc limit $2",
            target,
            limit,
        )
        .fetch_all(&self.inner.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Entry {
                at: row.at,
                actor: row.actor,
                action: row.action,
                target: row.target,
                detail: row.detail,
            })
            .collect())
    }
}
