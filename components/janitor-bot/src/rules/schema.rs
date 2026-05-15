use super::Action;
use super::matchers::Matcher;
use crate::event;
use forgejo_api::structs::MergePullRequestOptionDo;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize, JsonSchema, Clone)]
#[serde(untagged)]
pub enum TemplateString {
    Object {
        content: String,
        #[serde(default)]
        template: bool,
    },
    Plain(String),
}

impl TemplateString {
    pub fn render(&self, vars: &HashMap<&str, String>) -> String {
        match self {
            TemplateString::Plain(s) => s.clone(),
            TemplateString::Object { content, template } => {
                if *template {
                    event::render_template(content, vars)
                } else {
                    content.clone()
                }
            }
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            TemplateString::Plain(s) => s,
            TemplateString::Object { content, .. } => content,
        }
    }
}

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
    #[serde(default)]
    pub variables: Vec<VariableDef>,
    pub actions: ActionsDef,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VariableDef {
    pub var: String,
    #[serde(flatten)]
    pub matcher: Matcher,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ActionsDef {
    Conditional(Vec<ConditionalActionGroup>),
    Flat(Vec<ActionDef>),
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConditionalActionGroup {
    pub when: String,
    pub run: Vec<ActionDef>,
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
        comment: Option<TemplateString>,
    },
    #[serde(rename = "merge")]
    Merge {
        strategy: MergeStrategy,
        #[serde(default = "default_true")]
        delete_branch: bool,
    },
    #[serde(rename = "comment")]
    Comment { body: TemplateString },
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
        title: TemplateString,
        body: TemplateString,
        #[serde(default)]
        on_duplicate_comment: Option<TemplateString>,
        #[serde(default)]
        labels: Vec<LabelColor>,
    },
    #[serde(rename = "close_issue")]
    CloseIssue {
        target: IssueTarget,
        title: TemplateString,
        #[serde(default)]
        closing_comment: Option<TemplateString>,
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

impl Default for ActionsDef {
    fn default() -> Self {
        Self::Flat(Vec::new())
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
                on_duplicate_comment,
                labels,
            } => Action::CreateIssue {
                target_owner: target.owner.clone(),
                target_repo: target.repo.clone(),
                deduplicate_by_title: *deduplicate_by_title,
                title: title.clone(),
                body: body.clone(),
                on_duplicate_comment: on_duplicate_comment.clone(),
                labels: labels
                    .iter()
                    .map(|l| (l.name.clone(), l.color.clone()))
                    .collect(),
            },
            ActionDef::CloseIssue {
                target,
                title,
                closing_comment,
            } => Action::CloseIssue {
                target_owner: target.owner.clone(),
                target_repo: target.repo.clone(),
                title: title.clone(),
                closing_comment: closing_comment.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_string_plain_no_rendering() {
        let ts = TemplateString::Plain("hello {name}".to_string());
        let mut vars = HashMap::new();
        vars.insert("name", "world".to_string());
        assert_eq!(ts.render(&vars), "hello {name}");
    }

    #[test]
    fn template_string_object_template_false() {
        let ts = TemplateString::Object {
            content: "hello {name}".to_string(),
            template: false,
        };
        let mut vars = HashMap::new();
        vars.insert("name", "world".to_string());
        assert_eq!(ts.render(&vars), "hello {name}");
    }

    #[test]
    fn template_string_object_template_true() {
        let ts = TemplateString::Object {
            content: "hello {name}".to_string(),
            template: true,
        };
        let mut vars = HashMap::new();
        vars.insert("name", "world".to_string());
        assert_eq!(ts.render(&vars), "hello world");
    }

    #[test]
    fn deserialize_template_string_from_plain() {
        let yaml = "\"just a string\"";
        let ts: TemplateString = yaml_serde::from_str(yaml).unwrap();
        assert!(matches!(ts, TemplateString::Plain(_)));
        assert_eq!(ts.as_str(), "just a string");
    }

    #[test]
    fn deserialize_template_string_from_object() {
        let yaml = "content: \"hello {x}\"\ntemplate: true";
        let ts: TemplateString = yaml_serde::from_str(yaml).unwrap();
        match ts {
            TemplateString::Object { content, template } => {
                assert_eq!(content, "hello {x}");
                assert!(template);
            }
            _ => panic!("expected Object"),
        }
    }

    #[test]
    fn deserialize_template_string_object_defaults_template_false() {
        let yaml = "content: \"static\"";
        let ts: TemplateString = yaml_serde::from_str(yaml).unwrap();
        match ts {
            TemplateString::Object { template, .. } => assert!(!template),
            _ => panic!("expected Object"),
        }
    }

    #[test]
    fn deserialize_flat_actions() {
        let yaml = r#"
name: test-rule
enabled: true
matches:
  type: forgejo
actions:
  - type: approve
  - type: merge
    strategy: squash
"#;
        let rule: RuleDef = yaml_serde::from_str(yaml).unwrap();
        assert_eq!(rule.name, "test-rule");
        assert!(rule.variables.is_empty());
        assert!(matches!(rule.actions, ActionsDef::Flat(ref a) if a.len() == 2));
    }

    #[test]
    fn deserialize_conditional_actions() {
        let yaml = r#"
name: test-rule
enabled: true
matches:
  type: forgejo
variables:
  - var: in_window
    type: time_window
    timezone: Australia/Perth
    start: 17
    end: 22
actions:
  - when: "in_window"
    run:
      - type: approve
      - type: merge
        strategy: squash
  - when: "!in_window"
    run:
      - type: add_labels_by_name
        labels: [queued]
"#;
        let rule: RuleDef = yaml_serde::from_str(yaml).unwrap();
        assert_eq!(rule.variables.len(), 1);
        assert_eq!(rule.variables[0].var, "in_window");
        match &rule.actions {
            ActionsDef::Conditional(groups) => {
                assert_eq!(groups.len(), 2);
                assert_eq!(groups[0].when, "in_window");
                assert_eq!(groups[0].run.len(), 2);
                assert_eq!(groups[1].when, "!in_window");
                assert_eq!(groups[1].run.len(), 1);
            }
            ActionsDef::Flat(_) => panic!("expected conditional actions"),
        }
    }

    #[test]
    fn deserialize_no_variables_defaults_empty() {
        let yaml = r#"
name: test
enabled: true
matches:
  type: forgejo
actions:
  - type: comment
    body: hello
"#;
        let rule: RuleDef = yaml_serde::from_str(yaml).unwrap();
        assert!(rule.variables.is_empty());
    }

    #[test]
    fn rule_enabled_default_is_false() {
        let enabled = RuleEnabled::default();
        assert!(!enabled.is_active());
        assert!(!enabled.is_dry_run());
    }

    #[test]
    fn rule_enabled_true() {
        let enabled = RuleEnabled::Bool(true);
        assert!(enabled.is_active());
        assert!(!enabled.is_dry_run());
    }

    #[test]
    fn rule_enabled_dry_run() {
        let enabled = RuleEnabled::Mode(EnabledMode::DryRun);
        assert!(enabled.is_active());
        assert!(enabled.is_dry_run());
    }

    #[test]
    fn deserialize_dry_run_mode() {
        let yaml = r#"
name: test
enabled: dry_run
matches:
  type: forgejo
actions: []
"#;
        let rule: RuleDef = yaml_serde::from_str(yaml).unwrap();
        assert!(rule.enabled.is_active());
        assert!(rule.enabled.is_dry_run());
    }

    #[test]
    fn merge_strategy_conversion() {
        assert!(matches!(
            MergePullRequestOptionDo::from(MergeStrategy::Merge),
            MergePullRequestOptionDo::Merge
        ));
        assert!(matches!(
            MergePullRequestOptionDo::from(MergeStrategy::Squash),
            MergePullRequestOptionDo::Squash
        ));
        assert!(matches!(
            MergePullRequestOptionDo::from(MergeStrategy::Rebase),
            MergePullRequestOptionDo::Rebase
        ));
    }

    #[test]
    fn deserialize_all_action_types() {
        let yaml = r##"
name: all-actions
enabled: true
matches:
  type: forgejo
actions:
  - type: approve
    comment: "lgtm"
  - type: merge
    strategy: rebase
  - type: comment
    body: "hello"
  - type: add_labels_by_name
    labels: [bug, feature]
  - type: remove_labels_by_name
    labels: [wip]
  - type: ensure_labels_exist
    labels:
      - name: automated
        color: "#e4e669"
  - type: create_issue
    target:
      owner: test
      repo: repo
    title: "test issue"
    body: "test body"
  - type: close_issue
    target:
      owner: test
      repo: repo
    title: "test issue"
"##;
        let rule: RuleDef = yaml_serde::from_str(yaml).unwrap();
        match rule.actions {
            ActionsDef::Flat(actions) => assert_eq!(actions.len(), 8),
            _ => panic!("expected flat actions"),
        }
    }

    #[test]
    fn to_action_approve_with_comment() {
        let def = ActionDef::Approve {
            comment: Some(TemplateString::Plain("looks good".to_string())),
        };
        let action = def.to_action();
        assert_eq!(action.kind(), "approve");
    }

    #[test]
    fn to_action_approve_without_comment() {
        let def = ActionDef::Approve { comment: None };
        let action = def.to_action();
        assert_eq!(action.kind(), "approve");
    }

    #[test]
    fn to_action_merge() {
        let def = ActionDef::Merge {
            strategy: MergeStrategy::Squash,
            delete_branch: true,
        };
        let action = def.to_action();
        assert_eq!(action.kind(), "merge");
    }

    #[test]
    fn merge_delete_branch_defaults_true() {
        let yaml = r#"
type: merge
strategy: squash
"#;
        let def: ActionDef = yaml_serde::from_str(yaml).unwrap();
        match def {
            ActionDef::Merge { delete_branch, .. } => assert!(delete_branch),
            _ => panic!("expected merge"),
        }
    }

    #[test]
    fn deserialize_combinator_matchers() {
        let yaml = r#"
name: test
enabled: true
matches:
  all:
    - type: forgejo
    - type: pr_event
    - not:
        type: has_label
        value: wip
actions: []
"#;
        let rule: RuleDef = yaml_serde::from_str(yaml).unwrap();
        assert!(matches!(rule.matches, Matcher::Combinator(_)));
    }

    #[test]
    fn deserialize_variable_with_leaf_matcher() {
        let yaml = r#"
var: is_open
type: is_open
"#;
        let def: VariableDef = yaml_serde::from_str(yaml).unwrap();
        assert_eq!(def.var, "is_open");
    }

    #[test]
    fn deserialize_variable_with_has_label() {
        let yaml = r#"
var: is_queued
type: has_label
value: janitor/queued
"#;
        let def: VariableDef = yaml_serde::from_str(yaml).unwrap();
        assert_eq!(def.var, "is_queued");
    }

    #[test]
    fn deserialize_ensure_labels_with_target() {
        let yaml = r##"
type: ensure_labels_exist
target:
  owner: myorg
  repo: myrepo
labels:
  - name: bug
    color: "#d73a4a"
"##;
        let def: ActionDef = yaml_serde::from_str(yaml).unwrap();
        match def {
            ActionDef::EnsureLabelsExist { target, labels } => {
                assert!(target.is_some());
                assert_eq!(target.unwrap().owner, "myorg");
                assert_eq!(labels.len(), 1);
            }
            _ => panic!("expected ensure_labels_exist"),
        }
    }

    #[test]
    fn deserialize_create_issue_with_dedup() {
        let yaml = r#"
type: create_issue
target:
  owner: test
  repo: repo
deduplicateByTitle: true
title: "test"
body: "body"
"#;
        let def: ActionDef = yaml_serde::from_str(yaml).unwrap();
        match def {
            ActionDef::CreateIssue {
                deduplicate_by_title,
                ..
            } => assert!(deduplicate_by_title),
            _ => panic!("expected create_issue"),
        }
    }

    #[test]
    fn deserialize_rules_file() {
        let yaml = r#"
rules:
  - name: rule1
    enabled: true
    matches:
      type: forgejo
    actions:
      - type: approve
  - name: rule2
    enabled: false
    matches:
      type: github
    actions: []
"#;
        let file: RulesFile = yaml_serde::from_str(yaml).unwrap();
        assert_eq!(file.rules.len(), 2);
        assert!(file.rules[0].enabled.is_active());
        assert!(!file.rules[1].enabled.is_active());
    }
}
