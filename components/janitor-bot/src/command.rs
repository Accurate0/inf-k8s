use crate::clients::Clients;
use crate::event::{self, CommentEvent, IssueCommentEvent};
use crate::forgejo::ForgejoClient;
use crate::git;
use crate::rules::RulesOrchestrator;
use forgejo_api::structs::MergePullRequestOptionDo;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub enum PrCommand {
    Approve,
    Merge { strategy: MergePullRequestOptionDo },
    Revert,
    Recheck,
    Close,
    Reopen,
    Ignore,
    Explain,
    RunRule { name: String },
}

#[derive(Debug, Clone, Copy)]
pub enum IssueCommand {
    RetryWorkflow,
    Ack,
}

pub const IGNORE_LABEL: &str = "janitor/ignore";
pub const ACK_LABEL: &str = "janitor/acknowledged";

fn format_explain(matched: &[crate::rules::MatchedRule]) -> String {
    if matched.is_empty() {
        return "## janitor explain\n\nNo rules matched this PR.".to_owned();
    }
    let mut s = String::from("## janitor explain\n\n");
    for rule in matched {
        let suffix = if rule.dry_run { " _(dry-run)_" } else { "" };
        s.push_str(&format!(
            "- **{}**{suffix}: {}\n",
            rule.name,
            rule.actions.join(", ")
        ));
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
        "run" => {
            if words.next()? != "rule" {
                return None;
            }
            let name = words.next()?.to_owned();
            Some(PrCommand::RunRule { name })
        }
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
    clients: &Clients,
    orchestrator: &RulesOrchestrator,
    cmd: &CommentEvent,
    command: PrCommand,
) {
    let pr = cmd.pr_number as i64;
    let client = &clients.forgejo;
    tracing::info!(
        ?command,
        pr,
        owner = cmd.owner,
        repo = cmd.repo,
        "running PR command"
    );
    if let Err(e) = client
        .react_to_comment(&cmd.owner, &cmd.repo, cmd.comment_id, "+1")
        .await
    {
        tracing::warn!(pr, "failed to react to command comment: {e}");
    }
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
            let matched = orchestrator.explain_pr(clients, &mut pr_event).await;
            let body = format_explain(&matched);
            if let Err(e) = client.comment(&cmd.owner, &cmd.repo, pr, &body).await {
                tracing::error!("explain: comment failed: {e}");
            }
        }
        PrCommand::RunRule { name } => {
            let api_pr = match client.get_pr(&cmd.owner, &cmd.repo, pr).await {
                Ok(p) => p,
                Err(e) => {
                    tracing::error!("run action: failed to fetch PR: {e}");
                    return;
                }
            };
            let Some(mut pr_event) =
                event::PrEvent::from_api_pr(&api_pr, cmd.owner.clone(), cmd.repo.clone())
            else {
                tracing::warn!("run action: could not build PrEvent");
                return;
            };
            if let Err(e) = orchestrator
                .run_rule_by_name_unconditionally(clients, &mut pr_event, &name)
                .await
            {
                tracing::error!("run action failed: {e}");
                if let Err(e) = client
                    .comment(
                        &cmd.owner,
                        &cmd.repo,
                        pr,
                        &format!("Failed to run action `{name}`: {e}"),
                    )
                    .await
                {
                    tracing::error!("failed to comment: {e}");
                }
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
            orchestrator.evaluate_pr(clients, &mut pr_event).await;
        }
    }
}

fn extract_metadata_json(text: &str) -> Option<serde_json::Value> {
    let start = text.rfind("<!-- ")? + 5;
    let end = text[start..].find(" -->")? + start;
    serde_json::from_str(text[start..end].trim()).ok()
}

pub async fn handle_issue_command(
    clients: &Clients,
    event: &IssueCommentEvent,
    command: IssueCommand,
) {
    let client = &clients.forgejo;
    tracing::info!(
        ?command,
        issue = event.issue_number,
        owner = event.owner,
        repo = event.repo,
        "running issue command"
    );
    if let Err(e) = client
        .react_to_comment(&event.owner, &event.repo, event.comment_id, "+1")
        .await
    {
        tracing::warn!(
            issue = event.issue_number,
            "failed to react to command comment: {e}"
        );
    }
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
            if let Err(e) = clients.github.rerun_workflow(owner, repo, run_id).await {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_approve() {
        let cmd = parse_pr_command("@janitor approve").unwrap();
        assert!(matches!(cmd, PrCommand::Approve));
    }

    #[test]
    fn parse_merge_default_squash() {
        let cmd = parse_pr_command("@janitor merge").unwrap();
        match cmd {
            PrCommand::Merge { strategy } => {
                assert!(matches!(strategy, MergePullRequestOptionDo::Squash));
            }
            _ => panic!("expected Merge"),
        }
    }

    #[test]
    fn parse_merge_squash_explicit() {
        let cmd = parse_pr_command("@janitor merge squash").unwrap();
        match cmd {
            PrCommand::Merge { strategy } => {
                assert!(matches!(strategy, MergePullRequestOptionDo::Squash));
            }
            _ => panic!("expected Merge"),
        }
    }

    #[test]
    fn parse_merge_rebase() {
        let cmd = parse_pr_command("@janitor merge rebase").unwrap();
        match cmd {
            PrCommand::Merge { strategy } => {
                assert!(matches!(strategy, MergePullRequestOptionDo::Rebase));
            }
            _ => panic!("expected Merge"),
        }
    }

    #[test]
    fn parse_merge_merge() {
        let cmd = parse_pr_command("@janitor merge merge").unwrap();
        match cmd {
            PrCommand::Merge { strategy } => {
                assert!(matches!(strategy, MergePullRequestOptionDo::Merge));
            }
            _ => panic!("expected Merge"),
        }
    }

    #[test]
    fn parse_merge_invalid_strategy() {
        assert!(parse_pr_command("@janitor merge yolo").is_none());
    }

    #[test]
    fn parse_revert() {
        assert!(matches!(
            parse_pr_command("@janitor revert").unwrap(),
            PrCommand::Revert
        ));
    }

    #[test]
    fn parse_recheck() {
        assert!(matches!(
            parse_pr_command("@janitor recheck").unwrap(),
            PrCommand::Recheck
        ));
    }

    #[test]
    fn parse_close() {
        assert!(matches!(
            parse_pr_command("@janitor close").unwrap(),
            PrCommand::Close
        ));
    }

    #[test]
    fn parse_reopen() {
        assert!(matches!(
            parse_pr_command("@janitor reopen").unwrap(),
            PrCommand::Reopen
        ));
    }

    #[test]
    fn parse_ignore() {
        assert!(matches!(
            parse_pr_command("@janitor ignore").unwrap(),
            PrCommand::Ignore
        ));
    }

    #[test]
    fn parse_explain() {
        assert!(matches!(
            parse_pr_command("@janitor explain").unwrap(),
            PrCommand::Explain
        ));
    }

    #[test]
    fn parse_run_rule() {
        let cmd = parse_pr_command("@janitor run rule argocd-diff-renovate").unwrap();
        match cmd {
            PrCommand::RunRule { name } => assert_eq!(name, "argocd-diff-renovate"),
            _ => panic!("expected RunAction"),
        }
    }

    #[test]
    fn parse_run_rule_missing_name() {
        assert!(parse_pr_command("@janitor run rule").is_none());
    }

    #[test]
    fn parse_run_invalid_subcommand() {
        assert!(parse_pr_command("@janitor run something foo").is_none());
    }

    #[test]
    fn parse_unknown_command() {
        assert!(parse_pr_command("@janitor unknown").is_none());
    }

    #[test]
    fn parse_no_janitor_mention() {
        assert!(parse_pr_command("just a normal comment").is_none());
    }

    #[test]
    fn parse_command_in_multiline_body() {
        let body = "Some context here\n\n@janitor merge rebase\n\nmore text";
        let cmd = parse_pr_command(body).unwrap();
        assert!(
            matches!(cmd, PrCommand::Merge { strategy } if matches!(strategy, MergePullRequestOptionDo::Rebase))
        );
    }

    #[test]
    fn parse_command_with_leading_whitespace() {
        let cmd = parse_pr_command("  @janitor approve").unwrap();
        assert!(matches!(cmd, PrCommand::Approve));
    }

    #[test]
    fn parse_empty_body() {
        assert!(parse_pr_command("").is_none());
    }

    #[test]
    fn parse_janitor_no_command() {
        assert!(parse_pr_command("@janitor").is_none());
    }

    #[test]
    fn parse_issue_retry_workflow() {
        let cmd = parse_issue_command("@janitor retry-workflow").unwrap();
        assert!(matches!(cmd, IssueCommand::RetryWorkflow));
    }

    #[test]
    fn parse_issue_ack() {
        let cmd = parse_issue_command("@janitor ack").unwrap();
        assert!(matches!(cmd, IssueCommand::Ack));
    }

    #[test]
    fn parse_issue_unknown() {
        assert!(parse_issue_command("@janitor merge").is_none());
    }

    #[test]
    fn parse_issue_no_mention() {
        assert!(parse_issue_command("some comment").is_none());
    }

    #[test]
    fn format_explain_no_matches() {
        let result = format_explain(&[]);
        assert!(result.contains("No rules matched"));
    }

    #[test]
    fn format_explain_single_rule() {
        use crate::rules::MatchedRule;
        let matched = vec![MatchedRule {
            name: "auto-merge".to_string(),
            dry_run: false,
            actions: vec!["approve", "merge"],
        }];
        let result = format_explain(&matched);
        assert!(result.contains("**auto-merge**"));
        assert!(result.contains("approve, merge"));
        assert!(!result.contains("dry-run"));
    }

    #[test]
    fn format_explain_dry_run() {
        use crate::rules::MatchedRule;
        let matched = vec![MatchedRule {
            name: "test-rule".to_string(),
            dry_run: true,
            actions: vec!["comment"],
        }];
        let result = format_explain(&matched);
        assert!(result.contains("_(dry-run)_"));
    }

    #[test]
    fn format_explain_multiple_rules() {
        use crate::rules::MatchedRule;
        let matched = vec![
            MatchedRule {
                name: "rule1".to_string(),
                dry_run: false,
                actions: vec!["approve"],
            },
            MatchedRule {
                name: "rule2".to_string(),
                dry_run: true,
                actions: vec!["merge"],
            },
        ];
        let result = format_explain(&matched);
        assert!(result.contains("**rule1**"));
        assert!(result.contains("**rule2**"));
    }

    #[test]
    fn extract_metadata_basic() {
        let text = r#"some text <!-- {"run_id": 123, "workflow": "build"} --> more"#;
        let val = extract_metadata_json(text).unwrap();
        assert_eq!(val["run_id"], 123);
        assert_eq!(val["workflow"], "build");
    }

    #[test]
    fn extract_metadata_no_comment() {
        assert!(extract_metadata_json("no metadata here").is_none());
    }

    #[test]
    fn extract_metadata_invalid_json() {
        let text = "<!-- not json -->";
        assert!(extract_metadata_json(text).is_none());
    }

    #[test]
    fn extract_metadata_uses_last_comment() {
        let text = r#"<!-- {"old": true} --> text <!-- {"new": true} -->"#;
        let val = extract_metadata_json(text).unwrap();
        assert_eq!(val["new"], true);
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
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
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
