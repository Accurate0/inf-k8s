pub(crate) mod cache;
mod types;

use forgejo_api::structs::{CommitStatusState, StateType};
use open_feature::EvaluationContext;
use regex::Regex;
pub use types::*;

pub use cache::ResourceCache;
use cache::{
    combined_status_cached, get_changed_files_cached, get_pr_cached, get_reviews_cached,
    is_latest_by_metadata,
};

use crate::clients::Clients;
use crate::event::BotEvent;
use crate::rules::{expr, schema};
use chrono::Datelike;
use std::future::Future;
use std::pin::Pin;
use std::sync::LazyLock;
use tracing::Instrument;

fn event_kind_and_source(ev: &BotEvent<'_>) -> (EventKind, EventSource) {
    match ev {
        BotEvent::ForgejoPr(_) => (EventKind::Pr, EventSource::Forgejo),
        BotEvent::GitHubWorkflow(_) => (EventKind::Workflow, EventSource::Github),
        BotEvent::GitHubCommitStatus(_) => (EventKind::CommitStatus, EventSource::Github),
        BotEvent::GitHubCheckRun(_) => (EventKind::CheckRun, EventSource::Github),
        BotEvent::ArgoSync(_) => (EventKind::Sync, EventSource::Argocd),
    }
}

pub fn parse_pr_metadata(body: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"<!-- metadata:(\{.*?\}) -->").unwrap());

    let caps = RE.captures(body)?;
    serde_json::from_str(caps.get(1)?.as_str()).ok()
}

impl Matcher {
    pub fn matches<'a>(
        &'a self,
        ev: &'a BotEvent<'a>,
        rule: &'a schema::RuleDef,
        clients: &'a Clients,
        cache: &'a ResourceCache,
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

                Matcher::Ref(r) => panic!("unresolved matcher ref `{}`", r.name),

                Matcher::Leaf(leaf) => {
                    if let Some(cached) = cache.matcher_results.get(leaf) {
                        tracing::debug!(
                            matcher = leaf.kind(),
                            rule = rule.name,
                            result = cached,
                            cached = true,
                            "leaf matcher (cached)"
                        );
                        return cached;
                    }

                    let span = tracing::debug_span!(
                        "matcher.leaf",
                        otel.name = format!("matcher: {}", leaf.kind()),
                        matcher = leaf.kind(),
                        rule = rule.name,
                    );
                    let result = eval_leaf(leaf, ev, rule, clients, cache, now)
                        .instrument(span)
                        .await;
                    tracing::debug!(
                        matcher = leaf.kind(),
                        rule = rule.name,
                        result,
                        "leaf matcher evaluated"
                    );
                    cache.matcher_results.insert(leaf.clone(), result);
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
        cache: &'a ResourceCache,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Pin<Box<dyn Future<Output = expr::Value> + Send + 'a>> {
        Box::pin(async move {
            match self {
                Matcher::Leaf(leaf) | Matcher::LeafExpr(LeafExprMatcher { matcher: leaf, .. }) => {
                    eval_leaf_value(leaf, ev, rule, clients, cache, now).await
                }
                Matcher::Ref(r) => panic!("unresolved matcher ref `{}`", r.name),
                Matcher::Combinator(_) => {
                    expr::Value::Bool(self.matches(ev, rule, clients, cache, now).await)
                }
            }
        })
    }
}

fn eval_leaf_value<'a>(
    leaf: &'a LeafMatcher,
    ev: &'a BotEvent<'a>,
    rule: &'a schema::RuleDef,
    clients: &'a Clients,
    cache: &'a ResourceCache,
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

fn eval_leaf<'a>(
    leaf: &'a LeafMatcher,
    ev: &'a BotEvent<'a>,
    rule: &'a schema::RuleDef,
    clients: &'a Clients,
    cache: &'a ResourceCache,
    now: chrono::DateTime<chrono::Utc>,
) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
    Box::pin(async move {
        match leaf {
            LeafMatcher::Event { kind, source } => {
                let (ev_kind, ev_source) = event_kind_and_source(ev);
                ev_kind == *kind && source.matches(ev_source)
            }

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
            LeafMatcher::TitleMatches { value, mode } => match ev {
                BotEvent::ForgejoPr(pr) => mode.matches(&pr.title, value),
                BotEvent::GitHubWorkflow(wf) => mode.matches(&wf.display_title, value),
                BotEvent::GitHubCommitStatus(cs) => mode.matches(&cs.context, value),
                BotEvent::GitHubCheckRun(cr) => {
                    mode.matches(&format!("{} {}", cr.workflow_name, cr.name), value)
                }
                BotEvent::ArgoSync(sync) => mode.matches(&sync.app_name, value),
            },
            LeafMatcher::HasLabel { value } => match ev {
                BotEvent::ForgejoPr(pr) => pr.labels.iter().any(|l| l.name == *value),
                _ => false,
            },

            LeafMatcher::HasConflicts => match ev {
                BotEvent::ForgejoPr(pr) => get_pr_cached(clients, cache, pr)
                    .await
                    .and_then(|p| p.mergeable)
                    .is_some_and(|m| !m),
                _ => false,
            },
            LeafMatcher::IsOpen => match ev {
                BotEvent::ForgejoPr(pr) => {
                    get_pr_cached(clients, cache, pr).await.is_some_and(|p| {
                        matches!(p.state, Some(StateType::Open)) && !p.merged.unwrap_or(false)
                    })
                }
                _ => false,
            },
            LeafMatcher::IsMerged => match ev {
                BotEvent::ForgejoPr(pr) => pr.merged,
                _ => false,
            },
            LeafMatcher::BotHasApproved => match ev {
                BotEvent::ForgejoPr(pr) => get_reviews_cached(clients, cache, pr).await,
                _ => false,
            },

            LeafMatcher::HasChangedFiles => match ev {
                BotEvent::ForgejoPr(pr) => !get_changed_files_cached(clients, cache, pr)
                    .await
                    .is_empty(),
                _ => false,
            },
            LeafMatcher::ChangedFilesAllMatch { patterns } => match ev {
                BotEvent::ForgejoPr(pr) => {
                    let files = get_changed_files_cached(clients, cache, pr).await;
                    !files.is_empty() && files.iter().all(|f| patterns.iter().any(|p| p.matches(f)))
                }
                _ => false,
            },
            LeafMatcher::ChangedFilesAnyMatch { patterns } => match ev {
                BotEvent::ForgejoPr(pr) => {
                    let files = get_changed_files_cached(clients, cache, pr).await;
                    files.iter().any(|f| patterns.iter().any(|p| p.matches(f)))
                }
                _ => false,
            },
            LeafMatcher::ChangedFilesNoneMatch { patterns } => match ev {
                BotEvent::ForgejoPr(pr) => {
                    let files = get_changed_files_cached(clients, cache, pr).await;
                    files.iter().all(|f| !patterns.iter().any(|p| p.matches(f)))
                }
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

            LeafMatcher::TimeOfDay {
                tz,
                after,
                before,
                weekdays_only,
            } => {
                let parsed_tz: chrono_tz::Tz = match tz.parse() {
                    Ok(tz) => tz,
                    Err(_) => {
                        tracing::warn!(tz, "invalid timezone, defaulting to false");
                        return false;
                    }
                };

                let parse_hm = |s: &str| chrono::NaiveTime::parse_from_str(s, "%H:%M").ok();
                let (Some(after_t), Some(before_t)) = (parse_hm(after), parse_hm(before)) else {
                    tracing::warn!(after, before, "invalid time_of_day, defaulting to false");
                    return false;
                };

                let now = now.with_timezone(&parsed_tz);
                if *weekdays_only
                    && matches!(now.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun)
                {
                    return false;
                }

                let now_t = now.time();
                now_t >= after_t && now_t < before_t
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
            LeafMatcher::StatusChecks { names, state } => match ev {
                BotEvent::ForgejoPr(pr) => match combined_status_cached(clients, cache, pr).await {
                    Some(s) => {
                        let present: Vec<String> = s
                            .statuses
                            .iter()
                            .map(|st| format!("{}={:?}", st.context, st.state))
                            .collect();

                        let unmet: Vec<&String> = names
                            .iter()
                            .filter(|n| {
                                !s.statuses
                                    .iter()
                                    .any(|st| st.context == **n && state.matches(&st.state))
                            })
                            .collect();

                        tracing::debug!(
                            required = ?names,
                            required_state = ?state,
                            present = ?present,
                            unmet = ?unmet,
                            "status_checks matcher evaluated"
                        );

                        unmet.is_empty()
                    }
                    None => {
                        tracing::debug!("status_checks: no combined status available");
                        false
                    }
                },
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

#[cfg(test)]
mod tests {
    use super::*;

    fn pat(s: &str) -> FilePattern {
        FilePattern(s.to_string())
    }

    #[test]
    fn deserialize_event_single_source() {
        let yaml = "type: event\nkind: pr\nsource: forgejo";
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Leaf(LeafMatcher::Event { kind, source }) => {
                assert_eq!(kind, EventKind::Pr);
                assert!(source.matches(EventSource::Forgejo));
                assert!(!source.matches(EventSource::Github));
            }
            _ => panic!("expected Event"),
        }
    }

    #[test]
    fn deserialize_event_multi_source() {
        let yaml = "type: event\nkind: workflow\nsource: [forgejo, github]";
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Leaf(LeafMatcher::Event { kind, source }) => {
                assert_eq!(kind, EventKind::Workflow);
                assert!(source.matches(EventSource::Forgejo));
                assert!(source.matches(EventSource::Github));
                assert!(!source.matches(EventSource::Argocd));
            }
            _ => panic!("expected Event"),
        }
    }

    #[test]
    fn event_without_source_fails() {
        let yaml = "type: event\nkind: pr";
        let r: Result<Matcher, _> = yaml_serde::from_str(yaml);
        assert!(r.is_err(), "source should be required");
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
    fn deserialize_leaf_time_of_day() {
        let yaml = "type: time_of_day\ntz: Australia/Perth\nafter: \"17:00\"\nbefore: \"22:00\"";
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Leaf(LeafMatcher::TimeOfDay {
                tz,
                after,
                before,
                weekdays_only,
            }) => {
                assert_eq!(tz, "Australia/Perth");
                assert_eq!(after, "17:00");
                assert_eq!(before, "22:00");
                assert!(!weekdays_only);
            }
            _ => panic!("expected TimeOfDay"),
        }
    }

    #[test]
    fn deserialize_time_of_day_weekdays_only() {
        let yaml = "type: time_of_day\ntz: UTC\nafter: \"09:00\"\nbefore: \"17:00\"\nweekdays_only: true";
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Leaf(LeafMatcher::TimeOfDay { weekdays_only, .. }) => {
                assert!(weekdays_only);
            }
            _ => panic!("expected TimeOfDay"),
        }
    }

    #[test]
    fn deserialize_combinator_all() {
        let yaml = r#"
all:
  - type: event
    kind: pr
    source: forgejo
  - type: is_open
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
  - type: event
    kind: pr
    source: forgejo
  - type: event
    kind: workflow
    source: github
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
  - type: event
    kind: pr
    source: forgejo
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
    fn file_pattern_glob_basic() {
        let p = pat("src/**/*.rs");
        assert!(p.matches("src/main.rs"));
        assert!(p.matches("src/rules/mod.rs"));
        assert!(!p.matches("Cargo.toml"));
    }

    #[test]
    fn file_pattern_glob_negated() {
        let p = pat("!src/generated/**");
        assert!(p.matches("src/main.rs"));
        assert!(!p.matches("src/generated/types.rs"));
    }

    #[test]
    fn file_pattern_glob_dockerfile() {
        let p = pat("**/Dockerfile*");
        assert!(p.matches("Dockerfile"));
        assert!(p.matches("apps/Dockerfile"));
        assert!(p.matches("apps/Dockerfile.dev"));
        assert!(!p.matches("docker-compose.yml"));
    }

    #[test]
    fn file_pattern_glob_workflows() {
        let p = pat(".github/workflows/**");
        assert!(p.matches(".github/workflows/build.yaml"));
        assert!(!p.matches(".github/dependabot.yml"));
    }

    #[test]
    fn deserialize_pattern_from_bare_string() {
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
                assert_eq!(patterns[0].0, "src/**/*.rs");
                assert_eq!(patterns[1].0, "!src/generated/**");
            }
            _ => panic!("expected ChangedFilesAllMatch"),
        }
    }

    #[test]
    fn deserialize_event_sync_source_argocd() {
        let yaml = "type: event\nkind: sync\nsource: argocd";
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Leaf(LeafMatcher::Event { kind, source }) => {
                assert_eq!(kind, EventKind::Sync);
                assert!(source.matches(EventSource::Argocd));
            }
            _ => panic!("expected Event"),
        }
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
    fn deserialize_title_matches() {
        let yaml = "type: title_matches\nvalue: fix";
        let m: Matcher = yaml_serde::from_str(yaml).unwrap();
        match m {
            Matcher::Leaf(LeafMatcher::TitleMatches { value, mode }) => {
                assert_eq!(value, "fix");
                assert_eq!(mode, StringMatchMode::Contains);
            }
            _ => panic!("expected TitleMatches"),
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
