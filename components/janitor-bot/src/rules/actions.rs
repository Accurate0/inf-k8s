use crate::clients::Clients;
use crate::command::ACK_LABEL;
use crate::event::BotEvent;
use crate::forgejo::CommitStatusParams;
use crate::rules::schema::TemplateString;
use forgejo_api::structs::MergePullRequestOptionDo;

#[allow(dead_code)]
pub enum Action {
    Approve {
        body: Option<TemplateString>,
    },
    Merge {
        strategy: MergePullRequestOptionDo,
        delete_branch: bool,
    },
    Comment {
        body: TemplateString,
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
        title: TemplateString,
        body: TemplateString,
        on_duplicate_comment: Option<TemplateString>,
        labels: Vec<(String, String)>,
    },
    CloseIssue {
        target_owner: String,
        target_repo: String,
        title: TemplateString,
        closing_comment: Option<TemplateString>,
    },
    ArgoCdDiff,
    SetCommitStatus {
        target_owner: String,
        target_repo: String,
        sha: TemplateString,
        state: TemplateString,
        context: TemplateString,
        description: TemplateString,
        target_url: TemplateString,
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
            Action::ArgoCdDiff => "argocd_diff",
            Action::SetCommitStatus { .. } => "set_commit_status",
        }
    }

    pub async fn execute(&self, clients: &Clients, event: &BotEvent<'_>) {
        let client = &clients.forgejo;
        let result = match self {
            Action::Approve { body } => {
                let BotEvent::ForgejoPr(pr) = event else {
                    return;
                };
                let vars = event.template_vars();
                let rendered = body.as_ref().map(|b| b.render(&vars));
                client
                    .approve_pr(
                        &pr.owner,
                        &pr.repo,
                        pr.pr_number as i64,
                        rendered.as_deref(),
                    )
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
                let vars = event.template_vars();
                let rendered = body.render(&vars);
                client
                    .comment(&pr.owner, &pr.repo, pr.pr_number as i64, &rendered)
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
                on_duplicate_comment,
                labels,
            } => {
                let vars = event.template_vars();
                let rendered_title = title.render(&vars);
                let rendered_body = body.render(&vars);
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
                        let text = on_duplicate_comment
                            .as_ref()
                            .map(|t| t.render(&vars))
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
                closing_comment,
            } => {
                let vars = event.template_vars();
                let rendered_title = title.render(&vars);
                async {
                    let Some(existing) = client
                        .find_open_issue_by_title(target_owner, target_repo, &rendered_title)
                        .await?
                    else {
                        return Ok(());
                    };
                    let index = existing.number.unwrap();
                    if let Some(tmpl) = closing_comment {
                        let text = tmpl.render(&vars);
                        client
                            .comment_on_issue(target_owner, target_repo, index, &text)
                            .await?;
                    }
                    client.close_issue(target_owner, target_repo, index).await?;
                    Ok(())
                }
                .await
            }
            Action::SetCommitStatus {
                target_owner,
                target_repo,
                sha,
                state,
                context,
                description,
                target_url,
            } => {
                let vars = event.template_vars();
                client
                    .set_commit_status(CommitStatusParams {
                        owner: target_owner,
                        repo: target_repo,
                        sha: &sha.render(&vars),
                        state: &state.render(&vars),
                        context: &context.render(&vars),
                        description: &description.render(&vars),
                        target_url: &target_url.render(&vars),
                    })
                    .await
            }
            Action::ArgoCdDiff => {
                let BotEvent::ForgejoPr(pr) = event else {
                    return;
                };
                if let Err(e) = clients.argocd.run_diff_and_comment(client, pr).await {
                    tracing::error!("argocd_diff action failed: {e}");
                }
                return;
            }
        };
        if let Err(e) = result {
            tracing::error!("action failed: {e}");
        }
    }
}
