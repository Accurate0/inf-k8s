use crate::event::{self, CommentEvent};
use crate::forgejo::ForgejoClient;
use crate::rules::RulesOrchestrator;
use forgejo_api::structs::MergePullRequestOptionDo;

#[derive(Debug, Clone, Copy)]
pub enum Command {
    Approve,
    Merge,
    Recheck,
    Close,
    Reopen,
    Ignore,
}

pub const IGNORE_LABEL: &str = "janitor/ignore";

pub fn parse(body: &str) -> Option<Command> {
    let line = body
        .lines()
        .find(|l| l.trim_start().starts_with("@janitor"))?;
    let rest = line.trim_start().strip_prefix("@janitor")?.trim();
    match rest.split_whitespace().next()? {
        "approve" => Some(Command::Approve),
        "merge" => Some(Command::Merge),
        "recheck" => Some(Command::Recheck),
        "close" => Some(Command::Close),
        "reopen" => Some(Command::Reopen),
        "ignore" => Some(Command::Ignore),
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
                    .approve_pr(&cmd.owner, &cmd.repo, pr, "Approved via @janitor approve")
                    .await
            {
                tracing::error!("approve failed: {e}");
            }
        }
        Command::Merge => {
            if !client
                .is_pr_approved_by_bot(&cmd.owner, &cmd.repo, pr)
                .await
                && let Err(e) = client
                    .approve_pr(
                        &cmd.owner,
                        &cmd.repo,
                        pr,
                        "Auto-approved via @janitor merge",
                    )
                    .await
            {
                tracing::error!("approve-before-merge failed: {e}");
                return;
            }
            if let Err(e) = client
                .merge_pr(
                    &cmd.owner,
                    &cmd.repo,
                    pr,
                    MergePullRequestOptionDo::Squash,
                    true,
                )
                .await
            {
                tracing::error!("merge failed: {e}");
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
