use super::Action;
use super::actions::RetryWorkflowTarget;
use super::matchers::Matcher;
use crate::event;
use forgejo_api::structs::MergePullRequestOptionDo;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize, JsonSchema, Clone)]
#[serde(transparent)]
pub struct TemplateString(pub String);

impl TemplateString {
    pub fn render(&self, vars: &HashMap<&str, String>) -> String {
        event::render_template(&self.0, vars)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for TemplateString {
    fn from(s: String) -> Self {
        Self(s)
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RulesFile {
    /// Repositories the bot knows about, keyed by Forgejo `owner/repo` slug.
    #[serde(default)]
    pub repos: Vec<RepoConfig>,
    #[serde(default)]
    pub checks: std::collections::HashMap<String, Matcher>,
    #[serde(default)]
    pub label_colors: std::collections::HashMap<String, String>,
    pub rules: Vec<RuleDef>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RepoConfig {
    /// Forgejo repo slug, `owner/repo`.
    pub repo: String,
    /// Whether the cron poller and admin endpoints process this repo.
    #[serde(default)]
    pub watched: bool,
    /// GitHub mirror slug, `owner/repo`, when this repo is mirrored to GitHub.
    #[serde(default)]
    pub github_repo: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RuleDef {
    pub name: String,
    #[serde(default)]
    pub disabled: Disabled,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub checks: std::collections::HashMap<String, Matcher>,
    pub when: Matcher,
    #[serde(default)]
    pub actions: Vec<ActionGroup>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ActionGroup {
    #[serde(default)]
    pub when: Option<Matcher>,
    pub run: Vec<ActionDef>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Disabled {
    Bool(bool),
    Reason(String),
}

impl Default for Disabled {
    fn default() -> Self {
        Self::Bool(false)
    }
}

impl Disabled {
    pub fn is_disabled(&self) -> bool {
        match self {
            Self::Bool(b) => *b,
            Self::Reason(_) => true,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema, Clone)]
#[serde(untagged)]
pub enum LabelSpec {
    Name(String),
    WithColor { name: String, color: String },
}

impl LabelSpec {
    pub fn name(&self) -> &str {
        match self {
            LabelSpec::Name(n) => n,
            LabelSpec::WithColor { name, .. } => name,
        }
    }

    pub fn color(&self) -> Option<&str> {
        match self {
            LabelSpec::Name(_) => None,
            LabelSpec::WithColor { color, .. } => Some(color),
        }
    }
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
        comment: Option<TemplateString>,
    },
    #[serde(rename = "merge")]
    Merge {
        strategy: MergeStrategy,
        #[serde(default = "default_true")]
        delete_branch: bool,
    },
    #[serde(rename = "comment")]
    Comment { comment: TemplateString },
    #[serde(rename = "add_labels")]
    AddLabels {
        labels: Vec<LabelSpec>,
        #[serde(default)]
        target: Option<IssueTarget>,
    },
    #[serde(rename = "remove_labels")]
    RemoveLabels {
        #[serde(default)]
        labels: Vec<String>,
        /// Remove any of the PR's current labels whose name starts with one of
        /// these prefixes (e.g. `janitor/` to clear all janitor labels).
        #[serde(default)]
        prefixes: Vec<String>,
    },
    #[serde(rename = "create_issue")]
    CreateIssue {
        target: IssueTarget,
        #[serde(rename = "deduplicateByTitle", default)]
        deduplicate_by_title: bool,
        title: TemplateString,
        body: TemplateString,
        #[serde(default)]
        on_duplicate_comment: Option<TemplateString>,
        #[serde(default)]
        labels: Vec<LabelSpec>,
    },
    #[serde(rename = "close_issue")]
    CloseIssue {
        target: IssueTarget,
        title: TemplateString,
        #[serde(default)]
        comment: Option<TemplateString>,
    },
    #[serde(rename = "argocd_diff")]
    ArgoCdDiff,
    #[serde(rename = "retry_workflow")]
    RetryWorkflow {
        target: RetryWorkflowTargetDef,
        repository: TemplateString,
        id: TemplateString,
    },
    #[serde(rename = "set_commit_status")]
    SetCommitStatus {
        target: IssueTarget,
        sha: TemplateString,
        state: TemplateString,
        context: TemplateString,
        description: TemplateString,
        target_url: TemplateString,
    },
    #[serde(rename = "wait_for_github_sync")]
    WaitForGithubSync {
        target: IssueTarget,
        sha: TemplateString,
        #[serde(default = "default_sync_timeout_secs")]
        timeout_secs: u64,
    },
    #[serde(rename = "proxy_pass")]
    ProxyPass { service: ProxyServiceDef },
    #[serde(rename = "close_other_prs")]
    CloseOtherPrs {
        author: String,
        criteria: CloseOtherPrsCriteria,
        match_metadata_fields: Vec<String>,
        #[serde(default)]
        order_by_metadata_field: Option<String>,
        #[serde(default)]
        delete_branch: bool,
        #[serde(default)]
        comment: Option<TemplateString>,
    },
}

#[derive(Debug, Deserialize, JsonSchema, Clone)]
#[serde(rename_all = "snake_case")]
pub enum CloseOtherPrsCriteria {
    Older,
}

fn default_true() -> bool {
    true
}

fn default_sync_timeout_secs() -> u64 {
    30
}

#[derive(Debug, Deserialize, JsonSchema, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum RetryWorkflowTargetDef {
    Github,
}

/// Target service for the `proxy_pass` action. The original inbound request
/// (body + headers) is forwarded verbatim to the service's webhook endpoint.
#[derive(Debug, Deserialize, JsonSchema, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum ProxyServiceDef {
    Argocd,
}

impl From<ProxyServiceDef> for crate::rules::actions::ProxyService {
    fn from(def: ProxyServiceDef) -> Self {
        match def {
            ProxyServiceDef::Argocd => Self::Argocd,
        }
    }
}

impl From<&RetryWorkflowTargetDef> for RetryWorkflowTarget {
    fn from(def: &RetryWorkflowTargetDef) -> Self {
        match def {
            RetryWorkflowTargetDef::Github => RetryWorkflowTarget::GitHub,
        }
    }
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
                comment: comment.clone(),
            },
            ActionDef::Merge {
                strategy,
                delete_branch,
            } => Action::Merge {
                strategy: (*strategy).into(),
                delete_branch: *delete_branch,
            },
            ActionDef::Comment { comment } => Action::Comment {
                comment: comment.clone(),
            },
            ActionDef::AddLabels { labels, target } => Action::AddLabels {
                labels: labels.clone(),
                target_owner: target.as_ref().map(|t| t.owner.clone()),
                target_repo: target.as_ref().map(|t| t.repo.clone()),
            },
            ActionDef::RemoveLabels { labels, prefixes } => Action::RemoveLabels {
                labels: labels.clone(),
                prefixes: prefixes.clone(),
            },
            ActionDef::CreateIssue {
                target,
                deduplicate_by_title,
                title,
                body,
                on_duplicate_comment,
                labels,
            } => Action::CreateIssue {
                target_owner: target.owner.clone(),
                target_repo: target.repo.clone(),
                deduplicate_by_title: *deduplicate_by_title,
                title: title.clone(),
                body: body.clone(),
                on_duplicate_comment: on_duplicate_comment.clone(),
                labels: labels.clone(),
            },
            ActionDef::CloseIssue {
                target,
                title,
                comment,
            } => Action::CloseIssue {
                target_owner: target.owner.clone(),
                target_repo: target.repo.clone(),
                title: title.clone(),
                comment: comment.clone(),
            },
            ActionDef::ArgoCdDiff => Action::ArgoCdDiff,
            ActionDef::RetryWorkflow {
                target,
                repository,
                id,
            } => Action::RetryWorkflow {
                target: target.into(),
                repository: repository.clone(),
                id: id.clone(),
            },
            ActionDef::SetCommitStatus {
                target,
                sha,
                state,
                context,
                description,
                target_url,
            } => Action::SetCommitStatus {
                target_owner: target.owner.clone(),
                target_repo: target.repo.clone(),
                sha: sha.clone(),
                state: state.clone(),
                context: context.clone(),
                description: description.clone(),
                target_url: target_url.clone(),
            },
            ActionDef::WaitForGithubSync {
                target,
                sha,
                timeout_secs,
            } => Action::WaitForGithubSync {
                target_owner: target.owner.clone(),
                target_repo: target.repo.clone(),
                sha: sha.clone(),
                timeout_secs: *timeout_secs,
            },
            ActionDef::ProxyPass { service } => Action::ProxyPass {
                service: (*service).into(),
            },
            ActionDef::CloseOtherPrs {
                author,
                criteria,
                match_metadata_fields,
                order_by_metadata_field,
                delete_branch,
                comment,
            } => Action::CloseOtherPrs {
                author: author.clone(),
                criteria: criteria.clone(),
                match_metadata_fields: match_metadata_fields.clone(),
                order_by_metadata_field: order_by_metadata_field.clone(),
                delete_branch: *delete_branch,
                comment: comment.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_string_plain_passthrough() {
        let ts = TemplateString("hello".to_string());
        let vars = HashMap::new();
        assert_eq!(ts.render(&vars), "hello");
    }

    #[test]
    fn template_string_renders_placeholders() {
        let ts = TemplateString("hello {name}".to_string());
        let mut vars = HashMap::new();
        vars.insert("name", "world".to_string());
        assert_eq!(ts.render(&vars), "hello world");
    }

    #[test]
    fn deserialize_template_string_from_plain() {
        let ts: TemplateString = yaml_serde::from_str("\"just a string\"").unwrap();
        assert_eq!(ts.as_str(), "just a string");
    }

    #[test]
    fn deserialize_unconditional_group() {
        let yaml = r#"
name: test-rule
when:
  type: event
  kind: pr
  source: forgejo
actions:
  - run:
      - type: approve
      - type: merge
        strategy: squash
"#;
        let rule: RuleDef = yaml_serde::from_str(yaml).unwrap();
        assert_eq!(rule.name, "test-rule");
        assert_eq!(rule.actions.len(), 1);
        assert!(rule.actions[0].when.is_none());
        assert_eq!(rule.actions[0].run.len(), 2);
    }

    #[test]
    fn deserialize_group_with_when_matcher() {
        let yaml = r#"
name: test-rule
when:
  type: event
  kind: pr
  source: forgejo
actions:
  - when:
      type: time_of_day
      tz: UTC
      after: "09:00"
      before: "17:00"
    run:
      - type: approve
  - run:
      - type: add_labels
        labels: [queued]
"#;
        let rule: RuleDef = yaml_serde::from_str(yaml).unwrap();
        assert_eq!(rule.actions.len(), 2);
        assert!(rule.actions[0].when.is_some());
        assert!(rule.actions[1].when.is_none());
    }

    #[test]
    fn deserialize_label_spec_name_only() {
        let yaml = r#"
type: add_labels
labels: [bug, urgent]
"#;
        let def: ActionDef = yaml_serde::from_str(yaml).unwrap();
        match def {
            ActionDef::AddLabels { labels, .. } => {
                assert_eq!(labels.len(), 2);
                assert!(matches!(labels[0], LabelSpec::Name(ref n) if n == "bug"));
            }
            _ => panic!("expected add_labels"),
        }
    }

    #[test]
    fn deserialize_label_spec_with_color() {
        let yaml = r##"
type: add_labels
labels:
  - name: bug
    color: "#d73a4a"
  - urgent
"##;
        let def: ActionDef = yaml_serde::from_str(yaml).unwrap();
        match def {
            ActionDef::AddLabels { labels, .. } => {
                assert_eq!(labels.len(), 2);
                assert!(matches!(labels[0], LabelSpec::WithColor { .. }));
                assert_eq!(labels[0].name(), "bug");
                assert_eq!(labels[0].color(), Some("#d73a4a"));
                assert_eq!(labels[1].name(), "urgent");
                assert_eq!(labels[1].color(), None);
            }
            _ => panic!("expected add_labels"),
        }
    }

    #[test]
    fn disabled_default_is_enabled() {
        assert!(!Disabled::default().is_disabled());
    }

    #[test]
    fn disabled_true_disables() {
        assert!(Disabled::Bool(true).is_disabled());
    }

    #[test]
    fn disabled_with_reason_disables() {
        assert!(Disabled::Reason("broken".into()).is_disabled());
    }

    #[test]
    fn deserialize_dry_run_field() {
        let yaml = r#"
name: test
dry_run: true
when:
  type: event
  kind: pr
  source: forgejo
actions: []
"#;
        let rule: RuleDef = yaml_serde::from_str(yaml).unwrap();
        assert!(rule.dry_run);
        assert!(!rule.disabled.is_disabled());
    }

    #[test]
    fn deserialize_disabled_reason() {
        let yaml = r#"
name: test
disabled: "broken since 2026-04"
when:
  type: event
  kind: pr
  source: forgejo
actions: []
"#;
        let rule: RuleDef = yaml_serde::from_str(yaml).unwrap();
        assert!(rule.disabled.is_disabled());
    }

    #[test]
    fn merge_strategy_conversion() {
        assert!(matches!(
            MergePullRequestOptionDo::from(MergeStrategy::Squash),
            MergePullRequestOptionDo::Squash
        ));
    }

    #[test]
    fn deserialize_rules_file() {
        let yaml = r#"
rules:
  - name: rule1
    when:
      type: event
      kind: pr
      source: forgejo
    actions:
      - run:
          - type: approve
  - name: rule2
    disabled: true
    when:
      type: event
      kind: workflow
      source: github
    actions: []
"#;
        let file: RulesFile = yaml_serde::from_str(yaml).unwrap();
        assert_eq!(file.rules.len(), 2);
        assert!(!file.rules[0].disabled.is_disabled());
        assert!(file.rules[1].disabled.is_disabled());
    }
}
