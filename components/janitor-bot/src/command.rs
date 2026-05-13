use crate::event::{self, CommentEvent};
use crate::forgejo::ForgejoClient;
use crate::git;
use crate::rules::RulesOrchestrator;
use forgejo_api::structs::MergePullRequestOptionDo;

#[derive(Debug, Clone, Copy)]
pub enum Command {
    Approve,
    Merge {
        strategy: MergePullRequestOptionDo,
    },
    Revert,
    Recheck,
    Close,
    Reopen,
    Ignore,
    Explain,
}

pub const IGNORE_LABEL: &str = "janitor/ignore";

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

pub fn parse(body: &str) -> Option<Command> {
    let line = body
        .lines()
        .find(|l| l.trim_start().starts_with("@janitor"))?;
    let rest = line.trim_start().strip_prefix("@janitor")?.trim();
    let mut words = rest.split_whitespace();
    match words.next()? {
        "approve" => Some(Command::Approve),
        "merge" => {
            let strategy = match words.next() {
                Some("rebase") => MergePullRequestOptionDo::Rebase,
                Some("merge") => MergePullRequestOptionDo::Merge,
                Some("squash") | None => MergePullRequestOptionDo::Squash,
                _ => return None,
            };
            Some(Command::Merge { strategy })
        }
        "revert" => Some(Command::Revert),
        "recheck" => Some(Command::Recheck),
        "close" => Some(Command::Close),
        "reopen" => Some(Command::Reopen),
        "ignore" => Some(Command::Ignore),
        "explain" => Some(Command::Explain),
        _ => None,
    }
}

pub async fn handle(
    client: &ForgejoClient,
    orchestrator: &RulesOrchestrator,
    cmd: &CommentEvent,
    command: Command,
) {
    let pr = cmd.pr_number as i64;
    tracing::info!(
        ?command,
        pr,
        owner = cmd.owner,
        repo = cmd.repo,
        "running command"
    );
    match command {
        Command::Approve => {
            if !client
                .is_pr_approved_by_bot(&cmd.owner, &cmd.repo, pr)
                .await
                && let Err(e) = client
                    .approve_pr(&cmd.owner, &cmd.repo, pr, None)
                    .await
            {
                tracing::error!("approve failed: {e}");
            }
        }
        Command::Merge { strategy } => {
            if !client
                .is_pr_approved_by_bot(&cmd.owner, &cmd.repo, pr)
                .await
                && let Err(e) = client
                    .approve_pr(&cmd.owner, &cmd.repo, pr, None)
                    .await
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
        Command::Revert => {
            match revert_pr(client, &cmd.owner, &cmd.repo, pr).await {
                Ok(revert_pr_number) => {
                    tracing::info!(pr, revert_pr = revert_pr_number, "revert PR created");
                }
                Err(e) => {
                    tracing::error!("revert failed: {e}");
                }
            }
        }
        Command::Ignore => {
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
        Command::Close => {
            if let Err(e) = client
                .set_pr_state(&cmd.owner, &cmd.repo, pr, "closed")
                .await
            {
                tracing::error!("close failed: {e}");
            }
        }
        Command::Reopen => {
            if let Err(e) = client.set_pr_state(&cmd.owner, &cmd.repo, pr, "open").await {
                tracing::error!("reopen failed: {e}");
            }
        }
        Command::Explain => {
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
        Command::Recheck => {
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
        git::revert_and_push(&clone_url, &token, &merge_sha, &commit_msg, &branch, &target)
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
