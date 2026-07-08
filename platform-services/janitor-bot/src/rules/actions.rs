use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::clients::Clients;
use crate::command::ACK_LABEL;
use crate::event::{BotEvent, RawRequest};
use crate::forgejo::CommitStatusParams;
use crate::rules::matchers::ResourceCache;
use crate::rules::matchers::cache::{get_changed_files_cached, pr_image_committed_at};
use crate::rules::matchers::{metadata_order_key, parse_pr_metadata};
use crate::rules::schema::{CloseOtherPrsCriteria, LabelSpec, TemplateString};
use forgejo_api::structs::MergePullRequestOptionDo;

pub enum RetryWorkflowTarget {
    GitHub,
}

pub enum ProxyService {
    Argocd,
}

#[allow(dead_code)]
pub enum Action {
    Approve {
        comment: Option<TemplateString>,
    },
    Merge {
        strategy: MergePullRequestOptionDo,
        delete_branch: bool,
    },
    Comment {
        comment: TemplateString,
    },
    AddLabels {
        labels: Vec<LabelSpec>,
        target_owner: Option<String>,
        target_repo: Option<String>,
    },
    RemoveLabels {
        labels: Vec<String>,
        prefixes: Vec<String>,
    },
    CreateIssue {
        target_owner: String,
        target_repo: String,
        deduplicate_by_title: bool,
        title: TemplateString,
        body: TemplateString,
        on_duplicate_comment: Option<TemplateString>,
        labels: Vec<LabelSpec>,
    },
    CloseIssue {
        target_owner: String,
        target_repo: String,
        title: TemplateString,
        comment: Option<TemplateString>,
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
    ProxyPass {
        service: ProxyService,
    },
    CloseOtherPrs {
        author: String,
        criteria: CloseOtherPrsCriteria,
        match_metadata_fields: Vec<String>,
        order_by_metadata_field: Option<String>,
        images_metadata_field: String,
        tag_metadata_field: String,
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
            Action::RemoveLabels { .. } => "remove_labels",
            Action::CreateIssue { .. } => "create_issue",
            Action::CloseIssue { .. } => "close_issue",
            Action::ArgoCdDiff => "argocd_diff",
            Action::RetryWorkflow { .. } => "retry_workflow",
            Action::SetCommitStatus { .. } => "set_commit_status",
            Action::WaitForGithubSync { .. } => "wait_for_github_sync",
            Action::ProxyPass { .. } => "proxy_pass",
            Action::CloseOtherPrs { .. } => "close_other_prs",
        }
    }

    pub async fn execute(
        &self,
        clients: &Clients,
        event: &BotEvent<'_>,
        cache: &ResourceCache,
        raw: Option<&RawRequest>,
        extra_vars: &HashMap<&'static str, String>,
    ) {
        let client = &clients.forgejo;
        let result = match self {
            Action::Approve { comment } => {
                let BotEvent::ForgejoPr(pr) = event else {
                    return;
                };

                let vars = event.template_vars();
                let rendered = comment.as_ref().map(|c| c.render(&vars));
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
            Action::Comment { comment } => {
                let BotEvent::ForgejoPr(pr) = event else {
                    return;
                };

                let vars = event.template_vars();
                let rendered = comment.render(&vars);
                client
                    .comment(&pr.owner, &pr.repo, pr.pr_number as i64, &rendered)
                    .await
            }
            Action::AddLabels {
                labels,
                target_owner,
                target_repo,
            } => {
                let BotEvent::ForgejoPr(pr) = event else {
                    return;
                };

                let (owner, repo) = match (target_owner, target_repo) {
                    (Some(o), Some(r)) => (o.as_str(), r.as_str()),
                    _ => (pr.owner.as_str(), pr.repo.as_str()),
                };

                async {
                    let to_ensure: Vec<(String, String)> = labels
                        .iter()
                        .filter_map(|l| l.color().map(|c| (l.name().to_string(), c.to_string())))
                        .collect();
                    if !to_ensure.is_empty() {
                        client.ensure_labels(owner, repo, to_ensure).await?;
                    }

                    let names: Vec<String> = labels.iter().map(|l| l.name().to_string()).collect();
                    client
                        .add_labels_by_name(owner, repo, pr.pr_number as i64, names)
                        .await
                }
                .await
            }
            Action::RemoveLabels { labels, prefixes } => {
                let BotEvent::ForgejoPr(pr) = event else {
                    return;
                };

                let mut to_remove = labels.clone();
                if !prefixes.is_empty() {
                    for label in &pr.labels {
                        if prefixes.iter().any(|p| label.name.starts_with(p))
                            && !to_remove.contains(&label.name)
                        {
                            to_remove.push(label.name.clone());
                        }
                    }
                }

                if to_remove.is_empty() {
                    return;
                }

                client
                    .remove_labels_by_name(&pr.owner, &pr.repo, pr.pr_number as i64, to_remove)
                    .await
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
                        let to_ensure: Vec<(String, String)> = labels
                            .iter()
                            .filter_map(|l| {
                                l.color().map(|c| (l.name().to_string(), c.to_string()))
                            })
                            .collect();
                        if !to_ensure.is_empty() {
                            client
                                .ensure_labels(target_owner, target_repo, to_ensure)
                                .await?;
                        }
                        let label_names: Vec<String> =
                            labels.iter().map(|l| l.name().to_string()).collect();
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
                comment,
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
                    if let Some(tmpl) = comment {
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
                let mut vars = event.template_vars();
                vars.extend(extra_vars.iter().map(|(k, v)| (k.to_string(), v.clone())));

                let owner = crate::event::render_template(target_owner, &vars);
                let repo = crate::event::render_template(target_repo, &vars);
                if owner.is_empty() || repo.is_empty() {
                    tracing::debug!(
                        target_owner,
                        target_repo,
                        "skipping set_commit_status: target did not resolve (no mirror mapping)"
                    );
                    return;
                }

                client
                    .set_commit_status(CommitStatusParams {
                        owner: &owner,
                        repo: &repo,
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
                let poll_interval = Duration::from_secs(5);
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
            Action::ProxyPass { service } => {
                let Some(raw) = raw else {
                    tracing::warn!("proxy_pass action has no raw request to forward; skipping");
                    return;
                };

                let result = match service {
                    ProxyService::Argocd => {
                        clients
                            .argocd
                            .forward_webhook(&raw.body, &raw.headers)
                            .await
                    }
                };

                if let Err(e) = result {
                    tracing::error!("proxy_pass to argocd failed: {e}");
                }

                return;
            }
            Action::CloseOtherPrs {
                author,
                criteria,
                match_metadata_fields,
                order_by_metadata_field,
                images_metadata_field,
                tag_metadata_field,
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
                        tracing::debug!(
                            pr = pr.pr_number,
                            "no metadata in current PR, skipping close_other_prs"
                        );
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
                        tracing::debug!(
                            pr = pr.pr_number,
                            "missing metadata fields, skipping close_other_prs"
                        );
                        return Ok(());
                    }

                    let current_created = current.created_at;
                    let current_order_key = order_by_metadata_field
                        .as_deref()
                        .and_then(|f| metadata_order_key(&current_meta, f));

                    let current_image_ts =
                        if matches!(criteria, CloseOtherPrsCriteria::OlderPublishedImage) {
                            pr_image_committed_at(
                                clients,
                                cache,
                                &current_meta,
                                images_metadata_field,
                                tag_metadata_field,
                            )
                            .await
                        } else {
                            None
                        };

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

                        let all_fields_match = current_field_values.iter().all(|(field, value)| {
                            other_meta
                                .get(*field)
                                .and_then(|v| v.as_str())
                                .is_some_and(|s| s == value)
                        });

                        if !all_fields_match {
                            continue;
                        }

                        let should_close = match criteria {
                            CloseOtherPrsCriteria::Older => {
                                match (order_by_metadata_field.as_deref(), current_order_key) {
                                    (Some(f), Some(cur_key)) => {
                                        match metadata_order_key(&other_meta, f) {
                                            Some(other_key) => other_key < cur_key,
                                            None => other.created_at < current_created,
                                        }
                                    }
                                    _ => other.created_at < current_created,
                                }
                            }
                            CloseOtherPrsCriteria::OlderPublishedImage => {
                                let other_ts = pr_image_committed_at(
                                    clients,
                                    cache,
                                    &other_meta,
                                    images_metadata_field,
                                    tag_metadata_field,
                                )
                                .await;
                                match (current_image_ts, other_ts) {
                                    (Some(cur), Some(other_key)) => other_key < cur,
                                    _ => false,
                                }
                            }
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
                                .comment_on_issue(&pr.owner, &pr.repo, other_number, &rendered)
                                .await?;
                        }

                        client
                            .set_pr_state(&pr.owner, &pr.repo, other_number, "closed")
                            .await?;

                        if *delete_branch
                            && let Some(branch) =
                                other.head.as_ref().and_then(|h| h.r#ref.as_deref())
                            && let Err(e) = client.delete_branch(&pr.owner, &pr.repo, branch).await
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

                let files = get_changed_files_cached(clients, cache, pr).await;
                if let Err(e) = clients
                    .argocd
                    .run_diff_and_comment(client, pr, &files)
                    .await
                {
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
