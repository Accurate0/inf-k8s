use crate::event::{BotEvent, PrEvent, WorkflowEvent};
use crate::forgejo::ForgejoClient;
use crate::schema::RulesFile;
use forgejo_api::structs::MergePullRequestOptionDo;
use tokio::sync::Mutex;

const WORKFLOW_ISSUE_PREFIX: &str = "[GitHub CI] ";

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

    pub async fn evaluate_workflow(&self, client: &ForgejoClient, event: &WorkflowEvent) {
        let _guard = self.workflow_lock.lock().await;

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
    },
    ReportWorkflowFailure {
        target_owner: String,
        target_repo: String,
    },
    ResolveWorkflowFailure {
        target_owner: String,
        target_repo: String,
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
            Action::EnsureLabelsExist { labels } => {
                let BotEvent::ForgejoPr(pr) = event else {
                    return;
                };
                client
                    .ensure_labels(&pr.owner, &pr.repo, labels.clone())
                    .await
            }
            Action::ReportWorkflowFailure {
                target_owner,
                target_repo,
            } => {
                let BotEvent::GitHubWorkflow(wf) = event else {
                    return;
                };
                report_workflow_failure(client, target_owner, target_repo, wf).await
            }
            Action::ResolveWorkflowFailure {
                target_owner,
                target_repo,
            } => {
                let BotEvent::GitHubWorkflow(wf) = event else {
                    return;
                };
                resolve_workflow_failure(client, target_owner, target_repo, wf).await
            }
        };
        if let Err(e) = result {
            tracing::error!("action failed: {e}");
        }
    }
}

async fn report_workflow_failure(
    client: &ForgejoClient,
    owner: &str,
    repo: &str,
    wf: &WorkflowEvent,
) -> Result<(), forgejo_api::ForgejoError> {
    let title = format!("{WORKFLOW_ISSUE_PREFIX}{}", wf.workflow_name);

    if let Some(issue) = client.find_open_issue_by_title(owner, repo, &title).await? {
        let index = issue.number.unwrap();
        let body = format!(
            "Another failure on branch `{}`.\n\n**Run:** {}",
            wf.branch, wf.run_url
        );
        client.comment_on_issue(owner, repo, index, &body).await?;
        tracing::info!(
            owner,
            repo,
            issue = index,
            workflow = wf.workflow_name,
            "commented on existing workflow failure issue"
        );
    } else {
        let body = format!(
            "Workflow **{}** failed on branch `{}`.\n\n**Run:** {}",
            wf.workflow_name, wf.branch, wf.run_url
        );
        client.create_issue(owner, repo, &title, &body).await?;
    }

    Ok(())
}

async fn resolve_workflow_failure(
    client: &ForgejoClient,
    owner: &str,
    repo: &str,
    wf: &WorkflowEvent,
) -> Result<(), forgejo_api::ForgejoError> {
    let title = format!("{WORKFLOW_ISSUE_PREFIX}{}", wf.workflow_name);

    let Some(issue) = client.find_open_issue_by_title(owner, repo, &title).await? else {
        return Ok(());
    };

    let index = issue.number.unwrap();
    let body = format!(
        "Workflow is passing again on branch `{}`.\n\n**Run:** {}",
        wf.branch, wf.run_url
    );
    client.comment_on_issue(owner, repo, index, &body).await?;
    client.close_issue(owner, repo, index).await?;

    Ok(())
}
