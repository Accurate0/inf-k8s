use async_graphql::{ComplexObject, Context, ID, Interface, Result, SimpleObject};
use axum::extract::{Path, State};
use chrono::{DateTime, Utc};
use waf_manager_api as api;

use super::{Gql, IDS, state};
use crate::api::Api;

#[derive(Interface)]
#[graphql(field(name = "id", ty = "ID"))]
#[allow(clippy::large_enum_variant)]
pub enum Node {
    Block(Block),
    IpReport(IpReport),
    Workflow(Workflow),
    AllowlistEntry(AllowlistEntry),
}

#[derive(SimpleObject, Clone, PartialEq)]
#[graphql(name = "WafBlock", complex)]
pub struct Block {
    /// The WafBlock resource name.
    pub name: String,
    pub cidr: String,
    pub gateway: String,
    pub reason: Option<String>,
    pub created_by: Option<String>,
    /// Created by a workflow rather than a person.
    pub automatic: bool,
    pub created_at: Option<DateTime<Utc>>,
    /// RFC 3339, or absent for a permanent block.
    pub expires_at: Option<String>,
    pub enforced: bool,
    pub strikes: i32,
}

#[ComplexObject]
impl Block {
    async fn id(&self) -> ID {
        IDS.encode("block", &self.name)
    }

    async fn report(&self, ctx: &Context<'_>) -> Result<IpReport> {
        IpReport::load(ctx, self.cidr.clone()).await
    }
}

impl From<api::Block> for Block {
    fn from(b: api::Block) -> Self {
        Self {
            name: b.name,
            cidr: b.cidr,
            gateway: b.gateway,
            reason: b.reason,
            created_by: b.created_by,
            automatic: b.automatic,
            created_at: b.created_at,
            expires_at: b.expires_at,
            enforced: b.enforced,
            strikes: b.strikes,
        }
    }
}

impl Block {
    pub async fn list(ctx: &Context<'_>) -> Result<Vec<Self>> {
        Ok(Api::all_blocks(state(ctx))
            .await
            .gql()?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub async fn load(ctx: &Context<'_>, name: &str) -> Result<Self> {
        Self::list(ctx)
            .await?
            .into_iter()
            .find(|b| b.name == name)
            .ok_or_else(|| async_graphql::Error::new(format!("block {name} not found")))
    }
}

#[derive(SimpleObject)]
#[graphql(name = "WafConflict")]
pub struct Conflict {
    pub source: String,
    pub message: String,
}

#[derive(SimpleObject)]
#[graphql(name = "WafBlocks")]
pub struct Blocks {
    pub blocks: Vec<Block>,
    /// Policies the compositor could not apply.
    pub conflicts: Vec<Conflict>,
}

impl From<api::Blocks> for Blocks {
    fn from(b: api::Blocks) -> Self {
        Self {
            blocks: b.blocks.into_iter().map(Into::into).collect(),
            conflicts: b
                .conflicts
                .into_iter()
                .map(|c| Conflict {
                    source: c.source,
                    message: c.message,
                })
                .collect(),
        }
    }
}

#[derive(SimpleObject)]
#[graphql(name = "WafCandidate", complex)]
pub struct Candidate {
    pub client_ip: String,
    pub detections: u64,
    pub protected_by: Option<String>,
}

#[ComplexObject]
impl Candidate {
    async fn report(&self, ctx: &Context<'_>) -> Result<IpReport> {
        IpReport::load(ctx, self.client_ip.clone()).await
    }
}

impl From<api::Candidate> for Candidate {
    fn from(c: api::Candidate) -> Self {
        Self {
            client_ip: c.client_ip,
            detections: c.detections,
            protected_by: c.protected_by,
        }
    }
}

#[derive(SimpleObject)]
#[graphql(name = "WafCandidates")]
pub struct Candidates {
    pub window: String,
    /// Offenders that can be blocked.
    pub actionable: Vec<Candidate>,
    /// Offenders inside an allowlisted range.
    pub protected: Vec<Candidate>,
    /// Offenders already blocked.
    pub blocked: Vec<Candidate>,
}

impl From<api::Candidates> for Candidates {
    fn from(c: api::Candidates) -> Self {
        let convert = |rows: Vec<api::Candidate>| rows.into_iter().map(Into::into).collect();
        Self {
            window: c.window,
            actionable: convert(c.actionable),
            protected: convert(c.protected),
            blocked: convert(c.blocked),
        }
    }
}

#[derive(SimpleObject)]
#[graphql(name = "WafDecision")]
pub struct Decision {
    pub at: DateTime<Utc>,
    pub workflow: String,
    pub cidr: String,
    pub detections: u64,
    pub mode: String,
    pub outcome: String,
}

impl From<api::Decision> for Decision {
    fn from(d: api::Decision) -> Self {
        Self {
            at: d.at,
            workflow: d.workflow,
            cidr: d.cidr,
            detections: d.detections,
            mode: d.mode,
            outcome: d.outcome,
        }
    }
}

#[derive(SimpleObject)]
#[graphql(name = "WafAuditEntry")]
pub struct AuditEntry {
    pub at: DateTime<Utc>,
    pub actor: String,
    pub action: String,
    pub target: String,
    pub detail: Option<String>,
}

impl From<api::AuditEntry> for AuditEntry {
    fn from(e: api::AuditEntry) -> Self {
        Self {
            at: e.at,
            actor: e.actor,
            action: e.action,
            target: e.target,
            detail: e.detail,
        }
    }
}

#[derive(SimpleObject)]
#[graphql(name = "WafAuditPage")]
pub struct AuditPage {
    pub entries: Vec<AuditEntry>,
    pub total: i64,
}

#[derive(SimpleObject)]
#[graphql(name = "WafRuleHit")]
pub struct RuleHit {
    pub rule_id: String,
    pub rule_msg: String,
    pub severity: String,
    pub count: u64,
    pub ignored: bool,
}

#[derive(SimpleObject)]
#[graphql(name = "WafUriHit")]
pub struct UriHit {
    pub uri: String,
    pub count: u64,
}

#[derive(SimpleObject)]
#[graphql(name = "WafEnrichment")]
pub struct Enrichment {
    pub hostname: Option<String>,
    pub asn: Option<String>,
    pub org: Option<String>,
    pub prefix: Option<String>,
    pub country: Option<String>,
}

#[derive(SimpleObject)]
#[graphql(name = "WafIpReport", complex)]
pub struct IpReport {
    pub client_ip: String,
    pub cidr: String,
    pub window: String,
    pub strikes: i32,
    pub protected_by: Option<String>,
    pub block: Option<Block>,
    pub decisions: Vec<Decision>,
    pub audit: Vec<AuditEntry>,
    pub enrichment: Option<Enrichment>,
    pub rules: Vec<RuleHit>,
    pub uris: Vec<UriHit>,
    /// Recent raw log lines from Loki.
    pub lines: Vec<String>,
}

#[ComplexObject]
impl IpReport {
    async fn id(&self) -> ID {
        IDS.encode("ipReport", &self.client_ip)
    }
}

impl IpReport {
    pub async fn load(ctx: &Context<'_>, addr: String) -> Result<Self> {
        let d = Api::ip(State(state(ctx).clone()), Path(addr))
            .await
            .gql()?
            .0;
        Ok(Self {
            client_ip: d.client_ip,
            cidr: d.cidr,
            window: d.window,
            strikes: d.strikes,
            protected_by: d.protected,
            block: d.block.map(Into::into),
            decisions: d.decisions.into_iter().map(Into::into).collect(),
            audit: d.audit.into_iter().map(Into::into).collect(),
            enrichment: d.enrichment.map(|e| Enrichment {
                hostname: e.hostname,
                asn: e.asn,
                org: e.org,
                prefix: e.prefix,
                country: e.country,
            }),
            rules: d
                .rules
                .into_iter()
                .map(|r| RuleHit {
                    rule_id: r.rule_id,
                    rule_msg: r.rule_msg,
                    severity: r.severity,
                    count: r.count,
                    ignored: r.ignored,
                })
                .collect(),
            uris: d
                .uris
                .into_iter()
                .map(|u| UriHit {
                    uri: u.uri,
                    count: u.count,
                })
                .collect(),
            lines: d.lines,
        })
    }
}

#[derive(SimpleObject)]
#[graphql(name = "WafScorecard")]
pub struct Scorecard {
    pub enabled: String,
    pub decisions: i64,
    pub cidrs: i64,
    pub unblocked: i64,
    pub agreement: i64,
}

#[derive(SimpleObject)]
#[graphql(name = "WafWorkflow", complex)]
pub struct Workflow {
    pub name: String,
    pub enabled: String,
    pub tier: String,
    pub window: String,
    pub duration: String,
    pub gateway: String,
    pub reason: String,
    pub signals: Vec<String>,
    pub scorecard: Option<Scorecard>,
}

#[ComplexObject]
impl Workflow {
    async fn id(&self) -> ID {
        IDS.encode("workflow", &self.name)
    }

    async fn decisions(&self, ctx: &Context<'_>) -> Result<Vec<Decision>> {
        Ok(Workflows::load(ctx, Some(self.name.clone()))
            .await?
            .decisions)
    }
}

#[derive(SimpleObject)]
#[graphql(name = "WafWorkflows")]
pub struct Workflows {
    pub workflows: Vec<Workflow>,
    pub decisions: Vec<Decision>,
    /// How long a manual unblock keeps workflows from re-blocking.
    pub cooldown: String,
}

impl Workflows {
    pub async fn load(ctx: &Context<'_>, workflow: Option<String>) -> Result<Self> {
        let w = Api::workflows(
            State(state(ctx).clone()),
            axum::extract::Query(crate::api::WorkflowsQuery { workflow }),
        )
        .await
        .gql()?
        .0;

        let mut scorecards = w.scorecard;
        Ok(Self {
            workflows: w
                .workflows
                .into_iter()
                .map(|wf| {
                    let scorecard = scorecards
                        .iter()
                        .position(|s| s.workflow == wf.name)
                        .map(|i| scorecards.swap_remove(i))
                        .map(|s| Scorecard {
                            enabled: s.enabled,
                            decisions: s.decisions,
                            cidrs: s.cidrs,
                            unblocked: s.unblocked,
                            agreement: s.agreement,
                        });
                    Workflow {
                        name: wf.name,
                        enabled: wf.enabled,
                        tier: wf.tier,
                        window: wf.window,
                        duration: wf.duration,
                        gateway: wf.gateway,
                        reason: wf.reason,
                        signals: wf.signals,
                        scorecard,
                    }
                })
                .collect(),
            decisions: w.decisions.into_iter().map(Into::into).collect(),
            cooldown: w.cooldown,
        })
    }
}

#[derive(SimpleObject)]
#[graphql(name = "WafAllowlistEntry", complex)]
pub struct AllowlistEntry {
    pub cidr: String,
    pub note: Option<String>,
    pub created_by: String,
    pub at: DateTime<Utc>,
}

#[ComplexObject]
impl AllowlistEntry {
    async fn id(&self) -> ID {
        IDS.encode("allowlistEntry", &self.cidr)
    }
}

#[derive(SimpleObject)]
#[graphql(name = "WafProtectedSource")]
pub struct ProtectedSource {
    pub source: String,
    pub count: usize,
}

#[derive(SimpleObject)]
#[graphql(name = "WafAllowlist")]
pub struct Allowlist {
    /// Ranges added by hand.
    pub manual: Vec<AllowlistEntry>,
    /// Ranges protected by config, counted per source.
    pub protected: Vec<ProtectedSource>,
}

impl Allowlist {
    pub async fn load(ctx: &Context<'_>) -> Result<Self> {
        let a = Api::allowlist(State(state(ctx).clone())).await.gql()?.0;
        Ok(Self {
            manual: a
                .manual
                .into_iter()
                .map(|m| AllowlistEntry {
                    cidr: m.cidr,
                    note: m.note,
                    created_by: m.created_by,
                    at: m.at,
                })
                .collect(),
            protected: a
                .protected
                .into_iter()
                .map(|p| ProtectedSource {
                    source: p.source,
                    count: p.count,
                })
                .collect(),
        })
    }
}
