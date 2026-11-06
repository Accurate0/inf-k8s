use std::collections::BTreeMap;
use std::time::Duration;

use async_graphql::{Context, Enum, ID, InputObject, Object, Result, SimpleObject, Subscription};
use axum::extract::{Path, Query, State};
use axum::{Extension, Json};
use futures::Stream;
use waf_manager_api as api;

use super::objects::{
    Allowlist, AllowlistEntry, AuditPage, Block, Blocks, Candidates, IpReport, Workflow, Workflows,
};
use super::{Gql, IDS, claims, state};
use crate::api::{Api, AuditQuery, BlocksQuery, CidrQuery};

pub struct QueryRoot;

#[Object(name = "Query")]
impl QueryRoot {
    /// Offending IPs seen in the current window.
    async fn waf_candidates(&self, ctx: &Context<'_>) -> Result<Candidates> {
        Ok(Api::candidates(State(state(ctx).clone()))
            .await
            .gql()?
            .0
            .into())
    }

    /// Blocks newest first, optionally filtered by a CIDR substring.
    async fn waf_blocks(&self, ctx: &Context<'_>, query: Option<String>) -> Result<Blocks> {
        let blocks = Api::blocks(
            State(state(ctx).clone()),
            Query(BlocksQuery {
                q: query.unwrap_or_default(),
            }),
        )
        .await
        .gql()?;
        Ok(blocks.0.into())
    }

    async fn waf_block(&self, ctx: &Context<'_>, name: String) -> Result<Block> {
        Block::load(ctx, &name).await
    }

    /// Everything known about an IP or CIDR: strikes, rules hit, decisions and audit.
    async fn waf_ip(&self, ctx: &Context<'_>, addr: String) -> Result<IpReport> {
        IpReport::load(ctx, addr).await
    }

    async fn waf_audit(
        &self,
        ctx: &Context<'_>,
        #[graphql(default)] offset: i64,
        #[graphql(default = 100)] limit: i64,
    ) -> Result<AuditPage> {
        let page = Api::audit(
            State(state(ctx).clone()),
            Query(AuditQuery {
                offset,
                limit: Some(limit),
            }),
        )
        .await
        .gql()?
        .0;
        Ok(AuditPage {
            entries: page.entries.into_iter().map(Into::into).collect(),
            total: page.total,
        })
    }

    /// Workflows with their scorecards; `workflow` narrows the decisions to one.
    async fn waf_workflows(
        &self,
        ctx: &Context<'_>,
        workflow: Option<String>,
    ) -> Result<Workflows> {
        Workflows::load(ctx, workflow).await
    }

    async fn waf_allowlist(&self, ctx: &Context<'_>) -> Result<Allowlist> {
        Allowlist::load(ctx).await
    }

    #[graphql(entity)]
    async fn find_block_by_id(&self, ctx: &Context<'_>, id: ID) -> Result<Block> {
        Block::load(ctx, &IDS.decode_as(&id, "block")?).await
    }

    #[graphql(entity)]
    async fn find_ip_report_by_id(&self, ctx: &Context<'_>, id: ID) -> Result<IpReport> {
        IpReport::load(ctx, IDS.decode_as(&id, "ipReport")?).await
    }

    #[graphql(entity)]
    async fn find_workflow_by_id(&self, ctx: &Context<'_>, id: ID) -> Result<Workflow> {
        let name = IDS.decode_as(&id, "workflow")?;
        Workflows::load(ctx, None)
            .await?
            .workflows
            .into_iter()
            .find(|w| w.name == name)
            .ok_or_else(|| async_graphql::Error::new(format!("workflow {name} not found")))
    }

    #[graphql(entity)]
    async fn find_allowlist_entry_by_id(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> Result<AllowlistEntry> {
        let cidr = IDS.decode_as(&id, "allowlistEntry")?;
        Allowlist::load(ctx)
            .await?
            .manual
            .into_iter()
            .find(|e| e.cidr == cidr)
            .ok_or_else(|| async_graphql::Error::new(format!("allowlist entry {cidr} not found")))
    }
}

pub struct MutationRoot;

#[derive(InputObject)]
#[graphql(name = "WafCreateBlockInput")]
pub struct CreateBlockInput {
    cidr: String,
    /// Defaults to the workflow default gateway.
    gateway: Option<String>,
    reason: Option<String>,
    /// Omit for a permanent block.
    ttl_hours: Option<u32>,
}

#[Object(name = "Mutation")]
impl MutationRoot {
    async fn waf_create_block(&self, ctx: &Context<'_>, input: CreateBlockInput) -> Result<Block> {
        let (_, block) = Api::create_block(
            State(state(ctx).clone()),
            Extension(claims(ctx)?),
            Json(api::CreateBlock {
                cidr: input.cidr,
                gateway: input.gateway,
                reason: input.reason,
                ttl_hours: input.ttl_hours,
            }),
        )
        .await
        .gql()?;
        Ok(block.0.into())
    }

    async fn waf_delete_block(&self, ctx: &Context<'_>, name: String) -> Result<ID> {
        Api::delete_block(
            State(state(ctx).clone()),
            Extension(claims(ctx)?),
            Path(name.clone()),
        )
        .await
        .gql()?;
        Ok(IDS.encode("block", name))
    }

    /// Drops the block's expiry.
    async fn waf_make_block_permanent(&self, ctx: &Context<'_>, name: String) -> Result<Block> {
        Api::make_permanent(
            State(state(ctx).clone()),
            Extension(claims(ctx)?),
            Path(name.clone()),
        )
        .await
        .gql()?;
        Block::load(ctx, &name).await
    }

    async fn waf_add_allowlist(
        &self,
        ctx: &Context<'_>,
        cidr: String,
        note: Option<String>,
    ) -> Result<Allowlist> {
        Api::create_allowlist(
            State(state(ctx).clone()),
            Extension(claims(ctx)?),
            Json(api::CreateAllowlist { cidr, note }),
        )
        .await
        .gql()?;
        Allowlist::load(ctx).await
    }

    async fn waf_remove_allowlist(&self, ctx: &Context<'_>, cidr: String) -> Result<Allowlist> {
        Api::delete_allowlist(
            State(state(ctx).clone()),
            Extension(claims(ctx)?),
            Query(CidrQuery { cidr }),
        )
        .await
        .gql()?;
        Allowlist::load(ctx).await
    }
}

pub struct SubscriptionRoot;

const POLL_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
#[graphql(name = "WafBlockChange")]
pub enum BlockChange {
    Created,
    Updated,
    Removed,
}

#[derive(SimpleObject)]
#[graphql(name = "WafBlockEvent")]
pub struct BlockEvent {
    change: BlockChange,
    /// The block as it now stands, or as it was last seen when removed.
    block: Block,
}

fn by_name(blocks: Vec<api::Block>) -> BTreeMap<String, Block> {
    blocks
        .into_iter()
        .map(|b| (b.name.clone(), Block::from(b)))
        .collect()
}

#[Subscription(name = "Subscription")]
impl SubscriptionRoot {
    /// Blocks created, changed (enforced, strikes, expiry) or removed by anyone.
    async fn waf_blocks(
        &self,
        ctx: &Context<'_>,
    ) -> Result<impl Stream<Item = Result<BlockEvent>>> {
        let state = state(ctx).clone();
        let initial = by_name(Api::all_blocks(&state).await.gql()?);

        Ok(async_stream::try_stream! {
            let mut seen = initial;
            let mut ticker = tokio::time::interval(POLL_INTERVAL);
            loop {
                ticker.tick().await;
                let current = by_name(Api::all_blocks(&state).await.gql()?);

                for (name, block) in &current {
                    match seen.get(name) {
                        None => yield BlockEvent { change: BlockChange::Created, block: block.clone() },
                        Some(old) if old != block => {
                            yield BlockEvent { change: BlockChange::Updated, block: block.clone() }
                        }
                        Some(_) => {}
                    }
                }
                for (name, block) in &seen {
                    if !current.contains_key(name) {
                        yield BlockEvent { change: BlockChange::Removed, block: block.clone() };
                    }
                }
                seen = current;
            }
        })
    }
}
