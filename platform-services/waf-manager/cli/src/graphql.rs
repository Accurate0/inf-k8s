use chrono::{DateTime, Utc};
use serde::Serialize;

#[cynic::schema("graph")]
mod schema {}

cynic::impl_scalar!(DateTime<Utc>, schema::DateTime);

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "WafBlock")]
pub struct Block {
    pub name: String,
    pub cidr: String,
    pub gateway: String,
    pub reason: Option<String>,
    pub created_by: Option<String>,
    pub automatic: bool,
    pub expires_at: Option<String>,
    pub enforced: bool,
    pub strikes: i32,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "WafConflict")]
pub struct Conflict {
    pub source: String,
    pub message: String,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "WafBlocks")]
pub struct Blocks {
    pub blocks: Vec<Block>,
    pub conflicts: Vec<Conflict>,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "WafCandidate")]
pub struct Candidate {
    pub client_ip: String,
    pub detections: i32,
    pub protected_by: Option<String>,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "WafCandidates")]
pub struct Candidates {
    pub window: String,
    pub actionable: Vec<Candidate>,
    pub protected: Vec<Candidate>,
    pub blocked: Vec<Candidate>,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "WafEnrichment")]
pub struct Enrichment {
    pub hostname: Option<String>,
    pub asn: Option<String>,
    pub org: Option<String>,
    pub prefix: Option<String>,
    pub country: Option<String>,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "WafRuleHit")]
pub struct RuleHit {
    pub rule_id: String,
    pub rule_msg: String,
    pub severity: String,
    pub count: i32,
    pub ignored: bool,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "WafUriHit")]
pub struct UriHit {
    pub uri: String,
    pub count: i32,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "WafDecision")]
pub struct Decision {
    pub at: DateTime<Utc>,
    pub workflow: String,
    pub cidr: String,
    pub detections: i32,
    pub mode: String,
    pub outcome: String,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "WafAuditEntry")]
pub struct AuditEntry {
    pub at: DateTime<Utc>,
    pub actor: String,
    pub action: String,
    pub target: String,
    pub detail: Option<String>,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "WafIpReport")]
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
    pub lines: Vec<String>,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "WafAuditPage")]
pub struct AuditPage {
    pub entries: Vec<AuditEntry>,
    pub total: i32,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "WafScorecard")]
pub struct Scorecard {
    pub decisions: i32,
    pub cidrs: i32,
    pub unblocked: i32,
    pub agreement: i32,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "WafWorkflow")]
pub struct Workflow {
    pub name: String,
    pub enabled: String,
    pub tier: String,
    pub window: String,
    pub duration: String,
    pub scorecard: Option<Scorecard>,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "WafWorkflows")]
pub struct Workflows {
    pub workflows: Vec<Workflow>,
    pub decisions: Vec<Decision>,
    pub cooldown: String,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "WafAllowlistEntry")]
pub struct AllowlistEntry {
    pub cidr: String,
    pub note: Option<String>,
    pub created_by: String,
    pub at: DateTime<Utc>,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "WafProtectedSource")]
pub struct ProtectedSource {
    pub source: String,
    pub count: i32,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "WafAllowlist")]
pub struct Allowlist {
    pub manual: Vec<AllowlistEntry>,
    pub protected: Vec<ProtectedSource>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query")]
pub struct CandidatesQuery {
    pub waf_candidates: Option<Candidates>,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct BlocksVariables {
    pub query: Option<String>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "BlocksVariables")]
pub struct BlocksQuery {
    #[arguments(query: $query)]
    pub waf_blocks: Option<Blocks>,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct IpVariables {
    pub addr: String,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "IpVariables")]
pub struct IpQuery {
    #[arguments(addr: $addr)]
    pub waf_ip: Option<IpReport>,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct AuditVariables {
    pub offset: i32,
    pub limit: i32,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "AuditVariables")]
pub struct AuditQuery {
    #[arguments(offset: $offset, limit: $limit)]
    pub waf_audit: Option<AuditPage>,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct WorkflowsVariables {
    pub workflow: Option<String>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "WorkflowsVariables")]
pub struct WorkflowsQuery {
    #[arguments(workflow: $workflow)]
    pub waf_workflows: Option<Workflows>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query")]
pub struct AllowlistQuery {
    pub waf_allowlist: Option<Allowlist>,
}

#[derive(cynic::InputObject, Debug)]
#[cynic(graphql_type = "WafCreateBlockInput")]
pub struct CreateBlockInput {
    pub cidr: String,
    pub gateway: Option<String>,
    pub reason: Option<String>,
    pub ttl_hours: Option<i32>,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct CreateBlockVariables {
    pub input: CreateBlockInput,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "CreateBlockVariables")]
pub struct CreateBlock {
    #[arguments(input: $input)]
    pub waf_create_block: Block,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct NameVariables {
    pub name: String,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "Mutation", variables = "NameVariables")]
pub struct DeleteBlock {
    #[arguments(name: $name)]
    pub waf_delete_block: cynic::Id,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "Mutation", variables = "NameVariables")]
pub struct MakeBlockPermanent {
    #[arguments(name: $name)]
    pub waf_make_block_permanent: Block,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct AllowlistVariables {
    pub cidr: String,
    pub note: Option<String>,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "Mutation", variables = "AllowlistVariables")]
pub struct AddAllowlist {
    #[arguments(cidr: $cidr, note: $note)]
    pub waf_add_allowlist: Allowlist,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct CidrVariables {
    pub cidr: String,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "Mutation", variables = "CidrVariables")]
pub struct RemoveAllowlist {
    #[arguments(cidr: $cidr)]
    pub waf_remove_allowlist: Allowlist,
}
