use crate::event::PrEvent;
use crate::forgejo::ForgejoClient;
use crate::schema::RulesFile;
use forgejo_api::structs::MergePullRequestOptionDo;
use tokio::sync::Mutex;

const RULES_YAML: &str = include_str!("../rules.yaml");

pub struct RulesOrchestrator {
    rules: RulesFile,
    lock: Mutex<()>,
}

impl RulesOrchestrator {
    pub fn new() -> Self {
        let rules = serde_yaml::from_str(RULES_YAML).expect("failed to parse rules.yaml");
        Self {
            rules,
            lock: Mutex::new(()),
        }
    }

    pub async fn evaluate(&self, client: &ForgejoClient, event: &mut PrEvent) {
        let _guard = self.lock.lock().await;

        let pr_id = event.pr_number as i64;
        match client
            .get_pr_changed_files(&event.owner, &event.repo, pr_id)
            .await
        {
            Ok(files) => event.changed_files = files,
            Err(e) => tracing::warn!(pr = event.pr_number, "failed to fetch changed files: {e}"),
        }

        for rule in &self.rules.rules {
            if rule.matches.matches(event, client).await {
                tracing::info!(rule = rule.name, pr = event.pr_number, "rule matched");
                for action_def in &rule.actions {
                    let action = action_def.to_action();
                    action
                        .execute(client, &event.owner, &event.repo, event.pr_number as i64)
                        .await;
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
}

impl Action {
    pub async fn execute(
        &self,
        client: &ForgejoClient,
        owner: &str,
        repo: &str,
        pr: i64,
    ) {
        let result = match self {
            Action::Approve { body } => client.approve_pr(owner, repo, pr, body).await,
            Action::Merge {
                strategy,
                delete_branch,
            } => {
                client
                    .merge_pr(owner, repo, pr, *strategy, *delete_branch)
                    .await
            }
            Action::Comment { body } => client.comment(owner, repo, pr, body).await,
            Action::AddLabels { label_ids } => {
                client.add_labels(owner, repo, pr, label_ids.clone()).await
            }
            Action::AddLabelsByName { labels } => {
                client
                    .add_labels_by_name(owner, repo, pr, labels.clone())
                    .await
            }
            Action::EnsureLabelsExist { labels } => {
                client.ensure_labels(owner, repo, labels.clone()).await
            }
        };
        if let Err(e) = result {
            tracing::error!(pr, "action failed: {e}");
        }
    }
}

