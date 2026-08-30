use crate::config::Span;
use crate::error::Result;
use crate::metrics::Metrics;
use ipnet::IpNet;
use sqlx::postgres::PgPool;

pub struct Offenses {
    pool: PgPool,
    decay: Span,
}

impl Offenses {
    pub fn new(pool: PgPool, decay: Span) -> Self {
        Self { pool, decay }
    }

    pub async fn record(&self, net: &IpNet, now: chrono::DateTime<chrono::Utc>) -> Result<i32> {
        let row = sqlx::query!(
            "insert into offenses (cidr, strikes, first_seen, last_seen)
             values ($1, 1, $2, $2)
             on conflict (cidr) do update
             set strikes = offenses.strikes + 1, last_seen = $2
             returning strikes",
            net.to_string(),
            now,
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(row.strikes)
    }

    pub async fn strikes(&self, net: &IpNet) -> Result<i32> {
        let row = sqlx::query!(
            "select strikes from offenses where cidr = $1",
            net.to_string()
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|row| row.strikes).unwrap_or(0))
    }

    pub async fn all(&self) -> Result<std::collections::BTreeMap<String, i32>> {
        let rows = sqlx::query!("select cidr, strikes from offenses")
            .fetch_all(&self.pool)
            .await?;

        Ok(rows
            .into_iter()
            .map(|row| (row.cidr, row.strikes))
            .collect())
    }

    pub async fn reset(&self, net: &IpNet) -> Result<()> {
        sqlx::query!("delete from offenses where cidr = $1", net.to_string())
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn prune(&self, now: chrono::DateTime<chrono::Utc>) -> Result<()> {
        let cutoff =
            now - chrono::Duration::from_std(self.decay.0).unwrap_or(chrono::Duration::zero());

        sqlx::query!("delete from offenses where last_seen <= $1", cutoff)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn report(&self) -> Result<()> {
        let row = sqlx::query!("select count(*) as total from offenses")
            .fetch_one(&self.pool)
            .await?;

        Metrics::set_offenses(row.total.unwrap_or(0) as usize);
        Ok(())
    }
}
