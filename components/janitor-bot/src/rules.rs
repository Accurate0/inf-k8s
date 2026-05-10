use crate::event::PrEvent;
use crate::forgejo::ForgejoClient;
use forgejo_api::structs::MergePullRequestOptionDo;

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
    PreflightCheck,
}

pub enum ActionResult {
    Continue,
    StopProcessing,
}

impl Action {
    pub async fn execute(
        &self,
        client: &ForgejoClient,
        owner: &str,
        repo: &str,
        pr: i64,
    ) -> ActionResult {
        if let Action::PreflightCheck = self {
            if client.is_pr_merged(owner, repo, pr).await {
                tracing::info!(pr, "PR already merged, skipping remaining actions");
                return ActionResult::StopProcessing;
            }
            return ActionResult::Continue;
        }

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
            Action::PreflightCheck => unreachable!(),
        };
        if let Err(e) = result {
            tracing::error!(pr, "action failed: {e}");
        }
        ActionResult::Continue
    }
}

pub struct Rule {
    pub name: &'static str,
    pub matches: fn(&PrEvent) -> bool,
    pub actions: fn() -> Vec<Action>,
}

pub async fn evaluate(rules: &[Rule], client: &ForgejoClient, event: &PrEvent) {
    for rule in rules {
        if (rule.matches)(event) {
            tracing::info!(rule = rule.name, pr = event.pr_number, "rule matched");
            for action in (rule.actions)() {
                if matches!(
                    action
                        .execute(client, &event.owner, &event.repo, event.pr_number as i64)
                        .await,
                    ActionResult::StopProcessing
                ) {
                    break;
                }
            }
        }
    }
}

pub fn all_rules() -> Vec<Rule> {
    vec![
        label_renovate(),
        auto_merge_image_updater(),
        auto_merge_renovate(),
    ]
}

fn is_workflow_or_dockerfile(path: &str) -> bool {
    path.starts_with(".github/workflows/") || path.contains("Dockerfile")
}

fn label_renovate() -> Rule {
    Rule {
        name: "label-renovate",
        matches: |ev| ev.action == "opened" && ev.author == "renovate",
        actions: || {
            vec![
                Action::PreflightCheck,
                Action::EnsureLabelsExist {
                    labels: vec![("renovate".into(), "#1a7f37".into())],
                },
                Action::AddLabelsByName {
                    labels: vec!["renovate".into()],
                },
            ]
        },
    }
}

fn auto_merge_renovate() -> Rule {
    Rule {
        name: "auto-merge-renovate",
        matches: |ev| {
            ev.action == "opened"
                && ev.author == "renovate"
                && !ev.changed_files.is_empty()
                && ev
                    .changed_files
                    .iter()
                    .all(|f| is_workflow_or_dockerfile(f))
        },
        actions: || {
            vec![
                Action::PreflightCheck,
                Action::EnsureLabelsExist {
                    labels: vec![("automated".into(), "#e4e669".into())],
                },
                Action::AddLabelsByName {
                    labels: vec!["automated".into()],
                },
                Action::Approve {
                    body: "Auto-approved: Renovate PR targeting workflows/Dockerfiles".into(),
                },
                Action::Merge {
                    strategy: MergePullRequestOptionDo::Squash,
                    delete_branch: true,
                },
            ]
        },
    }
}

fn auto_merge_image_updater() -> Rule {
    Rule {
        name: "auto-merge-ci-image-updater",
        matches: |ev| ev.action == "opened" && ev.author == "ci-image-updater",
        actions: || {
            vec![
                Action::PreflightCheck,
                Action::EnsureLabelsExist {
                    labels: vec![
                        ("image-update".into(), "#0075ca".into()),
                        ("automated".into(), "#e4e669".into()),
                    ],
                },
                Action::AddLabelsByName {
                    labels: vec!["image-update".into(), "automated".into()],
                },
                Action::Approve {
                    body: "Auto-approved: PR from ci-image-updater".into(),
                },
                Action::Merge {
                    strategy: MergePullRequestOptionDo::Squash,
                    delete_branch: true,
                },
            ]
        },
    }
}
