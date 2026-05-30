use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Resource {
    PullRequest,
    PullRequestChangedFiles,
    Reviews,
    CombinedStatus,
    OpenPrs,
}

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
    #[serde(rename = "title_matches")]
    TitleMatches {
        value: String,
        #[serde(default)]
        mode: StringMatchMode,
    },
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
    #[serde(rename = "is_merged")]
    IsMerged,
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

    #[serde(rename = "has_status_checks")]
    HasStatusChecks,
    #[serde(rename = "all_status_checks_passed")]
    AllStatusChecksPassed,
    #[serde(rename = "status_checks")]
    StatusChecks {
        names: Vec<String>,
        state: StatusCheckState,
    },

    #[serde(rename = "is_latest_by_metadata")]
    IsLatestByMetadata { match_metadata_fields: Vec<String> },
}

#[derive(Debug, Deserialize, JsonSchema, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum StatusCheckState {
    Pending,
    Success,
    Error,
    Failure,
    Warning,
}

impl StatusCheckState {
    pub fn matches(&self, other: &forgejo_api::structs::CommitStatusState) -> bool {
        use forgejo_api::structs::CommitStatusState as F;
        matches!(
            (self, other),
            (Self::Pending, F::Pending)
                | (Self::Success, F::Success)
                | (Self::Error, F::Error)
                | (Self::Failure, F::Failure)
                | (Self::Warning, F::Warning)
        )
    }
}

#[derive(Debug, Deserialize, JsonSchema, Default, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum StringMatchMode {
    #[default]
    Contains,
    ContainsIgnoreCase,
    Equals,
    EqualsIgnoreCase,
    StartsWith,
    EndsWith,
}

impl StringMatchMode {
    pub fn matches(&self, haystack: &str, needle: &str) -> bool {
        match self {
            Self::Contains => haystack.contains(needle),
            Self::ContainsIgnoreCase => haystack.to_lowercase().contains(&needle.to_lowercase()),
            Self::Equals => haystack == needle,
            Self::EqualsIgnoreCase => haystack.eq_ignore_ascii_case(needle),
            Self::StartsWith => haystack.starts_with(needle),
            Self::EndsWith => haystack.ends_with(needle),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema, Clone, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum FilePattern {
    StartsWith { starts_with: String },
    Contains { contains: String },
    EndsWith { ends_with: String },
    Glob(String),
}

impl Matcher {
    pub fn requires(&self) -> HashSet<Resource> {
        match self {
            Matcher::Leaf(leaf) | Matcher::LeafExpr(LeafExprMatcher { matcher: leaf, .. }) => {
                leaf.requires()
            }
            Matcher::Combinator(c) => match c {
                Combinator::All(ms) | Combinator::Any(ms) => {
                    ms.iter().flat_map(|m| m.requires()).collect()
                }
                Combinator::Not(m) => m.requires(),
            },
        }
    }
}

impl LeafMatcher {
    pub fn kind(&self) -> &'static str {
        match self {
            LeafMatcher::Forgejo => "forgejo",
            LeafMatcher::GitHub => "github",
            LeafMatcher::Argocd => "argocd",
            LeafMatcher::PrEvent => "pr_event",
            LeafMatcher::WorkflowEvent => "workflow_event",
            LeafMatcher::CommitStatusEvent => "commit_status_event",
            LeafMatcher::CheckRunEvent => "check_run_event",
            LeafMatcher::SyncEvent => "sync_event",
            LeafMatcher::AppChangedInCommit { .. } => "app_changed_in_commit",
            LeafMatcher::Action { .. } => "action",
            LeafMatcher::Author { .. } => "author",
            LeafMatcher::TitleMatches { .. } => "title_matches",
            LeafMatcher::HasLabel { .. } => "has_label",
            LeafMatcher::HasChangedFiles => "has_changed_files",
            LeafMatcher::ChangedFilesAllMatch { .. } => "changed_files_all_match",
            LeafMatcher::ChangedFilesAnyMatch { .. } => "changed_files_any_match",
            LeafMatcher::ChangedFilesNoneMatch { .. } => "changed_files_none_match",
            LeafMatcher::IsOpen => "is_open",
            LeafMatcher::IsMerged => "is_merged",
            LeafMatcher::HasConflicts => "has_conflicts",
            LeafMatcher::NotApprovedBySelf => "not_approved_by_self",
            LeafMatcher::FeatureFlag { .. } => "feature_flag",
            LeafMatcher::TimeWindow { .. } => "time_window",
            LeafMatcher::WorkflowConclusion { .. } => "workflow_conclusion",
            LeafMatcher::TargetBranch { .. } => "target_branch",
            LeafMatcher::Repository { .. } => "repository",
            LeafMatcher::WorkflowRunAttempt => "workflow_run_attempt",
            LeafMatcher::HasStatusChecks => "has_status_checks",
            LeafMatcher::AllStatusChecksPassed => "all_status_checks_passed",
            LeafMatcher::StatusChecks { .. } => "status_checks",
            LeafMatcher::IsLatestByMetadata { .. } => "is_latest_by_metadata",
        }
    }

    pub fn requires(&self) -> HashSet<Resource> {
        match self {
            LeafMatcher::IsOpen | LeafMatcher::HasConflicts => [Resource::PullRequest].into(),
            LeafMatcher::NotApprovedBySelf => [Resource::Reviews].into(),
            LeafMatcher::HasStatusChecks
            | LeafMatcher::AllStatusChecksPassed
            | LeafMatcher::StatusChecks { .. } => {
                [Resource::PullRequest, Resource::CombinedStatus].into()
            }
            LeafMatcher::IsLatestByMetadata { .. } => {
                [Resource::PullRequest, Resource::OpenPrs].into()
            }
            LeafMatcher::HasChangedFiles
            | LeafMatcher::ChangedFilesAllMatch { .. }
            | LeafMatcher::ChangedFilesAnyMatch { .. }
            | LeafMatcher::ChangedFilesNoneMatch { .. } => {
                [Resource::PullRequestChangedFiles].into()
            }
            _ => HashSet::new(),
        }
    }
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
