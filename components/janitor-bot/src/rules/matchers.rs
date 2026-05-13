use crate::event::BotEvent;
use crate::forgejo::ForgejoClient;
use chrono::{Datelike, Timelike};
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Matcher {
    Combinator(Combinator),
    Leaf(LeafMatcher),
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

#[derive(Debug, Deserialize, JsonSchema)]
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
    #[serde(rename = "has_conflicts")]
    HasConflicts,
    #[serde(rename = "not_approved_by_self")]
    NotApprovedBySelf,

    // Time matchers
    #[serde(rename = "time_window")]
    TimeWindow {
        timezone: String,
        start: u32,
        end: u32,
        #[serde(default)]
        weekdays_only: bool,
    },

    // Workflow-specific matchers
    #[serde(rename = "workflow_conclusion")]
    WorkflowConclusion { value: String },
    #[serde(rename = "target_branch")]
    TargetBranch { value: String },
}

#[derive(Debug, Deserialize, JsonSchema)]
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
                    LeafMatcher::HasConflicts => match ev {
                        BotEvent::ForgejoPr(pr) => {
                            client
                                .is_pr_mergeable(&pr.owner, &pr.repo, pr.pr_number as i64)
                                .await
                                == Some(false)
                        }
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

                    // Time matchers
                    LeafMatcher::TimeWindow {
                        timezone,
                        start,
                        end,
                        weekdays_only,
                    } => {
                        let tz: chrono_tz::Tz = match timezone.parse() {
                            Ok(tz) => tz,
                            Err(_) => {
                                tracing::warn!(timezone, "invalid timezone, defaulting to false");
                                return false;
                            }
                        };
                        let now = chrono::Utc::now().with_timezone(&tz);
                        let hour = now.hour();
                        if *weekdays_only {
                            let weekday = now.weekday();
                            if matches!(weekday, chrono::Weekday::Sat | chrono::Weekday::Sun) {
                                return false;
                            }
                        }
                        hour >= *start && hour < *end
                    }

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
