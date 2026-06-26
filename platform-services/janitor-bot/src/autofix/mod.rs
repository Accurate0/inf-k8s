mod client;
mod workspace;

pub use client::{DEFAULT_MODEL, FileEdit, LlmAutofixClient, PrMeta};
pub use workspace::Workspace;

use crate::clients::Clients;
use crate::git;
use crate::marker::Marker;
use forgejo_api::structs::CommitStatusState;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

const AUTOFIX_LABEL: &str = "janitor/autofix";
static AUTOFIX_MARKER: LazyLock<Marker> = LazyLock::new(|| Marker::feature("autofix"));

/// Runs the LLM autofix flow for a failing renovate PR: clones the head branch,
/// lets the model explore it + the failing CI logs via tools, applies the
/// returned edits, and opens a PR targeting the original renovate branch.
#[tracing::instrument(skip(clients, model), fields(%owner, %repo, pr))]
pub async fn autofix_pr(
    clients: &Clients,
    owner: &str,
    repo: &str,
    pr: i64,
    model: Option<String>,
) {
    tracing::info!("autofix: starting");
    let client = &clients.forgejo;

    let Some(llm) = clients.llm.as_ref() else {
        let _ = client
            .comment(owner, repo, pr, "Autofix is not configured")
            .await;
        return;
    };

    let api_pr = match client.get_pr(owner, repo, pr).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("autofix: failed to fetch PR: {e}");
            return;
        }
    };

    let author = api_pr
        .user
        .as_ref()
        .and_then(|u| u.login.as_deref())
        .unwrap_or_default();
    if !author.to_lowercase().contains("renovate") {
        let _ = client
            .comment(owner, repo, pr, "Autofix only runs on renovate PRs.")
            .await;
        return;
    }

    let head_branch = api_pr.head.as_ref().and_then(|h| h.r#ref.clone());
    let base_branch = api_pr.base.as_ref().and_then(|b| b.r#ref.clone());
    let (Some(head_branch), Some(base_branch)) = (head_branch, base_branch) else {
        tracing::warn!(pr, "autofix: PR missing head/base branch info");
        return;
    };

    let title = api_pr.title.clone().unwrap_or_default();
    let head_sha = api_pr.head.as_ref().and_then(|h| h.sha.clone());
    let Some(head_sha) = head_sha else {
        tracing::warn!(pr, "autofix: PR missing head sha");
        return;
    };

    let Some(failure_logs) = collect_failure_logs(clients, owner, repo, &head_sha).await else {
        tracing::info!("autofix: no failing checks to fix");
        let _ = client
            .comment(owner, repo, pr, "No failing checks to fix.")
            .await;
        return;
    };
    tracing::debug!(%head_branch, %head_sha, "autofix: collected failure logs ({} bytes)", failure_logs.len());

    tracing::info!(%head_branch, "autofix: cloning head branch");
    let clone_url = client.clone_url(owner, repo);
    let token = client.token.clone();

    let tmp = {
        let clone_url = clone_url.clone();
        let token = token.clone();
        let branch = head_branch.clone();
        match tokio::task::spawn_blocking(move || git::clone_branch(&clone_url, &token, &branch))
            .await
            .unwrap_or_else(|e| Err(anyhow::anyhow!("join error: {e}")))
        {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("autofix: clone failed: {e}");
                let _ = client
                    .comment(
                        owner,
                        repo,
                        pr,
                        "Autofix failed while preparing the workspace.",
                    )
                    .await;
                return;
            }
        }
    };

    let workspace = Workspace::new(tmp.path());
    let model = model.unwrap_or_else(|| DEFAULT_MODEL.to_owned());
    let meta = PrMeta {
        pr_title: title,
        base_branch,
        head_branch: head_branch.clone(),
    };

    tracing::info!(%model, "autofix: requesting fix from model");
    let fixer = LlmAutofixClient::new(llm);
    let edits = match fixer
        .propose_fix(&workspace, &failure_logs, &meta, &model)
        .await
    {
        Ok(e) if !e.is_empty() => {
            tracing::info!("autofix: model proposed {} file edit(s)", e.len());
            tracing::debug!(paths = ?e.iter().map(|f| &f.path).collect::<Vec<_>>(), "autofix: proposed edits");
            e
        }
        Ok(_) => {
            tracing::info!("autofix: model could not determine a fix");
            let _ = client
                .comment(owner, repo, pr, "The model could not determine a fix.")
                .await;
            return;
        }
        Err(e) => {
            tracing::error!("autofix: LLM call failed: {e}");
            let _ = client
                .comment(owner, repo, pr, "Autofix failed while generating a fix.")
                .await;
            return;
        }
    };

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let fix_branch = format!("janitor/autofix-{pr}-{timestamp}");
    let commit_msg = format!("fix: autofix CI for #{pr}");
    tracing::info!(%fix_branch, "autofix: committing and pushing fix");
    let branch = fix_branch.clone();
    let edits_for_git = edits.clone();

    if let Err(e) = tokio::task::spawn_blocking(move || {
        let repo_path = tmp.path().to_owned();
        git::commit_and_push(&repo_path, &token, &branch, &commit_msg, &edits_for_git)
    })
    .await
    .unwrap_or_else(|e| Err(anyhow::anyhow!("join error: {e}")))
    {
        tracing::error!("autofix: push failed: {e}");
        let _ = client
            .comment(
                owner,
                repo,
                pr,
                "Autofix failed while pushing the proposed fix.",
            )
            .await;
        return;
    }

    let rows: String = edits
        .iter()
        .map(|e| format!("| `{}` |\n", e.path))
        .collect();
    let body = format!(
        "{}\nAutomated fix for #{pr} produced by `{model}`.\n\n\
         Merging this PR updates the renovate branch `{head_branch}`.\n\n\
         | Files changed |\n| --- |\n{rows}",
        *AUTOFIX_MARKER
    );

    let new_pr = match client
        .create_pull_request(
            owner,
            repo,
            &format!("LLM autofix for #{pr}"),
            &body,
            &fix_branch,
            &head_branch,
        )
        .await
    {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("autofix: failed to open PR: {e}");
            return;
        }
    };

    let new_number = new_pr.number.unwrap_or(0);
    if let Err(e) = client
        .ensure_labels(
            owner,
            repo,
            vec![(AUTOFIX_LABEL.to_owned(), "#1f6feb".to_owned())],
        )
        .await
    {
        tracing::warn!("autofix: ensure label failed: {e}");
    } else if let Err(e) = client
        .add_labels_by_name(owner, repo, new_number, vec![AUTOFIX_LABEL.to_owned()])
        .await
    {
        tracing::warn!("autofix: add label failed: {e}");
    }

    tracing::info!(new_pr = new_number, "autofix: opened fix PR");
    let comment = format!(
        "{}\nOpened autofix PR #{new_number} with a proposed fix from `{model}`.",
        *AUTOFIX_MARKER
    );
    if let Err(e) = client
        .comment_or_update(owner, repo, pr, &AUTOFIX_MARKER, &comment)
        .await
    {
        tracing::error!("autofix: comment failed: {e}");
    }
}

/// Builds the failing-check log text. For checks reported by GitHub Actions
/// (context prefixed `GitHub Actions`), pulls the failed-job logs from the
/// mirror; otherwise falls back to the status description.
async fn collect_failure_logs(
    clients: &Clients,
    owner: &str,
    repo: &str,
    sha: &str,
) -> Option<String> {
    let status = clients
        .forgejo
        .get_combined_status_by_ref(owner, repo, sha)
        .await?;

    let mut out = String::new();
    for entry in &status.statuses {
        if !matches!(
            entry.state,
            CommitStatusState::Failure | CommitStatusState::Error
        ) {
            continue;
        }

        out.push_str(&format!("### {}\n", entry.context));

        let ci_logs = if entry.context.starts_with("GitHub Actions") {
            github_logs_from_url(clients, &entry.target_url).await
        } else if is_forgejo_action_url(&entry.target_url) {
            clients.forgejo.get_action_logs(&entry.target_url).await
        } else {
            None
        };

        match ci_logs {
            Some(logs) if !logs.is_empty() => {
                // keep the last 200 lines, in order — the error is usually at the end
                let lines: Vec<&str> = logs.lines().collect();
                let start = lines.len().saturating_sub(200);
                out.push_str(&lines[start..].join("\n"));
                out.push('\n');
            }
            _ => {
                if !entry.description.is_empty() {
                    out.push_str(&entry.description);
                    out.push('\n');
                }
            }
        }
        out.push('\n');
    }

    let out = out.trim();
    if out.is_empty() {
        None
    } else {
        Some(out.to_owned())
    }
}

/// Fetches a GitHub Actions run's failed-job logs from the mirror, given the
/// status `target_url`.
async fn github_logs_from_url(clients: &Clients, url: &str) -> Option<String> {
    let (owner, repo, run_id) = parse_github_run_url(url)?;
    let result = clients
        .github
        .failed_logs_for_run(owner, repo, run_id)
        .await;
    Some(result.logs)
}

/// Parses `https://github.com/<owner>/<repo>/actions/runs/<id>/...` into its
/// `(owner, repo, run_id)` parts.
fn parse_github_run_url(url: &str) -> Option<(&str, &str, u64)> {
    let rest = url.split("github.com/").nth(1)?;
    let mut parts = rest.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if parts.next()? != "actions" || parts.next()? != "runs" {
        return None;
    }
    let run_id: u64 = parts.next()?.parse().ok()?;
    Some((owner, repo, run_id))
}

/// Whether `url` points at an Actions run on our own Forgejo instance, i.e. a
/// commit status reported by Forgejo Actions (`.forgejo/workflows`) rather than
/// the GitHub mirror. Forgejo reports these as site-relative paths
/// (`/owner/repo/actions/runs/...`); the GitHub mirror uses absolute URLs.
fn is_forgejo_action_url(url: &str) -> bool {
    url.starts_with('/') && url.contains("/actions/runs/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_run_url() {
        let (owner, repo, run_id) = parse_github_run_url(
            "https://github.com/Accurate0/inf-k8s/actions/runs/123456/job/789",
        )
        .unwrap();
        assert_eq!(owner, "Accurate0");
        assert_eq!(repo, "inf-k8s");
        assert_eq!(run_id, 123456);
    }

    #[test]
    fn rejects_non_actions_url() {
        assert!(parse_github_run_url("https://git.anurag.sh/anurag/k8s/pulls/5").is_none());
    }

    #[test]
    fn detects_forgejo_action_url() {
        // Forgejo Actions commit statuses carry site-relative target URLs
        assert!(is_forgejo_action_url("/anurag/k8s/actions/runs/561/jobs/0"));
        // GitHub mirror runs are absolute URLs on github.com
        assert!(!is_forgejo_action_url(
            "https://github.com/Accurate0/inf-k8s/actions/runs/456"
        ));
        // non-actions Forgejo paths (e.g. argocd status) don't carry logs
        assert!(!is_forgejo_action_url("/anurag/k8s/pulls/5"));
        assert!(!is_forgejo_action_url(""));
    }
}
