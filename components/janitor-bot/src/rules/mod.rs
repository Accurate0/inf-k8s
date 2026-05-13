pub mod actions;
pub mod matchers;
pub mod schema;
pub mod validate;

use crate::event::{BotEvent, PrEvent, WorkflowEvent};
use crate::forgejo::ForgejoClient;
use crate::github::GitHubClient;
pub use actions::Action;
use moka::sync::Cache;
use schema::RulesFile;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

pub struct RulesOrchestrator {
    rules: RulesFile,
    pr_locks: Cache<(String, String, u64), Arc<Mutex<()>>>,
    workflow_lock: Mutex<()>,
}

impl RulesOrchestrator {
    pub fn from_rules(rules: RulesFile) -> Self {
        Self {
            rules,
            pr_locks: Cache::builder()
                .time_to_idle(Duration::from_secs(10 * 60))
                .build(),
            workflow_lock: Mutex::new(()),
        }
    }

    fn pr_lock(&self, owner: &str, repo: &str, pr: u64) -> Arc<Mutex<()>> {
        self.pr_locks
            .get_with((owner.to_owned(), repo.to_owned(), pr), || {
                Arc::new(Mutex::new(()))
            })
    }

    pub async fn explain_pr(
        &self,
        client: &ForgejoClient,
        event: &mut PrEvent,
    ) -> Vec<(String, bool, Vec<&'static str>)> {
        let lock = self.pr_lock(&event.owner, &event.repo, event.pr_number);
        let _guard = lock.lock().await;

        let pr_id = event.pr_number as i64;
        if let Ok(files) = client
            .get_pr_changed_files(&event.owner, &event.repo, pr_id)
            .await
        {
            event.changed_files = files;
        }

        let bot_event = BotEvent::ForgejoPr(event);
        let mut out = Vec::new();
        for rule in &self.rules.rules {
            if !rule.enabled.is_active() {
                continue;
            }
            if rule.matches.matches(&bot_event, client).await {
                let actions = rule.actions.iter().map(|a| a.to_action().kind()).collect();
                out.push((rule.name.clone(), rule.enabled.is_dry_run(), actions));
            }
        }
        out
    }

    pub async fn evaluate_pr(&self, client: &ForgejoClient, event: &mut PrEvent) {
        let lock = self.pr_lock(&event.owner, &event.repo, event.pr_number);
        let _guard = lock.lock().await;

        if event
            .labels
            .iter()
            .any(|l| l.name == crate::command::IGNORE_LABEL)
        {
            tracing::info!(
                pr = event.pr_number,
                "skipping rules: {} label present",
                crate::command::IGNORE_LABEL
            );
            return;
        }

        let pr_id = event.pr_number as i64;
        match client
            .get_pr_changed_files(&event.owner, &event.repo, pr_id)
            .await
        {
            Ok(files) => event.changed_files = files,
            Err(e) => tracing::warn!(pr = event.pr_number, "failed to fetch changed files: {e}"),
        }

        let bot_event = BotEvent::ForgejoPr(event);
        self.run_rules(client, &bot_event).await;
    }

    pub async fn evaluate_workflow(
        &self,
        client: &ForgejoClient,
        github_client: &GitHubClient,
        event: &mut WorkflowEvent,
    ) {
        let _guard = self.workflow_lock.lock().await;

        if event.conclusion == "failure" {
            let result = github_client.fetch_failed_jobs(&event.jobs_url).await;
            event.failed_jobs_logs = result.logs;
        }

        let bot_event = BotEvent::GitHubWorkflow(event);
        self.run_rules(client, &bot_event).await;
    }

    async fn run_rules<'a>(&self, client: &ForgejoClient, event: &BotEvent<'a>) {
        for rule in &self.rules.rules {
            if !rule.enabled.is_active() {
                continue;
            }
            let match_start = std::time::Instant::now();
            let matched = rule.matches.matches(event, client).await;
            let match_ms = match_start.elapsed().as_millis();
            if !matched {
                tracing::debug!(rule = rule.name, match_ms, "rule did not match");
                continue;
            }
            let dry_run = rule.enabled.is_dry_run();
            tracing::info!(rule = rule.name, dry_run, match_ms, "rule matched");
            let rule_start = std::time::Instant::now();
            for action_def in &rule.actions {
                let action = action_def.to_action();
                if dry_run {
                    tracing::info!(
                        rule = rule.name,
                        action = action.kind(),
                        "[dry-run] would execute action"
                    );
                    continue;
                }
                let action_start = std::time::Instant::now();
                action.execute(client, event).await;
                tracing::info!(
                    rule = rule.name,
                    action = action.kind(),
                    elapsed_ms = action_start.elapsed().as_millis(),
                    "action executed"
                );
            }
            tracing::info!(
                rule = rule.name,
                elapsed_ms = rule_start.elapsed().as_millis(),
                "rule actions complete"
            );
        }
    }
}
