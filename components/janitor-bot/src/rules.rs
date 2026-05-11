use crate::event::{self, BotEvent, PrEvent, WorkflowEvent};
use crate::forgejo::ForgejoClient;
use crate::github::GitHubClient;
use crate::schema::RulesFile;
use forgejo_api::structs::MergePullRequestOptionDo;
use tokio::sync::Mutex;

const RULES_YAML: &str = include_str!("../rules.yaml");

pub struct RulesOrchestrator {
    rules: RulesFile,
    pr_lock: Mutex<()>,
    workflow_lock: Mutex<()>,
}

impl RulesOrchestrator {
    pub fn new() -> Self {
        let rules = serde_yaml::from_str(RULES_YAML).expect("failed to parse rules.yaml");
        Self {
            rules,
            pr_lock: Mutex::new(()),
            workflow_lock: Mutex::new(()),
        }
    }

    pub async fn evaluate_pr(&self, client: &ForgejoClient, event: &mut PrEvent) {
        let _guard = self.pr_lock.lock().await;

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
            if rule.matches.matches(event, client).await {
                tracing::info!(rule = rule.name, "rule matched");
                for action_def in &rule.actions {
                    let action = action_def.to_action();
                    action.execute(client, event).await;
                }
            }
        }
    }
}

#[allow(dead_code)]
pub enum Action {
    Approve {
        body: String,
    },
    Merge {
        strategy: MergePullRequestOptionDo,
        delete_branch: bool,
    },
    Comment {
        body: String,
    },
    AddLabels {
        label_ids: Vec<i64>,
    },
    AddLabelsByName {
        labels: Vec<String>,
    },
    EnsureLabelsExist {
        labels: Vec<(String, String)>,
        target_owner: Option<String>,
        target_repo: Option<String>,
    },
    CreateIssue {
        target_owner: String,
        target_repo: String,
        dedup_key: String,
        title: String,
        body: String,
        comment_body: Option<String>,
        labels: Vec<(String, String)>,
    },
    CloseIssue {
        target_owner: String,
        target_repo: String,
        dedup_key: String,
        comment_body: Option<String>,
    },
}

impl Action {
    pub async fn execute(&self, client: &ForgejoClient, event: &BotEvent<'_>) {
        let result = match self {
            Action::Approve { body } => {
                let BotEvent::ForgejoPr(pr) = event else {
                    return;
                };
                client
                    .approve_pr(&pr.owner, &pr.repo, pr.pr_number as i64, body)
                    .await
            }
            Action::Merge {
                strategy,
                delete_branch,
            } => {
                let BotEvent::ForgejoPr(pr) = event else {
                    return;
                };
                client
                    .merge_pr(
                        &pr.owner,
                        &pr.repo,
                        pr.pr_number as i64,
                        *strategy,
                        *delete_branch,
                    )
                    .await
            }
            Action::Comment { body } => {
                let BotEvent::ForgejoPr(pr) = event else {
                    return;
                };
                client
                    .comment(&pr.owner, &pr.repo, pr.pr_number as i64, body)
                    .await
            }
            Action::AddLabels { label_ids } => {
                let BotEvent::ForgejoPr(pr) = event else {
                    return;
                };
                client
                    .add_labels(&pr.owner, &pr.repo, pr.pr_number as i64, label_ids.clone())
                    .await
            }
            Action::AddLabelsByName { labels } => {
                let BotEvent::ForgejoPr(pr) = event else {
                    return;
                };
                client
                    .add_labels_by_name(&pr.owner, &pr.repo, pr.pr_number as i64, labels.clone())
                    .await
            }
            Action::EnsureLabelsExist {
                labels,
                target_owner,
                target_repo,
            } => {
                let (owner, repo) = match (target_owner, target_repo) {
                    (Some(o), Some(r)) => (o.as_str(), r.as_str()),
                    _ => match event {
                        BotEvent::ForgejoPr(pr) => (pr.owner.as_str(), pr.repo.as_str()),
                        _ => return,
                    },
                };
                client.ensure_labels(owner, repo, labels.clone()).await
            }
            Action::CreateIssue {
                target_owner,
                target_repo,
                dedup_key,
                title,
                body,
                comment_body,
                labels,
            } => {
                let vars = event.template_vars();
                let rendered_key = event::render_template(dedup_key, &vars);
                let rendered_title = event::render_template(title, &vars);
                let rendered_body = event::render_template(body, &vars);
                async {
                    if let Some(existing) = client
                        .find_open_issue_by_title(target_owner, target_repo, &rendered_key)
                        .await?
                    {
                        let index = existing.number.unwrap();
                        let text = comment_body
                            .as_deref()
                            .map(|t| event::render_template(t, &vars))
                            .unwrap_or(rendered_body);
                        client
                            .comment_on_issue(target_owner, target_repo, index, &text)
                            .await?;
                    } else {
                        let label_names: Vec<String> =
                            labels.iter().map(|(name, _)| name.clone()).collect();
                        client
                            .create_issue_with_labels(
                                target_owner,
                                target_repo,
                                &rendered_title,
                                &rendered_body,
                                &label_names,
                            )
                            .await?;
                    }
                    Ok(())
                }
                .await
            }
            Action::CloseIssue {
                target_owner,
                target_repo,
                dedup_key,
                comment_body,
            } => {
                let vars = event.template_vars();
                let rendered_key = event::render_template(dedup_key, &vars);
                async {
                    let Some(existing) = client
                        .find_open_issue_by_title(target_owner, target_repo, &rendered_key)
                        .await?
                    else {
                        return Ok(());
                    };
                    let index = existing.number.unwrap();
                    if let Some(tmpl) = comment_body {
                        let text = event::render_template(tmpl, &vars);
                        client
                            .comment_on_issue(target_owner, target_repo, index, &text)
                            .await?;
                    }
                    client.close_issue(target_owner, target_repo, index).await?;
                    Ok(())
                }
                .await
            }
        };
        if let Err(e) = result {
            tracing::error!("action failed: {e}");
        }
    }
}
