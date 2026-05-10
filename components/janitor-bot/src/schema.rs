use crate::event::PrEvent;
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
    #[serde(rename = "is_open")]
    IsOpen,
    #[serde(rename = "not_approved_by_self")]
    NotApprovedBySelf,
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
        ev: &'a PrEvent,
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
                    LeafMatcher::Action { value } => ev.action == *value,
                    LeafMatcher::Author { value } => ev.author == *value,
                    LeafMatcher::TitleContains { value } => ev.title.contains(value.as_str()),
                    LeafMatcher::HasLabel { value } => {
                        ev.labels.iter().any(|l| l.name == *value)
                    }
                    LeafMatcher::IsOpen => {
                        client
                            .is_pr_open(&ev.owner, &ev.repo, ev.pr_number as i64)
                            .await
                    }
                    LeafMatcher::NotApprovedBySelf => {
                        !client
                            .is_pr_approved_by_bot(&ev.owner, &ev.repo, ev.pr_number as i64)
                            .await
                    }
                    LeafMatcher::HasChangedFiles => !ev.changed_files.is_empty(),
                    LeafMatcher::ChangedFilesAllMatch { patterns } => {
                        !ev.changed_files.is_empty()
                            && ev
                                .changed_files
                                .iter()
                                .all(|f| patterns.iter().any(|p| p.matches(f)))
                    }
                },
            }
        })
    }
}

// --- Action deserialization ---

#[derive(Debug, Deserialize)]
pub struct LabelColor {
    pub name: String,
    pub color: String,
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
    EnsureLabelsExist { labels: Vec<LabelColor> },
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
            ActionDef::EnsureLabelsExist { labels } => Action::EnsureLabelsExist {
                labels: labels
                    .iter()
                    .map(|l| (l.name.clone(), l.color.clone()))
                    .collect(),
            },
        }
    }
}
