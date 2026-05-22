use std::time::{Duration, Instant};

use crate::clients::Clients;
use crate::command::ACK_LABEL;
use crate::event::BotEvent;
use crate::forgejo::CommitStatusParams;
use crate::rules::matchers::parse_pr_metadata;
use crate::rules::schema::{CloseOtherPrsCriteria, TemplateString};
use forgejo_api::structs::MergePullRequestOptionDo;

pub enum RetryWorkflowTarget {
    GitHub,
}

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
    RetryWorkflow {
        target: RetryWorkflowTarget,
        repository: TemplateString,
        id: TemplateString,
    },
    SetCommitStatus {
        target_owner: String,
        target_repo: String,
        sha: TemplateString,
        state: TemplateString,
        context: TemplateString,
        description: TemplateString,
        target_url: TemplateString,
    },
    WaitForGithubSync {
        target_owner: String,
        target_repo: String,
        sha: TemplateString,
        timeout_secs: u64,
    },
    ArgocdSyncChangedApps,
    CloseOtherPrs {
        author: String,
        criteria: CloseOtherPrsCriteria,
        match_metadata_fields: Vec<String>,
        delete_branch: bool,
        comment: Option<TemplateString>,
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
            Action::RetryWorkflow { .. } => "retry_workflow",
            Action::SetCommitStatus { .. } => "set_commit_status",
            Action::WaitForGithubSync { .. } => "wait_for_github_sync",
            Action::ArgocdSyncChangedApps => "argocd_sync_changed_apps",
            Action::CloseOtherPrs { .. } => "close_other_prs",
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
            Action::RetryWorkflow {
                target,
                repository,
                id,
            } => {
                let vars = event.template_vars();
                let repo_str = repository.render(&vars);
                let id_str = id.render(&vars);
                let Some((owner, repo)) = repo_str.split_once('/') else {
                    tracing::error!(repository = repo_str, "invalid repository format");
                    return;
                };

                let Ok(id) = id_str.parse::<u64>() else {
                    tracing::error!(id = id_str, "invalid id");
                    return;
                };

                match target {
                    RetryWorkflowTarget::GitHub => {
                        if let Err(e) = clients.github.rerun_workflow(owner, repo, id).await {
                            tracing::error!(id, "retry_workflow failed: {e}");
                        }
                    }
                }

                return;
            }
            Action::WaitForGithubSync {
                target_owner,
                target_repo,
                sha,
                timeout_secs,
            } => {
                let vars = event.template_vars();
                let sha = sha.render(&vars);
                let poll_interval = Duration::from_secs(10);
                let deadline = Instant::now() + Duration::from_secs(*timeout_secs);
                loop {
                    if clients
                        .github
                        .commit_exists(target_owner, target_repo, &sha)
                        .await
                    {
                        tracing::info!(
                            owner = target_owner,
                            repo = target_repo,
                            sha,
                            "github sync confirmed"
                        );
                        break;
                    }

                    if Instant::now() + poll_interval >= deadline {
                        tracing::warn!(
                            owner = target_owner,
                            repo = target_repo,
                            sha,
                            timeout_secs,
                            "timed out waiting for github sync"
                        );
                        break;
                    }

                    tokio::time::sleep(poll_interval).await;
                }
                return;
            }
            Action::ArgocdSyncChangedApps => {
                let BotEvent::ForgejoPr(pr) = event else {
                    return;
                };

                if !pr.merged {
                    tracing::info!(pr = pr.pr_number, "PR not merged, skipping argocd sync");
                    return;
                }

                clients.argocd.sync_changed_apps(&pr.changed_files).await;
                return;
            }
            Action::CloseOtherPrs {
                author,
                criteria,
                match_metadata_fields,
                delete_branch,
                comment,
            } => {
                let BotEvent::ForgejoPr(pr) = event else {
                    return;
                };

                let vars = event.template_vars();

                async {
                    // Fetch current PR to get body + created_at
                    let current = client
                        .get_pr(&pr.owner, &pr.repo, pr.pr_number as i64)
                        .await?;
                    let current_body = current.body.as_deref().unwrap_or("");
                    let Some(current_meta) = parse_pr_metadata(current_body) else {
                        tracing::debug!(pr = pr.pr_number, "no metadata in current PR, skipping close_other_prs");
                        return Ok(());
                    };

                    let current_field_values: Vec<_> = match_metadata_fields
                        .iter()
                        .filter_map(|f| {
                            current_meta
                                .get(f)
                                .and_then(|v| v.as_str())
                                .map(|s| (f.as_str(), s.to_owned()))
                        })
                        .collect();

                    if current_field_values.len() != match_metadata_fields.len() {
                        tracing::debug!(pr = pr.pr_number, "missing metadata fields, skipping close_other_prs");
                        return Ok(());
                    }

                    let current_created = current.created_at;

                    // List all open PRs
                    let open_prs = client.list_open_prs(&pr.owner, &pr.repo).await?;

                    for other in &open_prs {
                        let other_number = other.number.unwrap_or(0);
                        if other_number as u64 == pr.pr_number {
                            continue;
                        }

                        let other_author = other
                            .user
                            .as_ref()
                            .and_then(|u| u.login.as_deref())
                            .unwrap_or("");
                        if other_author != author {
                            continue;
                        }

                        let other_body = other.body.as_deref().unwrap_or("");
                        let Some(other_meta) = parse_pr_metadata(other_body) else {
                            continue;
                        };

                        let all_fields_match =
                            current_field_values.iter().all(|(field, value)| {
                                other_meta
                                    .get(*field)
                                    .and_then(|v| v.as_str())
                                    .is_some_and(|s| s == value)
                            });

                        if !all_fields_match {
                            continue;
                        }

                        // Apply criteria
                        let should_close = match criteria {
                            CloseOtherPrsCriteria::Older => other.created_at < current_created,
                        };

                        if !should_close {
                            continue;
                        }

                        // Remove janitor/* labels before closing
                        let janitor_labels: Vec<String> = other
                            .labels
                            .as_deref()
                            .unwrap_or_default()
                            .iter()
                            .filter_map(|l| {
                                l.name
                                    .as_deref()
                                    .filter(|n| n.starts_with("janitor/"))
                                    .map(|n| n.to_owned())
                            })
                            .collect();
                        if !janitor_labels.is_empty() {
                            client
                                .remove_labels_by_name(
                                    &pr.owner,
                                    &pr.repo,
                                    other_number,
                                    janitor_labels,
                                )
                                .await?;
                        }

                        if let Some(tmpl) = comment {
                            let rendered = tmpl.render(&vars);
                            client
                                .comment_on_issue(
                                    &pr.owner,
                                    &pr.repo,
                                    other_number,
                                    &rendered,
                                )
                                .await?;
                        }

                        client
                            .set_pr_state(&pr.owner, &pr.repo, other_number, "closed")
                            .await?;

                        if *delete_branch
                            && let Some(branch) = other
                                .head
                                .as_ref()
                                .and_then(|h| h.r#ref.as_deref())
                            && let Err(e) =
                                client.delete_branch(&pr.owner, &pr.repo, branch).await
                        {
                            tracing::warn!(
                                branch,
                                pr = other_number,
                                "failed to delete branch: {e}"
                            );
                        }

                        tracing::info!(
                            closed = other_number,
                            superseded_by = pr.pr_number,
                            "closed superseded PR"
                        );
                    }

                    Ok(())
                }
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
