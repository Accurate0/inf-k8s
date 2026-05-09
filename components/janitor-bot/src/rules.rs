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
}

impl Action {
    pub async fn execute(&self, client: &ForgejoClient, owner: &str, repo: &str, pr: i64) {
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
        };
        if let Err(e) = result {
            tracing::error!(pr, "action failed: {e}");
        }
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
                action
                    .execute(client, &event.owner, &event.repo, event.pr_number as i64)
                    .await;
            }
        }
    }
}

pub fn all_rules() -> Vec<Rule> {
    vec![auto_merge_image_updater()]
}

fn auto_merge_image_updater() -> Rule {
    Rule {
        name: "auto-merge-ci-image-updater",
        matches: |ev| {
            (ev.action == "opened" || ev.action == "synchronized")
                && ev.author == "ci-image-updater"
        },
        actions: || {
            vec![
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
