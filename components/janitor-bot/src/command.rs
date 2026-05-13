use crate::event::{self, CommentEvent, IssueCommentEvent};
use crate::forgejo::ForgejoClient;
use crate::git;
use crate::github::GitHubClient;
use crate::rules::RulesOrchestrator;
use forgejo_api::structs::MergePullRequestOptionDo;

#[derive(Debug, Clone, Copy)]
pub enum PrCommand {
    Approve,
    Merge { strategy: MergePullRequestOptionDo },
    Revert,
    Recheck,
    Close,
    Reopen,
    Ignore,
    Explain,
}

#[derive(Debug, Clone, Copy)]
pub enum IssueCommand {
    RetryWorkflow,
    Ack,
}

pub const IGNORE_LABEL: &str = "janitor/ignore";
pub const ACK_LABEL: &str = "janitor/acknowledged";

fn format_explain(matched: &[(String, bool, Vec<&'static str>)]) -> String {
    if matched.is_empty() {
        return "## janitor explain\n\nNo rules matched this PR.".to_owned();
    }
    let mut s = String::from("## janitor explain\n\n");
    for (name, dry_run, actions) in matched {
        let suffix = if *dry_run { " _(dry-run)_" } else { "" };
        s.push_str(&format!("- **{name}**{suffix}: {}\n", actions.join(", ")));
    }
    s
}

pub fn parse_pr_command(body: &str) -> Option<PrCommand> {
    let line = body
        .lines()
        .find(|l| l.trim_start().starts_with("@janitor"))?;
    let rest = line.trim_start().strip_prefix("@janitor")?.trim();
    let mut words = rest.split_whitespace();
    match words.next()? {
        "approve" => Some(PrCommand::Approve),
        "merge" => {
            let strategy = match words.next() {
                Some("rebase") => MergePullRequestOptionDo::Rebase,
                Some("merge") => MergePullRequestOptionDo::Merge,
                Some("squash") | None => MergePullRequestOptionDo::Squash,
                _ => return None,
            };
            Some(PrCommand::Merge { strategy })
        }
        "revert" => Some(PrCommand::Revert),
        "recheck" => Some(PrCommand::Recheck),
        "close" => Some(PrCommand::Close),
        "reopen" => Some(PrCommand::Reopen),
        "ignore" => Some(PrCommand::Ignore),
        "explain" => Some(PrCommand::Explain),
        _ => None,
    }
}

pub fn parse_issue_command(body: &str) -> Option<IssueCommand> {
    let line = body
        .lines()
        .find(|l| l.trim_start().starts_with("@janitor"))?;
    let rest = line.trim_start().strip_prefix("@janitor")?.trim();
    let mut words = rest.split_whitespace();
    match words.next()? {
        "retry-workflow" => Some(IssueCommand::RetryWorkflow),
        "ack" => Some(IssueCommand::Ack),
        _ => None,
    }
}

pub async fn handle_pr_command(
    client: &ForgejoClient,
    orchestrator: &RulesOrchestrator,
    cmd: &CommentEvent,
    command: PrCommand,
) {
    let pr = cmd.pr_number as i64;
    tracing::info!(
        ?command,
        pr,
        owner = cmd.owner,
        repo = cmd.repo,
        "running PR command"
    );
    match command {
        PrCommand::Approve => {
            if !client
                .is_pr_approved_by_bot(&cmd.owner, &cmd.repo, pr)
                .await
                && let Err(e) = client.approve_pr(&cmd.owner, &cmd.repo, pr, None).await
            {
                tracing::error!("approve failed: {e}");
            }
        }
        PrCommand::Merge { strategy } => {
            if !client
                .is_pr_approved_by_bot(&cmd.owner, &cmd.repo, pr)
                .await
                && let Err(e) = client.approve_pr(&cmd.owner, &cmd.repo, pr, None).await
            {
                tracing::error!("approve-before-merge failed: {e}");
                return;
            }
            if let Err(e) = client
                .merge_pr(&cmd.owner, &cmd.repo, pr, strategy, true)
                .await
            {
                tracing::error!("merge failed: {e}");
            }
        }
        PrCommand::Revert => match revert_pr(client, &cmd.owner, &cmd.repo, pr).await {
            Ok(revert_pr_number) => {
                tracing::info!(pr, revert_pr = revert_pr_number, "revert PR created");
            }
            Err(e) => {
                tracing::error!("revert failed: {e}");
            }
        },
        PrCommand::Ignore => {
            if let Err(e) = client
                .ensure_labels(
                    &cmd.owner,
                    &cmd.repo,
                    vec![(IGNORE_LABEL.to_owned(), "#cccccc".to_owned())],
                )
                .await
            {
                tracing::error!("ensure ignore label failed: {e}");
                return;
            }
            if let Err(e) = client
                .add_labels_by_name(&cmd.owner, &cmd.repo, pr, vec![IGNORE_LABEL.to_owned()])
                .await
            {
                tracing::error!("add ignore label failed: {e}");
            }
        }
        PrCommand::Close => {
            if let Err(e) = client
                .set_pr_state(&cmd.owner, &cmd.repo, pr, "closed")
                .await
            {
                tracing::error!("close failed: {e}");
            }
        }
        PrCommand::Reopen => {
            if let Err(e) = client.set_pr_state(&cmd.owner, &cmd.repo, pr, "open").await {
                tracing::error!("reopen failed: {e}");
            }
        }
        PrCommand::Explain => {
            let api_pr = match client.get_pr(&cmd.owner, &cmd.repo, pr).await {
                Ok(p) => p,
                Err(e) => {
                    tracing::error!("explain: failed to fetch PR: {e}");
                    return;
                }
            };
            let Some(mut pr_event) =
                event::PrEvent::from_api_pr(&api_pr, cmd.owner.clone(), cmd.repo.clone())
            else {
                tracing::warn!("explain: could not build PrEvent");
                return;
            };
            let matched = orchestrator.explain_pr(client, &mut pr_event).await;
            let body = format_explain(&matched);
            if let Err(e) = client.comment(&cmd.owner, &cmd.repo, pr, &body).await {
                tracing::error!("explain: comment failed: {e}");
            }
        }
        PrCommand::Recheck => {
            let api_pr = match client.get_pr(&cmd.owner, &cmd.repo, pr).await {
                Ok(p) => p,
                Err(e) => {
                    tracing::error!("recheck: failed to fetch PR: {e}");
                    return;
                }
            };
            let Some(mut pr_event) =
                event::PrEvent::from_api_pr(&api_pr, cmd.owner.clone(), cmd.repo.clone())
            else {
                tracing::warn!("recheck: could not build PrEvent");
                return;
            };
            orchestrator.evaluate_pr(client, &mut pr_event).await;
        }
    }
}

fn extract_metadata_json(text: &str) -> Option<serde_json::Value> {
    let start = text.rfind("<!-- ")? + 5;
    let end = text[start..].find(" -->")? + start;
    serde_json::from_str(text[start..end].trim()).ok()
}

pub async fn handle_issue_command(
    client: &ForgejoClient,
    github_client: &GitHubClient,
    event: &IssueCommentEvent,
    command: IssueCommand,
) {
    tracing::info!(
        ?command,
        issue = event.issue_number,
        owner = event.owner,
        repo = event.repo,
        "running issue command"
    );
    match command {
        IssueCommand::RetryWorkflow => {
            if !event.issue_labels.iter().any(|l| l == "github-ci-failure") {
                tracing::warn!(
                    issue = event.issue_number,
                    "retry-workflow: issue missing github-ci-failure label"
                );
                return;
            }
            let Some(metadata) = extract_metadata_json(&event.issue_body) else {
                tracing::warn!(
                    issue = event.issue_number,
                    "retry-workflow: no metadata JSON found in issue body"
                );
                return;
            };
            let Some(run_id) = metadata["run_id"].as_u64() else {
                tracing::warn!(
                    issue = event.issue_number,
                    "retry-workflow: no run_id in metadata"
                );
                return;
            };
            let repo = metadata["html_url"].as_str().and_then(|url| {
                // https://github.com/{owner}/{repo}/actions/runs/{id}
                let path = url.strip_prefix("https://github.com/")?;
                let parts: Vec<&str> = path.splitn(3, '/').collect();
                if parts.len() >= 2 {
                    Some(format!("{}/{}", parts[0], parts[1]))
                } else {
                    None
                }
            });
            let Some(full_repo) = repo else {
                tracing::warn!(
                    issue = event.issue_number,
                    "retry-workflow: could not extract repo from html_url"
                );
                return;
            };
            let (owner, repo) = full_repo.split_once('/').unwrap();
            if let Err(e) = github_client.rerun_workflow(owner, repo, run_id).await {
                tracing::error!(
                    issue = event.issue_number,
                    run_id,
                    "retry-workflow failed: {e}"
                );
                if let Err(e) = client
                    .comment_on_issue(
                        &event.owner,
                        &event.repo,
                        event.issue_number as i64,
                        &format!("Failed to retry workflow run {run_id}: {e}"),
                    )
                    .await
                {
                    tracing::error!("failed to comment on issue: {e}");
                }
            }
        }
        IssueCommand::Ack => {
            let issue = event.issue_number as i64;
            if let Err(e) = client
                .ensure_labels(
                    &event.owner,
                    &event.repo,
                    vec![(ACK_LABEL.to_owned(), "#c5def5".to_owned())],
                )
                .await
            {
                tracing::error!("ensure ack label failed: {e}");
                return;
            }
            if let Err(e) = client
                .add_labels_by_name(&event.owner, &event.repo, issue, vec![ACK_LABEL.to_owned()])
                .await
            {
                tracing::error!("add ack label failed: {e}");
            }
        }
    }
}

async fn revert_pr(
    client: &ForgejoClient,
    owner: &str,
    repo: &str,
    pr: i64,
) -> Result<i64, anyhow::Error> {
    let api_pr = client.get_pr(owner, repo, pr).await?;

    if !api_pr.merged.unwrap_or(false) {
        anyhow::bail!("PR #{pr} is not merged");
    }

    let merge_sha = api_pr
        .merge_commit_sha
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("PR #{pr} has no merge commit SHA"))?;

    let original_title = api_pr.title.as_deref().unwrap_or("unknown");
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let branch_name = format!("revert-pr-{pr}-{timestamp}");
    let target_branch = api_pr
        .base
        .as_ref()
        .and_then(|b| b.r#ref.clone())
        .unwrap_or_else(|| "main".to_owned());

    let clone_url = client.clone_url(owner, repo);
    let token = client.token.clone();
    let commit_msg = format!("Revert \"{original_title}\" (#{pr})");
    let branch = branch_name.clone();
    let target = target_branch.clone();

    tokio::task::spawn_blocking(move || {
        git::revert_and_push(
            &clone_url,
            &token,
            &merge_sha,
            &commit_msg,
            &branch,
            &target,
        )
    })
    .await??;

    let revert = client
        .create_pull_request(
            owner,
            repo,
            &format!("Revert \"{original_title}\""),
            &format!("Reverts #{pr}"),
            &branch_name,
            &target_branch,
        )
        .await?;

    Ok(revert.number.unwrap_or(0))
}
