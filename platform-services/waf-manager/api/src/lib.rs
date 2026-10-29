use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub name: String,
    pub cidr: String,
    pub gateway: String,
    pub reason: Option<String>,
    pub created_by: Option<String>,
    pub automatic: bool,
    pub created_at: Option<DateTime<Utc>>,
    pub expires_at: Option<String>,
    pub enforced: bool,
    pub strikes: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conflict {
    pub source: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blocks {
    pub blocks: Vec<Block>,
    pub conflicts: Vec<Conflict>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateBlock {
    pub cidr: String,
    #[serde(default)]
    pub gateway: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub ttl_hours: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub client_ip: String,
    pub detections: u64,
    #[serde(default)]
    pub protected_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidates {
    pub window: String,
    pub actionable: Vec<Candidate>,
    pub protected: Vec<Candidate>,
    pub blocked: Vec<Candidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub rule_id: String,
    pub rule_msg: String,
    pub severity: String,
    pub count: u64,
    pub ignored: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UriHit {
    pub uri: String,
    pub count: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Enrichment {
    pub hostname: Option<String>,
    pub asn: Option<String>,
    pub org: Option<String>,
    pub prefix: Option<String>,
    pub country: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub at: DateTime<Utc>,
    pub workflow: String,
    pub cidr: String,
    pub detections: u64,
    pub mode: String,
    pub outcome: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub at: DateTime<Utc>,
    pub actor: String,
    pub action: String,
    pub target: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditPage {
    pub entries: Vec<AuditEntry>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpDetail {
    pub client_ip: String,
    pub cidr: String,
    pub window: String,
    pub strikes: i32,
    pub protected: Option<String>,
    pub block: Option<Block>,
    pub decisions: Vec<Decision>,
    pub audit: Vec<AuditEntry>,
    pub enrichment: Option<Enrichment>,
    pub rules: Vec<Rule>,
    pub uris: Vec<UriHit>,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    pub name: String,
    pub enabled: String,
    pub tier: String,
    pub window: String,
    pub duration: String,
    pub gateway: String,
    pub reason: String,
    pub signals: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scorecard {
    pub workflow: String,
    pub enabled: String,
    pub decisions: i64,
    pub cidrs: i64,
    pub unblocked: i64,
    pub agreement: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflows {
    pub workflows: Vec<Workflow>,
    pub scorecard: Vec<Scorecard>,
    pub decisions: Vec<Decision>,
    pub cooldown: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowlistEntry {
    pub cidr: String,
    pub note: Option<String>,
    pub created_by: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtectedSource {
    pub source: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Allowlist {
    pub manual: Vec<AllowlistEntry>,
    pub protected: Vec<ProtectedSource>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateAllowlist {
    pub cidr: String,
    #[serde(default)]
    pub note: Option<String>,
}
