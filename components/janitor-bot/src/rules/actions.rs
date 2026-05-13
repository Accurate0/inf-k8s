use crate::command::ACK_LABEL;
use crate::event::{self, BotEvent};
use crate::forgejo::ForgejoClient;
use forgejo_api::structs::MergePullRequestOptionDo;

#[allow(dead_code)]
pub enum Action {
    Approve {
        body: Option<String>,
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
    RemoveLabelsByName {
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
        deduplicate_by_title: bool,
        title: String,
        body: String,
        comment_body: Option<String>,
        labels: Vec<(String, String)>,
    },
    CloseIssue {
        target_owner: String,
        target_repo: String,
        title: String,
        comment_body: Option<String>,
    },
}

impl Action {
    pub fn kind(&self) -> &'static str {
        match self {
            Action::Approve { .. } => "approve",
            Action::Merge { .. } => "merge",
            Action::Comment { .. } => "comment",
            Action::AddLabels { .. } => "add_labels",
            Action::AddLabelsByName { .. } => "add_labels_by_name",
            Action::RemoveLabelsByName { .. } => "remove_labels_by_name",
            Action::EnsureLabelsExist { .. } => "ensure_labels_exist",
            Action::CreateIssue { .. } => "create_issue",
            Action::CloseIssue { .. } => "close_issue",
        }
    }

    pub async fn execute(&self, client: &ForgejoClient, event: &BotEvent<'_>) {
        let result = match self {
            Action::Approve { body } => {
                let BotEvent::ForgejoPr(pr) = event else {
                    return;
                };
                client
                    .approve_pr(&pr.owner, &pr.repo, pr.pr_number as i64, body.as_deref())
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
            Action::RemoveLabelsByName { labels } => {
                let BotEvent::ForgejoPr(pr) = event else {
                    return;
                };
                client
                    .remove_labels_by_name(&pr.owner, &pr.repo, pr.pr_number as i64, labels.clone())
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
                deduplicate_by_title,
                title,
                body,
                comment_body,
                labels,
            } => {
                let vars = event.template_vars();
                let rendered_title = event::render_template(title, &vars);
                let rendered_body = event::render_template(body, &vars);
                async {
                    let existing = if *deduplicate_by_title {
                        client
                            .find_open_issue_by_title(target_owner, target_repo, &rendered_title)
                            .await?
                    } else {
                        None
                    };
                    if let Some(existing) = existing {
                        let index = existing.number.unwrap();
                        let is_acked = existing.labels.as_ref().is_some_and(|ls| {
                            ls.iter().any(|l| l.name.as_deref() == Some(ACK_LABEL))
                        });
                        if is_acked {
                            tracing::info!(issue = index, "skipping comment on acknowledged issue");
                            return Ok(());
                        }
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
                title,
                comment_body,
            } => {
                let vars = event.template_vars();
                let rendered_title = event::render_template(title, &vars);
                async {
                    let Some(existing) = client
                        .find_open_issue_by_title(target_owner, target_repo, &rendered_title)
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
