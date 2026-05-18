use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Matcher {
    Combinator(Combinator),
    LeafExpr(LeafExprMatcher),
    Leaf(LeafMatcher),
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LeafExprMatcher {
    #[serde(flatten)]
    pub matcher: LeafMatcher,
    pub expr: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub enum Combinator {
    #[serde(rename = "all")]
    All(Vec<Matcher>),
    #[serde(rename = "any")]
    Any(Vec<Matcher>),
    #[serde(rename = "not")]
    Not(Box<Matcher>),
}

#[derive(Debug, Deserialize, JsonSchema, Clone, PartialEq, Eq, Hash)]
#[serde(tag = "type")]
pub enum LeafMatcher {
    #[serde(rename = "forgejo")]
    Forgejo,
    #[serde(rename = "github")]
    GitHub,
    #[serde(rename = "argocd")]
    Argocd,

    #[serde(rename = "pr_event")]
    PrEvent,
    #[serde(rename = "workflow_event")]
    WorkflowEvent,
    #[serde(rename = "commit_status_event")]
    CommitStatusEvent,
    #[serde(rename = "check_run_event")]
    CheckRunEvent,
    #[serde(rename = "sync_event")]
    SyncEvent,

    #[serde(rename = "app_changed_in_commit")]
    AppChangedInCommit { owner: String, repo: String },

    #[serde(rename = "action")]
    Action { value: String },
    #[serde(rename = "author")]
    Author { value: String },
    #[serde(rename = "title_contains")]
    TitleContains { value: String },
    #[serde(rename = "has_label")]
    HasLabel { value: String },
    #[serde(rename = "has_changed_files")]
    HasChangedFiles,
    #[serde(rename = "changed_files_all_match")]
    ChangedFilesAllMatch { patterns: Vec<FilePattern> },
    #[serde(rename = "changed_files_any_match")]
    ChangedFilesAnyMatch { patterns: Vec<FilePattern> },
    #[serde(rename = "changed_files_none_match")]
    ChangedFilesNoneMatch { patterns: Vec<FilePattern> },
    #[serde(rename = "is_open")]
    IsOpen,
    #[serde(rename = "has_conflicts")]
    HasConflicts,
    #[serde(rename = "not_approved_by_self")]
    NotApprovedBySelf,

    #[serde(rename = "feature_flag")]
    FeatureFlag { name: String, default: bool },

    #[serde(rename = "time_window")]
    TimeWindow {
        timezone: String,
        start: u32,
        end: u32,
        #[serde(default)]
        weekdays_only: bool,
    },

    #[serde(rename = "workflow_conclusion")]
    WorkflowConclusion { value: String },
    #[serde(rename = "target_branch")]
    TargetBranch { value: String },
    #[serde(rename = "repository")]
    Repository { value: String },

    #[serde(rename = "workflow_run_attempt")]
    WorkflowRunAttempt,
}

#[derive(Debug, Deserialize, JsonSchema, Clone, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum FilePattern {
    StartsWith { starts_with: String },
    Contains { contains: String },
    EndsWith { ends_with: String },
    Glob(String),
}

impl FilePattern {
    pub fn matches(&self, path: &str) -> bool {
        match self {
            FilePattern::StartsWith { starts_with } => path.starts_with(starts_with.as_str()),
            FilePattern::Contains { contains } => path.contains(contains.as_str()),
            FilePattern::EndsWith { ends_with } => path.ends_with(ends_with.as_str()),
            FilePattern::Glob(pattern) => {
                let (negated, pat) = if let Some(rest) = pattern.strip_prefix('!') {
                    (true, rest)
                } else {
                    (false, pattern.as_str())
                };
                let glob = globset::Glob::new(pat)
                    .unwrap_or_else(|e| {
                        tracing::warn!(pattern, "invalid glob pattern: {e}");
                        // Fall back to a pattern that matches nothing
                        globset::Glob::new("").unwrap()
                    })
                    .compile_matcher();
                let matched = glob.is_match(path);
                if negated { !matched } else { matched }
            }
        }
    }
}
