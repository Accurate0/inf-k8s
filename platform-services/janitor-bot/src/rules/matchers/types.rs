use crate::rules::schema::TemplateString;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Deserialize, JsonSchema, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Pr,
    Workflow,
    CommitStatus,
    CheckRun,
    Push,
    Sync,
}

#[derive(Debug, Deserialize, JsonSchema, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EventSource {
    Forgejo,
    Github,
    Argocd,
}

#[derive(Debug, Deserialize, JsonSchema, Clone, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum EventSourceSpec {
    One(EventSource),
    Many(Vec<EventSource>),
}

impl EventSourceSpec {
    pub fn matches(&self, source: EventSource) -> bool {
        match self {
            EventSourceSpec::One(s) => *s == source,
            EventSourceSpec::Many(ss) => ss.contains(&source),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Resource {
    PullRequest,
    PullRequestChangedFiles,
    Reviews,
    CombinedStatus,
    OpenPrs,
}

#[derive(Debug, Deserialize, JsonSchema, Clone)]
#[serde(untagged)]
pub enum Matcher {
    Combinator(Combinator),
    LeafExpr(LeafExprMatcher),
    Ref(MatcherRef),
    Leaf(LeafMatcher),
}

#[derive(Debug, Deserialize, JsonSchema, Clone)]
pub struct MatcherRef {
    #[serde(rename = "ref")]
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema, Clone)]
pub struct LeafExprMatcher {
    #[serde(flatten)]
    pub matcher: LeafMatcher,
    pub expr: String,
}

#[derive(Debug, Deserialize, JsonSchema, Clone)]
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
    #[serde(rename = "event")]
    Event {
        kind: EventKind,
        source: EventSourceSpec,
    },

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
    #[serde(rename = "bot.has_approved")]
    BotHasApproved,
    #[serde(rename = "bot.comment_contains")]
    BotCommentContains { marker: String, value: String },

    #[serde(rename = "feature_flag")]
    FeatureFlag {
        name: String,
        default: bool,
        #[serde(default)]
        context: BTreeMap<String, TemplateString>,
    },

    #[serde(rename = "time_of_day")]
    TimeOfDay {
        tz: String,
        after: String,
        before: String,
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
    IsLatestByMetadata {
        match_metadata_fields: Vec<String>,
        #[serde(default)]
        order_by_metadata_field: Option<String>,
    },

    #[serde(rename = "is_latest_published_image")]
    IsLatestPublishedImage {
        #[serde(default = "default_images_field")]
        images_metadata_field: String,
        #[serde(default = "default_tag_field")]
        tag_metadata_field: String,
        #[serde(default = "default_path_field")]
        path_metadata_field: String,
    },
}

fn default_images_field() -> String {
    "images".to_string()
}

fn default_tag_field() -> String {
    "tag".to_string()
}

fn default_path_field() -> String {
    "path".to_string()
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
#[serde(transparent)]
pub struct FilePattern(pub String);

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
            Matcher::Ref(r) => panic!("unresolved matcher ref `{}`", r.name),
        }
    }
}

impl LeafMatcher {
    pub fn kind(&self) -> &'static str {
        match self {
            LeafMatcher::Event { .. } => "event",
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
            LeafMatcher::BotHasApproved => "bot.has_approved",
            LeafMatcher::BotCommentContains { .. } => "bot.comment_contains",
            LeafMatcher::FeatureFlag { .. } => "feature_flag",
            LeafMatcher::TimeOfDay { .. } => "time_of_day",
            LeafMatcher::WorkflowConclusion { .. } => "workflow_conclusion",
            LeafMatcher::TargetBranch { .. } => "target_branch",
            LeafMatcher::Repository { .. } => "repository",
            LeafMatcher::WorkflowRunAttempt => "workflow_run_attempt",
            LeafMatcher::HasStatusChecks => "has_status_checks",
            LeafMatcher::AllStatusChecksPassed => "all_status_checks_passed",
            LeafMatcher::StatusChecks { .. } => "status_checks",
            LeafMatcher::IsLatestByMetadata { .. } => "is_latest_by_metadata",
            LeafMatcher::IsLatestPublishedImage { .. } => "is_latest_published_image",
        }
    }

    pub fn requires(&self) -> HashSet<Resource> {
        match self {
            LeafMatcher::IsOpen | LeafMatcher::HasConflicts => [Resource::PullRequest].into(),
            LeafMatcher::BotHasApproved => [Resource::Reviews].into(),
            LeafMatcher::HasStatusChecks
            | LeafMatcher::AllStatusChecksPassed
            | LeafMatcher::StatusChecks { .. } => {
                [Resource::PullRequest, Resource::CombinedStatus].into()
            }
            LeafMatcher::IsLatestByMetadata { .. } => {
                [Resource::PullRequest, Resource::OpenPrs].into()
            }
            LeafMatcher::IsLatestPublishedImage { .. } => [Resource::PullRequest].into(),
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
        let pattern = &self.0;
        let (negated, pat) = match pattern.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, pattern.as_str()),
        };
        let glob = globset::Glob::new(pat)
            .unwrap_or_else(|e| {
                tracing::warn!(pattern, "invalid glob pattern: {e}");
                globset::Glob::new("").unwrap()
            })
            .compile_matcher();
        let matched = glob.is_match(path);
        if negated { !matched } else { matched }
    }
}
