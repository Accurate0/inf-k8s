use crate::event::BotEvent;
use crate::forgejo::ForgejoClient;
use crate::rules::Action;
use forgejo_api::structs::MergePullRequestOptionDo;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

#[derive(Debug, Deserialize)]
pub struct RulesFile {
    pub rules: Vec<RuleDef>,
}

#[derive(Debug, Deserialize)]
pub struct RuleDef {
    pub name: String,
    pub matches: Matcher,
    pub actions: Vec<ActionDef>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Matcher {
    Combinator(Combinator),
    Leaf(LeafMatcher),
}

#[derive(Debug, Deserialize)]
pub enum Combinator {
    #[serde(rename = "all")]
    All(Vec<Matcher>),
    #[serde(rename = "any")]
    Any(Vec<Matcher>),
    #[serde(rename = "not")]
    Not(Box<Matcher>),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum LeafMatcher {
    // Source matchers
    #[serde(rename = "forgejo")]
    Forgejo,
    #[serde(rename = "github")]
    GitHub,

    // Event type matchers
    #[serde(rename = "pr_event")]
    PrEvent,
    #[serde(rename = "workflow_event")]
    WorkflowEvent,

    // PR-specific matchers
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
    #[serde(rename = "changed_files_none_match")]
    ChangedFilesNoneMatch { patterns: Vec<FilePattern> },
    #[serde(rename = "is_open")]
    IsOpen,
    #[serde(rename = "not_approved_by_self")]
    NotApprovedBySelf,

    // Workflow-specific matchers
    #[serde(rename = "workflow_conclusion")]
    WorkflowConclusion { value: String },
    #[serde(rename = "target_branch")]
    TargetBranch { value: String },
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum FilePattern {
    StartsWith { starts_with: String },
    Contains { contains: String },
    EndsWith { ends_with: String },
}

impl FilePattern {
    fn matches(&self, path: &str) -> bool {
        match self {
            FilePattern::StartsWith { starts_with } => path.starts_with(starts_with.as_str()),
            FilePattern::Contains { contains } => path.contains(contains.as_str()),
            FilePattern::EndsWith { ends_with } => path.ends_with(ends_with.as_str()),
        }
    }
}

impl Matcher {
    pub fn matches<'a>(
        &'a self,
        ev: &'a BotEvent<'a>,
        client: &'a ForgejoClient,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async move {
            match self {
                Matcher::Combinator(c) => match c {
                    Combinator::All(matchers) => {
                        for m in matchers {
                            if !m.matches(ev, client).await {
                                return false;
                            }
                        }
                        true
                    }
                    Combinator::Any(matchers) => {
                        for m in matchers {
                            if m.matches(ev, client).await {
                                return true;
                            }
                        }
                        false
                    }
                    Combinator::Not(matcher) => !matcher.matches(ev, client).await,
                },
                Matcher::Leaf(leaf) => match leaf {
                    // Source matchers
                    LeafMatcher::Forgejo => matches!(ev, BotEvent::ForgejoPr(_)),
                    LeafMatcher::GitHub => matches!(ev, BotEvent::GitHubWorkflow(_)),
                    LeafMatcher::PrEvent => matches!(ev, BotEvent::ForgejoPr(_)),
                    LeafMatcher::WorkflowEvent => matches!(ev, BotEvent::GitHubWorkflow(_)),

                    // PR-specific matchers (only match on ForgejoPr)
                    LeafMatcher::Action { value } => match ev {
                        BotEvent::ForgejoPr(pr) => pr.action == *value,
                        _ => false,
                    },
                    LeafMatcher::Author { value } => match ev {
                        BotEvent::ForgejoPr(pr) => pr.author == *value,
                        _ => false,
                    },
                    LeafMatcher::TitleContains { value } => match ev {
                        BotEvent::ForgejoPr(pr) => pr.title.contains(value.as_str()),
                        _ => false,
                    },
                    LeafMatcher::HasLabel { value } => match ev {
                        BotEvent::ForgejoPr(pr) => pr.labels.iter().any(|l| l.name == *value),
                        _ => false,
                    },
                    LeafMatcher::IsOpen => match ev {
                        BotEvent::ForgejoPr(pr) => {
                            client
                                .is_pr_open(&pr.owner, &pr.repo, pr.pr_number as i64)
                                .await
                        }
                        _ => false,
                    },
                    LeafMatcher::NotApprovedBySelf => match ev {
                        BotEvent::ForgejoPr(pr) => {
                            !client
                                .is_pr_approved_by_bot(&pr.owner, &pr.repo, pr.pr_number as i64)
                                .await
                        }
                        _ => false,
                    },
                    LeafMatcher::HasChangedFiles => match ev {
                        BotEvent::ForgejoPr(pr) => !pr.changed_files.is_empty(),
                        _ => false,
                    },
                    LeafMatcher::ChangedFilesAllMatch { patterns } => match ev {
                        BotEvent::ForgejoPr(pr) => {
                            !pr.changed_files.is_empty()
                                && pr
                                    .changed_files
                                    .iter()
                                    .all(|f| patterns.iter().any(|p| p.matches(f)))
                        }
                        _ => false,
                    },
                    LeafMatcher::ChangedFilesNoneMatch { patterns } => match ev {
                        BotEvent::ForgejoPr(pr) => pr
                            .changed_files
                            .iter()
                            .all(|f| !patterns.iter().any(|p| p.matches(f))),
                        _ => false,
                    },

                    // Workflow-specific matchers
                    LeafMatcher::WorkflowConclusion { value } => match ev {
                        BotEvent::GitHubWorkflow(wf) => wf.conclusion == *value,
                        _ => false,
                    },
                    LeafMatcher::TargetBranch { value } => match ev {
                        BotEvent::ForgejoPr(pr) => pr.target_branch == *value,
                        BotEvent::GitHubWorkflow(wf) => wf.branch == *value,
                    },
                },
            }
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct LabelColor {
    pub name: String,
    pub color: String,
}

#[derive(Debug, Deserialize)]
pub struct IssueTarget {
    pub owner: String,
    pub repo: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ActionDef {
    #[serde(rename = "approve")]
    Approve { body: String },
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
    #[serde(rename = "ensure_labels_exist")]
    EnsureLabelsExist {
        labels: Vec<LabelColor>,
        #[serde(default)]
        target: Option<IssueTarget>,
    },
    #[serde(rename = "create_issue")]
    CreateIssue {
        target: IssueTarget,
        dedup_key: String,
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
        dedup_key: String,
        #[serde(default)]
        comment_body: Option<String>,
    },
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, Clone, Copy)]
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
            ActionDef::Approve { body } => Action::Approve { body: body.clone() },
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
                dedup_key,
                title,
                body,
                comment_body,
                labels,
            } => Action::CreateIssue {
                target_owner: target.owner.clone(),
                target_repo: target.repo.clone(),
                dedup_key: dedup_key.clone(),
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
                dedup_key,
                comment_body,
            } => Action::CloseIssue {
                target_owner: target.owner.clone(),
                target_repo: target.repo.clone(),
                dedup_key: dedup_key.clone(),
                comment_body: comment_body.clone(),
            },
        }
    }
}
