mod types;

use forgejo_api::structs::CommitStatusState;
use open_feature::EvaluationContext;
pub use types::*;

use crate::clients::Clients;
use crate::event::BotEvent;
use crate::rules::{expr, schema};
use chrono::{Datelike, Timelike};
use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub fn parse_pr_metadata(body: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"<!-- metadata:(\{.*?\}) -->").unwrap()
    });
    let caps = RE.captures(body)?;
    serde_json::from_str(caps.get(1)?.as_str()).ok()
}

pub struct MatcherCache {
    results: moka::sync::Cache<LeafMatcher, bool>,
    values: moka::sync::Cache<String, Arc<dyn Any + Send + Sync>>,
}

impl Default for MatcherCache {
    fn default() -> Self {
        Self::new()
    }
}

impl MatcherCache {
    pub fn new() -> Self {
        Self {
            results: moka::sync::Cache::builder().build(),
            values: moka::sync::Cache::builder().build(),
        }
    }

    pub async fn get_or_compute<T, F, Fut>(&self, key: &str, compute: F) -> T
    where
        T: Clone + Send + Sync + 'static,
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        if let Some(v) = self.values.get(key)
            && let Some(t) = v.downcast_ref::<T>()
        {
            return t.clone();
        }
        let computed = compute().await;
        self.values
            .insert(key.to_owned(), Arc::new(computed.clone()));
        computed
    }
}

impl Matcher {
    pub fn matches<'a>(
        &'a self,
        ev: &'a BotEvent<'a>,
        rule: &'a schema::RuleDef,
        clients: &'a Clients,
        cache: &'a MatcherCache,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async move {
            match self {
                Matcher::Combinator(c) => match c {
                    Combinator::All(matchers) => {
                        for m in matchers {
                            if !m.matches(ev, rule, clients, cache, now).await {
                                return false;
                            }
                        }

                        true
                    }
                    Combinator::Any(matchers) => {
                        for m in matchers {
                            if m.matches(ev, rule, clients, cache, now).await {
                                return true;
                            }
                        }

                        false
                    }
                    Combinator::Not(matcher) => {
                        !matcher.matches(ev, rule, clients, cache, now).await
                    }
                },
                Matcher::LeafExpr(leaf_expr) => {
                    let value =
                        eval_leaf_value(&leaf_expr.matcher, ev, rule, clients, cache, now).await;
                    let mut vars = std::collections::HashMap::new();
                    vars.insert("value".to_string(), value);

                    match expr::parse(&leaf_expr.expr) {
                        Ok(parsed) => match expr::eval(&parsed, &vars) {
                            Ok(v) => v.as_bool().unwrap_or(false),
                            Err(e) => {
                                tracing::error!(expr = leaf_expr.expr, "expr eval error: {e}");
                                false
                            }
                        },
                        Err(e) => {
                            tracing::error!(expr = leaf_expr.expr, "expr parse error: {e}");
                            false
                        }
                    }
                }
                Matcher::Leaf(leaf) => {
                    if let Some(cached) = cache.results.get(leaf) {
                        return cached;
                    }

                    let result = eval_leaf(leaf, ev, rule, clients, cache, now).await;
                    cache.results.insert(leaf.clone(), result);

                    result
                }
            }
        })
    }

    pub fn eval_value<'a>(
        &'a self,
        ev: &'a BotEvent<'a>,
        rule: &'a schema::RuleDef,
        clients: &'a Clients,
        cache: &'a MatcherCache,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Pin<Box<dyn Future<Output = expr::Value> + Send + 'a>> {
        Box::pin(async move {
            match self {
                Matcher::Leaf(leaf) | Matcher::LeafExpr(LeafExprMatcher { matcher: leaf, .. }) => {
                    eval_leaf_value(leaf, ev, rule, clients, cache, now).await
                }
                _ => expr::Value::Bool(self.matches(ev, rule, clients, cache, now).await),
            }
        })
    }
}

fn eval_leaf_value<'a>(
    leaf: &'a LeafMatcher,
    ev: &'a BotEvent<'a>,
    rule: &'a schema::RuleDef,
    clients: &'a Clients,
    cache: &'a MatcherCache,
    now: chrono::DateTime<chrono::Utc>,
) -> Pin<Box<dyn Future<Output = expr::Value> + Send + 'a>> {
    Box::pin(async move {
        match leaf {
            LeafMatcher::WorkflowRunAttempt => match ev {
                BotEvent::GitHubWorkflow(wf) => expr::Value::I64(wf.run_attempt as i64),
                _ => expr::Value::I64(0),
            },
            other => expr::Value::Bool(eval_leaf(other, ev, rule, clients, cache, now).await),
        }
    })
}

async fn combined_status_cached(
    clients: &Clients,
    cache: &MatcherCache,
    pr: &crate::event::PrEvent,
) -> Option<crate::forgejo::PrCombinedStatus> {
    let key = format!("combined_status:{}/{}:{}", pr.owner, pr.repo, pr.pr_number);

    cache
        .get_or_compute(&key, || {
            clients
                .forgejo
                .get_pr_combined_status(&pr.owner, &pr.repo, pr.pr_number as i64)
        })
        .await
}

fn eval_leaf<'a>(
    leaf: &'a LeafMatcher,
    ev: &'a BotEvent<'a>,
    rule: &'a schema::RuleDef,
    clients: &'a Clients,
    cache: &'a MatcherCache,
    now: chrono::DateTime<chrono::Utc>,
) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
    Box::pin(async move {
        match leaf {
            LeafMatcher::Forgejo => matches!(ev, BotEvent::ForgejoPr(_)),
            LeafMatcher::GitHub => {
                matches!(
                    ev,
                    BotEvent::GitHubWorkflow(_)
                        | BotEvent::GitHubCommitStatus(_)
                        | BotEvent::GitHubCheckRun(_)
                )
            }
            LeafMatcher::PrEvent => matches!(ev, BotEvent::ForgejoPr(_)),
            LeafMatcher::WorkflowEvent => matches!(ev, BotEvent::GitHubWorkflow(_)),
            LeafMatcher::CommitStatusEvent => matches!(ev, BotEvent::GitHubCommitStatus(_)),
            LeafMatcher::CheckRunEvent => matches!(ev, BotEvent::GitHubCheckRun(_)),
            LeafMatcher::Argocd => matches!(ev, BotEvent::ArgoSync(_)),
            LeafMatcher::SyncEvent => matches!(ev, BotEvent::ArgoSync(_)),
            LeafMatcher::AppChangedInCommit { owner, repo } => match ev {
                BotEvent::ArgoSync(sync) => {
                    clients
                        .argocd
                        .check_app_changed_in_commit(&clients.forgejo, owner, repo, sync)
                        .await
                }
                _ => false,
            },

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
                BotEvent::GitHubWorkflow(wf) => wf.display_title.contains(value.as_str()),
                BotEvent::GitHubCommitStatus(cs) => cs.context.contains(value.as_str()),
                BotEvent::GitHubCheckRun(cr) => {
                    format!("{} {}", cr.workflow_name, cr.name).contains(value.as_str())
                }
                BotEvent::ArgoSync(sync) => sync.app_name.contains(value.as_str()),
            },
            LeafMatcher::HasLabel { value } => match ev {
                BotEvent::ForgejoPr(pr) => pr.labels.iter().any(|l| l.name == *value),
                _ => false,
            },
            LeafMatcher::HasConflicts => match ev {
                BotEvent::ForgejoPr(pr) => {
                    clients
                        .forgejo
                        .is_pr_mergeable(&pr.owner, &pr.repo, pr.pr_number as i64)
                        .await
                        == Some(false)
                }
                _ => false,
            },
            LeafMatcher::IsOpen => match ev {
                BotEvent::ForgejoPr(pr) => {
                    clients
                        .forgejo
                        .is_pr_open(&pr.owner, &pr.repo, pr.pr_number as i64)
                        .await
                }
                _ => false,
            },
            LeafMatcher::IsMerged => match ev {
                BotEvent::ForgejoPr(pr) => pr.merged,
                _ => false,
            },
            LeafMatcher::NotApprovedBySelf => match ev {
                BotEvent::ForgejoPr(pr) => {
                    !clients
                        .forgejo
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
            LeafMatcher::ChangedFilesAnyMatch { patterns } => match ev {
                BotEvent::ForgejoPr(pr) => pr
                    .changed_files
                    .iter()
                    .any(|f| patterns.iter().any(|p| p.matches(f))),
                _ => false,
            },
            LeafMatcher::ChangedFilesNoneMatch { patterns } => match ev {
                BotEvent::ForgejoPr(pr) => pr
                    .changed_files
                    .iter()
                    .all(|f| !patterns.iter().any(|p| p.matches(f))),
                _ => false,
            },
            LeafMatcher::FeatureFlag { name, default } => {
                let evaluation_context =
                    EvaluationContext::default().with_custom_field("rule_name", rule.name.clone());
                clients
                    .feature_flag
                    .is_feature_enabled(name, *default, evaluation_context)
                    .await
            }
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
                let now = now.with_timezone(&tz);
                let hour = now.hour();

                if *weekdays_only {
                    let weekday = now.weekday();
                    if matches!(weekday, chrono::Weekday::Sat | chrono::Weekday::Sun) {
                        return false;
                    }
                }

                hour >= *start && hour < *end
            }
            LeafMatcher::WorkflowConclusion { value } => match ev {
                BotEvent::GitHubWorkflow(wf) => wf.conclusion == *value,
                _ => false,
            },
            LeafMatcher::TargetBranch { value } => match ev {
                BotEvent::ForgejoPr(pr) => pr.target_branch == *value,
                BotEvent::GitHubWorkflow(wf) => wf.branch == *value,
                BotEvent::GitHubCommitStatus(_) => false,
                BotEvent::GitHubCheckRun(_) => false,
                BotEvent::ArgoSync(_) => false,
            },
            LeafMatcher::Repository { value } => match ev {
                BotEvent::ForgejoPr(pr) => format!("{}/{}", pr.owner, pr.repo) == *value,
                BotEvent::GitHubWorkflow(wf) => wf.repository == *value,
                BotEvent::GitHubCommitStatus(cs) => cs.repository == *value,
                BotEvent::GitHubCheckRun(cr) => cr.repository == *value,
                BotEvent::ArgoSync(_) => false,
            },
            LeafMatcher::WorkflowRunAttempt => matches!(ev, BotEvent::GitHubWorkflow(_)),
            LeafMatcher::HasStatusChecks => match ev {
                BotEvent::ForgejoPr(pr) => combined_status_cached(clients, cache, pr)
                    .await
                    .is_some_and(|s| s.total_count > 0),
                _ => false,
            },
            LeafMatcher::AllStatusChecksPassed => match ev {
                BotEvent::ForgejoPr(pr) => combined_status_cached(clients, cache, pr)
                    .await
                    .is_some_and(|s| matches!(s.state, CommitStatusState::Success)),
                _ => false,
            },
            LeafMatcher::IsLatestByMetadata {
                match_metadata_fields,
            } => match ev {
                BotEvent::ForgejoPr(pr) => {
                    is_latest_by_metadata(clients, cache, pr, match_metadata_fields).await
                }
                _ => false,
            },
        }
    })
}

async fn is_latest_by_metadata(
    clients: &Clients,
    cache: &MatcherCache,
    pr: &crate::event::PrEvent,
    fields: &[String],
) -> bool {
    let client = &clients.forgejo;

    // Fetch current PR details
    let current = match client.get_pr(&pr.owner, &pr.repo, pr.pr_number as i64).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(pr = pr.pr_number, "failed to fetch PR for metadata check: {e}");
            return true; // default to latest if we can't check
        }
    };

    let current_body = current.body.as_deref().unwrap_or("");
    let Some(current_meta) = parse_pr_metadata(current_body) else {
        tracing::debug!(pr = pr.pr_number, "no metadata in current PR");
        return true; // no metadata = assume latest
    };

    let current_field_values: Vec<_> = fields
        .iter()
        .filter_map(|f| current_meta.get(f).and_then(|v| v.as_str()).map(|s| (f.as_str(), s.to_owned())))
        .collect();

    if current_field_values.len() != fields.len() {
        tracing::debug!(pr = pr.pr_number, "missing metadata fields");
        return true;
    }

    let current_created = current.created_at;

    // List all open PRs (cached)
    let key = format!("open_prs:{}/{}", pr.owner, pr.repo);
    let open_prs: Option<Vec<forgejo_api::structs::PullRequest>> = cache
        .get_or_compute(&key, || async {
            match client.list_open_prs(&pr.owner, &pr.repo).await {
                Ok(prs) => Some(prs),
                Err(e) => {
                    tracing::warn!("failed to list open PRs: {e}");
                    None
                }
            }
        })
        .await;

    let Some(open_prs) = open_prs else {
        return true;
    };

    for other in &open_prs {
        let other_number = other.number.unwrap_or(0) as u64;
        if other_number == pr.pr_number {
            continue;
        }

        let other_author = other
            .user
            .as_ref()
            .and_then(|u| u.login.as_deref())
            .unwrap_or("");
        if other_author != pr.author {
            continue;
        }

        let other_body = other.body.as_deref().unwrap_or("");
        let Some(other_meta) = parse_pr_metadata(other_body) else {
            continue;
        };

        let all_fields_match = current_field_values.iter().all(|(field, value)| {
            other_meta
                .get(*field)
                .and_then(|v| v.as_str())
                .is_some_and(|s| s == value)
        });

        if !all_fields_match {
            continue;
        }

        // If any other PR with matching metadata is newer, current is not the latest
        if other.created_at > current_created {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_pattern_starts_with() {
        let p = FilePattern::StartsWith {
            starts_with: ".github/workflows/".to_string(),
        };
        assert!(p.matches(".github/workflows/build.yaml"));
        assert!(p.matches(".github/workflows/test.yml"));
        assert!(!p.matches("src/main.rs"));
        assert!(!p.matches(".github/dependabot.yml"));
    }

    #[test]
    fn file_pattern_contains() {
        let p = FilePattern::Contains {
            contains: "Dockerfile".to_string(),
        };
        assert!(p.matches("Dockerfile"));
        assert!(p.matches("apps/Dockerfile"));
        assert!(p.matches("Dockerfile.dev"));
        assert!(!p.matches("docker-compose.yml"));
    }

    #[test]
    fn file_pattern_ends_with() {
        let p = FilePattern::EndsWith {
            ends_with: ".yaml".to_string(),
        };
        assert!(p.matches("config.yaml"));
        assert!(p.matches("path/to/file.yaml"));
        assert!(!p.matches("config.yml"));
        assert!(!p.matches("config.yaml.bak"));
    }

    #[test]
    fn file_pattern_empty_path() {
        let p = FilePattern::Contains {
            contains: "foo".to_string(),
        };
        assert!(!p.matches(""));
    }

    #[test]
    fn file_pattern_empty_pattern() {
        let p = FilePattern::StartsWith {
            starts_with: "".to_string(),
        };
        assert!(p.matches("anything"));
        assert!(p.matches(""));
    }

    #[test]
    fn deserialize_leaf_forgejo() {
        let yaml = "type: forgejo";
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        assert!(matches!(m, Matcher::Leaf(LeafMatcher::Forgejo)));
    }

    #[test]
    fn deserialize_leaf_github() {
        let yaml = "type: github";
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        assert!(matches!(m, Matcher::Leaf(LeafMatcher::GitHub)));
    }

    #[test]
    fn deserialize_leaf_action() {
        let yaml = "type: action\nvalue: opened";
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Leaf(LeafMatcher::Action { value }) => assert_eq!(value, "opened"),
            _ => panic!("expected Action"),
        }
    }

    #[test]
    fn deserialize_leaf_author() {
        let yaml = "type: author\nvalue: renovate";
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Leaf(LeafMatcher::Author { value }) => assert_eq!(value, "renovate"),
            _ => panic!("expected Author"),
        }
    }

    #[test]
    fn deserialize_leaf_has_label() {
        let yaml = "type: has_label\nvalue: wip";
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Leaf(LeafMatcher::HasLabel { value }) => assert_eq!(value, "wip"),
            _ => panic!("expected HasLabel"),
        }
    }

    #[test]
    fn deserialize_leaf_time_window() {
        let yaml = "type: time_window\ntimezone: Australia/Perth\nstart: 17\nend: 22";
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Leaf(LeafMatcher::TimeWindow {
                timezone,
                start,
                end,
                weekdays_only,
            }) => {
                assert_eq!(timezone, "Australia/Perth");
                assert_eq!(start, 17);
                assert_eq!(end, 22);
                assert!(!weekdays_only);
            }
            _ => panic!("expected TimeWindow"),
        }
    }

    #[test]
    fn deserialize_time_window_weekdays_only() {
        let yaml = "type: time_window\ntimezone: UTC\nstart: 9\nend: 17\nweekdays_only: true";
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Leaf(LeafMatcher::TimeWindow { weekdays_only, .. }) => {
                assert!(weekdays_only);
            }
            _ => panic!("expected TimeWindow"),
        }
    }

    #[test]
    fn deserialize_combinator_all() {
        let yaml = r#"
all:
  - type: forgejo
  - type: pr_event
"#;
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Combinator(Combinator::All(matchers)) => assert_eq!(matchers.len(), 2),
            _ => panic!("expected All combinator"),
        }
    }

    #[test]
    fn deserialize_combinator_any() {
        let yaml = r#"
any:
  - type: forgejo
  - type: github
"#;
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Combinator(Combinator::Any(matchers)) => assert_eq!(matchers.len(), 2),
            _ => panic!("expected Any combinator"),
        }
    }

    #[test]
    fn deserialize_combinator_not() {
        let yaml = r#"
not:
  type: has_label
  value: wip
"#;
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Combinator(Combinator::Not(inner)) => {
                assert!(matches!(
                    *inner,
                    Matcher::Leaf(LeafMatcher::HasLabel { .. })
                ));
            }
            _ => panic!("expected Not combinator"),
        }
    }

    #[test]
    fn deserialize_nested_combinators() {
        let yaml = r#"
all:
  - type: forgejo
  - any:
      - type: author
        value: renovate
      - type: author
        value: dependabot
  - not:
      type: has_label
      value: wip
"#;
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Combinator(Combinator::All(matchers)) => {
                assert_eq!(matchers.len(), 3);
                assert!(matches!(
                    matchers[1],
                    Matcher::Combinator(Combinator::Any(_))
                ));
                assert!(matches!(
                    matchers[2],
                    Matcher::Combinator(Combinator::Not(_))
                ));
            }
            _ => panic!("expected All combinator"),
        }
    }

    #[test]
    fn deserialize_changed_files_all_match() {
        let yaml = r#"
type: changed_files_all_match
patterns:
  - starts_with: ".github/"
  - contains: Dockerfile
"#;
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Leaf(LeafMatcher::ChangedFilesAllMatch { patterns }) => {
                assert_eq!(patterns.len(), 2);
            }
            _ => panic!("expected ChangedFilesAllMatch"),
        }
    }

    #[test]
    fn deserialize_changed_files_none_match() {
        let yaml = r#"
type: changed_files_none_match
patterns:
  - contains: "secret"
"#;
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Leaf(LeafMatcher::ChangedFilesNoneMatch { patterns }) => {
                assert_eq!(patterns.len(), 1);
            }
            _ => panic!("expected ChangedFilesNoneMatch"),
        }
    }

    #[test]
    fn deserialize_file_pattern_variants() {
        let yaml_sw: FilePattern = yaml_serde::from_str("starts_with: foo/").unwrap();
        assert!(matches!(yaml_sw, FilePattern::StartsWith { .. }));

        let yaml_c: FilePattern = yaml_serde::from_str("contains: bar").unwrap();
        assert!(matches!(yaml_c, FilePattern::Contains { .. }));

        let yaml_ew: FilePattern = yaml_serde::from_str("ends_with: .rs").unwrap();
        assert!(matches!(yaml_ew, FilePattern::EndsWith { .. }));
    }

    #[test]
    fn file_pattern_glob_basic() {
        let p = FilePattern::Glob("src/**/*.rs".to_string());
        assert!(p.matches("src/main.rs"));
        assert!(p.matches("src/rules/mod.rs"));
        assert!(!p.matches("Cargo.toml"));
        assert!(!p.matches("tests/integration.rs"));
    }

    #[test]
    fn file_pattern_glob_negated() {
        let p = FilePattern::Glob("!src/generated/**".to_string());
        assert!(p.matches("src/main.rs"));
        assert!(!p.matches("src/generated/types.rs"));
        assert!(!p.matches("src/generated/deep/nested.rs"));
    }

    #[test]
    fn file_pattern_glob_dockerfile() {
        let p = FilePattern::Glob("**/Dockerfile*".to_string());
        assert!(p.matches("Dockerfile"));
        assert!(p.matches("apps/Dockerfile"));
        assert!(p.matches("apps/Dockerfile.dev"));
        assert!(!p.matches("docker-compose.yml"));
    }

    #[test]
    fn file_pattern_glob_workflows() {
        let p = FilePattern::Glob(".github/workflows/**".to_string());
        assert!(p.matches(".github/workflows/build.yaml"));
        assert!(p.matches(".github/workflows/test.yml"));
        assert!(!p.matches(".github/dependabot.yml"));
        assert!(!p.matches("src/main.rs"));
    }

    #[test]
    fn deserialize_glob_pattern_from_string() {
        let yaml = r#"
type: changed_files_all_match
patterns:
  - "src/**/*.rs"
  - "!src/generated/**"
"#;
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Leaf(LeafMatcher::ChangedFilesAllMatch { patterns }) => {
                assert_eq!(patterns.len(), 2);
                assert!(matches!(&patterns[0], FilePattern::Glob(s) if s == "src/**/*.rs"));
                assert!(matches!(&patterns[1], FilePattern::Glob(s) if s == "!src/generated/**"));
            }
            _ => panic!("expected ChangedFilesAllMatch"),
        }
    }

    #[test]
    fn glob_and_legacy_patterns_mixed() {
        let yaml = r#"
type: changed_files_all_match
patterns:
  - starts_with: ".github/"
  - "**/*.yaml"
"#;
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Leaf(LeafMatcher::ChangedFilesAllMatch { patterns }) => {
                assert_eq!(patterns.len(), 2);
                assert!(matches!(&patterns[0], FilePattern::StartsWith { .. }));
                assert!(matches!(&patterns[1], FilePattern::Glob(_)));
            }
            _ => panic!("expected ChangedFilesAllMatch"),
        }
    }

    #[test]
    fn deserialize_leaf_argocd() {
        let yaml = "type: argocd";
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        assert!(matches!(m, Matcher::Leaf(LeafMatcher::Argocd)));
    }

    #[test]
    fn deserialize_workflow_conclusion() {
        let yaml = "type: workflow_conclusion\nvalue: failure";
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Leaf(LeafMatcher::WorkflowConclusion { value }) => {
                assert_eq!(value, "failure");
            }
            _ => panic!("expected WorkflowConclusion"),
        }
    }

    #[test]
    fn deserialize_target_branch() {
        let yaml = "type: target_branch\nvalue: main";
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Leaf(LeafMatcher::TargetBranch { value }) => assert_eq!(value, "main"),
            _ => panic!("expected TargetBranch"),
        }
    }

    #[test]
    fn deserialize_title_contains() {
        let yaml = "type: title_contains\nvalue: fix";
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Leaf(LeafMatcher::TitleContains { value }) => assert_eq!(value, "fix"),
            _ => panic!("expected TitleContains"),
        }
    }

    #[test]
    fn deserialize_repository() {
        let yaml = "type: repository\nvalue: anurag/k8s";
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Leaf(LeafMatcher::Repository { value }) => {
                assert_eq!(value, "anurag/k8s");
            }
            _ => panic!("expected Repository"),
        }
    }

    #[test]
    fn deserialize_leaf_expr() {
        let yaml = "type: workflow_run_attempt\nexpr: \"value < 3\"";
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::LeafExpr(le) => {
                assert!(matches!(le.matcher, LeafMatcher::WorkflowRunAttempt));
                assert_eq!(le.expr, "value < 3");
            }
            _ => panic!("expected LeafExpr, got {m:?}"),
        }
    }

    #[test]
    fn deserialize_leaf_without_expr_stays_leaf() {
        let yaml = "type: workflow_run_attempt";
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        assert!(matches!(m, Matcher::Leaf(LeafMatcher::WorkflowRunAttempt)));
    }

    #[test]
    fn deserialize_leaf_expr_with_other_fields() {
        let yaml = "type: workflow_conclusion\nvalue: failure\nexpr: \"value == true\"";
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::LeafExpr(le) => {
                assert!(matches!(le.matcher, LeafMatcher::WorkflowConclusion { .. }));
                assert_eq!(le.expr, "value == true");
            }
            _ => panic!("expected LeafExpr"),
        }
    }
}
