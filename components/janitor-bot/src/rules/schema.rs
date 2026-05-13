use super::Action;
use super::matchers::Matcher;
use forgejo_api::structs::MergePullRequestOptionDo;
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RulesFile {
    pub rules: Vec<RuleDef>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RuleDef {
    pub name: String,
    #[serde(default)]
    pub enabled: RuleEnabled,
    pub matches: Matcher,
    pub actions: Vec<ActionDef>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum RuleEnabled {
    Bool(bool),
    Mode(EnabledMode),
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnabledMode {
    DryRun,
}

impl Default for RuleEnabled {
    fn default() -> Self {
        Self::Bool(false)
    }
}

impl RuleEnabled {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Bool(true) | Self::Mode(_))
    }
    pub fn is_dry_run(&self) -> bool {
        matches!(self, Self::Mode(EnabledMode::DryRun))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LabelColor {
    pub name: String,
    pub color: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct IssueTarget {
    pub owner: String,
    pub repo: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "type")]
pub enum ActionDef {
    #[serde(rename = "approve")]
    Approve {
        #[serde(default)]
        comment: Option<String>,
    },
    #[serde(rename = "merge")]
    Merge {
        strategy: MergeStrategy,
        #[serde(default = "default_true")]
        delete_branch: bool,
    },
    #[serde(rename = "comment")]
    Comment { body: String },
    #[serde(rename = "add_labels_by_name")]
    AddLabelsByName { labels: Vec<String> },
    #[serde(rename = "remove_labels_by_name")]
    RemoveLabelsByName { labels: Vec<String> },
    #[serde(rename = "ensure_labels_exist")]
    EnsureLabelsExist {
        labels: Vec<LabelColor>,
        #[serde(default)]
        target: Option<IssueTarget>,
    },
    #[serde(rename = "create_issue")]
    CreateIssue {
        target: IssueTarget,
        #[serde(rename = "deduplicateByTitle", default)]
        deduplicate_by_title: bool,
        title: String,
        body: String,
        #[serde(default)]
        comment_body: Option<String>,
        #[serde(default)]
        labels: Vec<LabelColor>,
    },
    #[serde(rename = "close_issue")]
    CloseIssue {
        target: IssueTarget,
        title: String,
        #[serde(default)]
        comment_body: Option<String>,
    },
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, JsonSchema, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum MergeStrategy {
    Merge,
    Squash,
    Rebase,
}

impl From<MergeStrategy> for MergePullRequestOptionDo {
    fn from(s: MergeStrategy) -> Self {
        match s {
            MergeStrategy::Merge => MergePullRequestOptionDo::Merge,
            MergeStrategy::Squash => MergePullRequestOptionDo::Squash,
            MergeStrategy::Rebase => MergePullRequestOptionDo::Rebase,
        }
    }
}

impl ActionDef {
    pub fn to_action(&self) -> Action {
        match self {
            ActionDef::Approve { comment } => Action::Approve {
                body: comment.clone(),
            },
            ActionDef::Merge {
                strategy,
                delete_branch,
            } => Action::Merge {
                strategy: (*strategy).into(),
                delete_branch: *delete_branch,
            },
            ActionDef::Comment { body } => Action::Comment { body: body.clone() },
            ActionDef::AddLabelsByName { labels } => Action::AddLabelsByName {
                labels: labels.clone(),
            },
            ActionDef::RemoveLabelsByName { labels } => Action::RemoveLabelsByName {
                labels: labels.clone(),
            },
            ActionDef::EnsureLabelsExist { labels, target } => Action::EnsureLabelsExist {
                labels: labels
                    .iter()
                    .map(|l| (l.name.clone(), l.color.clone()))
                    .collect(),
                target_owner: target.as_ref().map(|t| t.owner.clone()),
                target_repo: target.as_ref().map(|t| t.repo.clone()),
            },
            ActionDef::CreateIssue {
                target,
                deduplicate_by_title,
                title,
                body,
                comment_body,
                labels,
            } => Action::CreateIssue {
                target_owner: target.owner.clone(),
                target_repo: target.repo.clone(),
                deduplicate_by_title: *deduplicate_by_title,
                title: title.clone(),
                body: body.clone(),
                comment_body: comment_body.clone(),
                labels: labels
                    .iter()
                    .map(|l| (l.name.clone(), l.color.clone()))
                    .collect(),
            },
            ActionDef::CloseIssue {
                target,
                title,
                comment_body,
            } => Action::CloseIssue {
                target_owner: target.owner.clone(),
                target_repo: target.repo.clone(),
                title: title.clone(),
                comment_body: comment_body.clone(),
            },
        }
    }
}
