use crate::marker::Marker;
use forgejo_api::structs::*;
use forgejo_api::{Auth, Forgejo};
use std::collections::HashSet;
use url::Url;

pub(crate) const BOT_USERNAME: &str = "janitor";

#[derive(Debug, Clone)]
pub struct PrCombinedStatus {
    pub state: CommitStatusState,
    pub total_count: i64,
}

pub struct BotComment {
    pub id: i64,
    pub body: String,
}

pub struct CommitStatusParams<'a> {
    pub owner: &'a str,
    pub repo: &'a str,
    pub sha: &'a str,
    pub state: &'a str,
    pub context: &'a str,
    pub description: &'a str,
    pub target_url: &'a str,
}

pub struct ForgejoClient {
    api: Forgejo,
    pub base_url: String,
    pub token: String,
}

impl ForgejoClient {
    pub fn from_env() -> anyhow::Result<Self> {
        let base_url = std::env::var("FORGEJO_INSTANCE_URL")?;
        let token = std::env::var("FORGEJO_ACCESS_KEY")?;
        Self::new(base_url, token)
    }

    pub fn new(base_url: String, token: String) -> anyhow::Result<Self> {
        let url = Url::parse(&base_url)?;
        let api = Forgejo::new(Auth::Token(&token), url)?;

        Ok(Self {
            api,
            base_url,
            token,
        })
    }

    pub fn clone_url(&self, owner: &str, repo: &str) -> String {
        format!("{}/{}/{}.git", self.base_url, owner, repo)
    }

    pub async fn is_pr_approved_by_bot(&self, owner: &str, repo: &str, pr: i64) -> bool {
        let reviews = match self
            .api
            .repo_list_pull_reviews(owner, repo, pr)
            .send()
            .await
        {
            Ok((_, reviews)) => reviews,
            Err(e) => {
                tracing::warn!(pr, "failed to list reviews: {e}");
                return false;
            }
        };

        reviews.iter().any(|r| {
            r.user.as_ref().and_then(|u| u.login.as_deref()) == Some(BOT_USERNAME)
                && r.state.as_deref() == Some("APPROVED")
                && !r.dismissed.unwrap_or(false)
        })
    }

    pub async fn approve_pr(
        &self,
        owner: &str,
        repo: &str,
        pr: i64,
        body: Option<&str>,
    ) -> Result<(), forgejo_api::ForgejoError> {
        self.api
            .repo_create_pull_review(
                owner,
                repo,
                pr,
                CreatePullReviewOptions {
                    event: Some("APPROVED".into()),
                    body: body.map(|b| b.into()),
                    comments: None,
                    commit_id: None,
                },
            )
            .send()
            .await?;
        tracing::info!(pr, owner, repo, "approved");
        Ok(())
    }

    pub async fn merge_pr(
        &self,
        owner: &str,
        repo: &str,
        pr: i64,
        strategy: MergePullRequestOptionDo,
        delete_branch: bool,
    ) -> Result<(), forgejo_api::ForgejoError> {
        self.api
            .repo_merge_pull_request(
                owner,
                repo,
                pr,
                MergePullRequestOption {
                    r#do: strategy,
                    delete_branch_after_merge: Some(delete_branch),
                    merge_commit_id: None,
                    merge_message_field: None,
                    merge_title_field: None,
                    force_merge: None,
                    head_commit_id: None,
                    merge_when_checks_succeed: None,
                },
            )
            .send()
            .await?;
        tracing::info!(pr, owner, repo, "merged");
        Ok(())
    }

    pub async fn comment(
        &self,
        owner: &str,
        repo: &str,
        pr: i64,
        body: &str,
    ) -> Result<(), forgejo_api::ForgejoError> {
        self.api
            .issue_create_comment(
                owner,
                repo,
                pr,
                CreateIssueCommentOption {
                    body: body.into(),
                    updated_at: None,
                },
            )
            .send()
            .await?;
        tracing::info!(pr, owner, repo, "commented");
        Ok(())
    }

    pub async fn add_labels(
        &self,
        owner: &str,
        repo: &str,
        pr: i64,
        label_ids: Vec<i64>,
    ) -> Result<(), forgejo_api::ForgejoError> {
        let labels: Vec<serde_json::Value> =
            label_ids.into_iter().map(serde_json::Value::from).collect();
        self.api
            .issue_add_label(
                owner,
                repo,
                pr,
                IssueLabelsOption {
                    labels: Some(labels),
                    updated_at: None,
                },
            )
            .send()
            .await?;
        tracing::info!(pr, owner, repo, "labels added");
        Ok(())
    }

    pub async fn add_labels_by_name(
        &self,
        owner: &str,
        repo: &str,
        pr: i64,
        labels: Vec<String>,
    ) -> Result<(), forgejo_api::ForgejoError> {
        let labels: Vec<serde_json::Value> =
            labels.into_iter().map(serde_json::Value::String).collect();
        self.api
            .issue_add_label(
                owner,
                repo,
                pr,
                IssueLabelsOption {
                    labels: Some(labels),
                    updated_at: None,
                },
            )
            .send()
            .await?;
        tracing::info!(pr, owner, repo, "labels added by name");
        Ok(())
    }

    pub async fn list_open_prs(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<Vec<PullRequest>, forgejo_api::ForgejoError> {
        let (_, prs) = self
            .api
            .repo_list_pull_requests(
                owner,
                repo,
                RepoListPullRequestsQuery {
                    state: Some(RepoListPullRequestsQueryState::Open),
                    ..Default::default()
                },
            )
            .send()
            .await?;
        Ok(prs)
    }

    pub async fn health_check(&self) -> Result<(), forgejo_api::ForgejoError> {
        self.api.user_get_current().send().await?;
        Ok(())
    }

    pub async fn is_pr_mergeable(&self, owner: &str, repo: &str, pr: i64) -> Option<bool> {
        match self.api.repo_get_pull_request(owner, repo, pr).send().await {
            Ok(pr) => pr.mergeable,
            Err(e) => {
                tracing::warn!(pr, "failed to fetch PR for mergeable check: {e}");
                None
            }
        }
    }

    pub async fn remove_labels_by_name(
        &self,
        owner: &str,
        repo: &str,
        pr: i64,
        labels: Vec<String>,
    ) -> Result<(), forgejo_api::ForgejoError> {
        let (_, all_labels) = self
            .api
            .issue_list_labels(owner, repo, IssueListLabelsQuery { sort: None })
            .send()
            .await?;
        for name in &labels {
            let Some(id) = all_labels
                .iter()
                .find(|l| l.name.as_deref() == Some(name.as_str()))
                .and_then(|l| l.id)
            else {
                continue;
            };

            let id_str = id.to_string();
            if let Err(e) = self
                .api
                .issue_remove_label(
                    owner,
                    repo,
                    pr,
                    &id_str,
                    DeleteLabelsOption { updated_at: None },
                )
                .send()
                .await
            {
                tracing::warn!(pr, label = name, "failed to remove label: {e}");
            } else {
                tracing::info!(pr, owner, repo, label = name, "label removed");
            }
        }
        Ok(())
    }

    pub async fn get_pr(
        &self,
        owner: &str,
        repo: &str,
        pr: i64,
    ) -> Result<PullRequest, forgejo_api::ForgejoError> {
        self.api.repo_get_pull_request(owner, repo, pr).send().await
    }

    pub async fn is_pr_open(&self, owner: &str, repo: &str, pr: i64) -> bool {
        match self.api.repo_get_pull_request(owner, repo, pr).send().await {
            Ok(pr) => matches!(pr.state, Some(StateType::Open)) && !pr.merged.unwrap_or(false),
            Err(_) => false,
        }
    }

    pub async fn get_commit_changed_files(
        &self,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<Vec<String>, forgejo_api::ForgejoError> {
        let query = RepoGetSingleCommitQuery {
            stat: Some(false),
            verification: Some(false),
            files: Some(true),
        };
        let commit = self
            .api
            .repo_get_single_commit(owner, repo, sha, query)
            .send()
            .await?;
        Ok(commit
            .files
            .unwrap_or_default()
            .into_iter()
            .filter_map(|f| f.filename)
            .collect())
    }

    pub async fn get_pr_changed_files(
        &self,
        owner: &str,
        repo: &str,
        pr: i64,
    ) -> Result<Vec<String>, forgejo_api::ForgejoError> {
        let (_, files) = self
            .api
            .repo_get_pull_request_files(owner, repo, pr, RepoGetPullRequestFilesQuery::default())
            .send()
            .await?;
        Ok(files.into_iter().filter_map(|f| f.filename).collect())
    }

    pub async fn find_open_issue_by_title(
        &self,
        owner: &str,
        repo: &str,
        title: &str,
    ) -> Result<Option<Issue>, forgejo_api::ForgejoError> {
        let (_, issues) = self
            .api
            .issue_list_issues(
                owner,
                repo,
                IssueListIssuesQuery {
                    state: Some(IssueListIssuesQueryState::Open),
                    q: Some(title.to_owned()),
                    r#type: Some(IssueListIssuesQueryType::Issues),
                    ..Default::default()
                },
            )
            .send()
            .await?;
        Ok(issues
            .into_iter()
            .find(|i| i.title.as_deref() == Some(title)))
    }

    pub async fn create_issue_with_labels(
        &self,
        owner: &str,
        repo: &str,
        title: &str,
        body: &str,
        label_names: &[String],
    ) -> Result<Issue, forgejo_api::ForgejoError> {
        let label_ids = if label_names.is_empty() {
            None
        } else {
            let (_, all_labels) = self
                .api
                .issue_list_labels(owner, repo, IssueListLabelsQuery { sort: None })
                .send()
                .await?;
            let ids: Vec<i64> = all_labels
                .iter()
                .filter(|l| {
                    l.name
                        .as_deref()
                        .is_some_and(|n| label_names.iter().any(|ln| ln == n))
                })
                .filter_map(|l| l.id)
                .collect();
            Some(ids)
        };

        let issue = self
            .api
            .issue_create_issue(
                owner,
                repo,
                CreateIssueOption {
                    title: title.to_owned(),
                    body: Some(body.to_owned()),
                    assignee: None,
                    assignees: None,
                    closed: None,
                    due_date: None,
                    labels: label_ids,
                    milestone: None,
                    r#ref: None,
                },
            )
            .send()
            .await?;
        tracing::info!(owner, repo, issue = issue.number, "created issue");
        Ok(issue)
    }

    pub async fn comment_on_issue(
        &self,
        owner: &str,
        repo: &str,
        index: i64,
        body: &str,
    ) -> Result<(), forgejo_api::ForgejoError> {
        self.api
            .issue_create_comment(
                owner,
                repo,
                index,
                CreateIssueCommentOption {
                    body: body.to_owned(),
                    updated_at: None,
                },
            )
            .send()
            .await?;
        tracing::info!(owner, repo, issue = index, "commented on issue");
        Ok(())
    }

    pub async fn delete_branch(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<(), forgejo_api::ForgejoError> {
        self.api
            .repo_delete_branch(owner, repo, branch)
            .send()
            .await?;
        tracing::info!(owner, repo, branch, "branch deleted");
        Ok(())
    }

    pub async fn set_pr_state(
        &self,
        owner: &str,
        repo: &str,
        pr: i64,
        state: &str,
    ) -> Result<(), forgejo_api::ForgejoError> {
        self.api
            .issue_edit_issue(
                owner,
                repo,
                pr,
                EditIssueOption {
                    state: Some(state.to_owned()),
                    assignee: None,
                    assignees: None,
                    body: None,
                    due_date: None,
                    milestone: None,
                    r#ref: None,
                    title: None,
                    unset_due_date: None,
                    updated_at: None,
                },
            )
            .send()
            .await?;
        tracing::info!(pr, owner, repo, state, "PR state changed");
        Ok(())
    }

    pub async fn close_issue(
        &self,
        owner: &str,
        repo: &str,
        index: i64,
    ) -> Result<(), forgejo_api::ForgejoError> {
        self.api
            .issue_edit_issue(
                owner,
                repo,
                index,
                EditIssueOption {
                    state: Some("closed".into()),
                    assignee: None,
                    assignees: None,
                    body: None,
                    due_date: None,
                    milestone: None,
                    r#ref: None,
                    title: None,
                    unset_due_date: None,
                    updated_at: None,
                },
            )
            .send()
            .await?;
        tracing::info!(owner, repo, issue = index, "closed issue");
        Ok(())
    }

    pub async fn ensure_labels(
        &self,
        owner: &str,
        repo: &str,
        labels: Vec<(String, String)>,
    ) -> Result<(), forgejo_api::ForgejoError> {
        let (_, existing) = self
            .api
            .issue_list_labels(owner, repo, IssueListLabelsQuery { sort: None })
            .send()
            .await?;
        let existing_names: HashSet<String> =
            existing.iter().filter_map(|l| l.name.clone()).collect();

        for (name, color) in labels {
            if !existing_names.contains(&name) {
                self.api
                    .issue_create_label(
                        owner,
                        repo,
                        CreateLabelOption {
                            name: name.clone(),
                            color,
                            description: None,
                            exclusive: None,
                            is_archived: None,
                        },
                    )
                    .send()
                    .await?;
                tracing::info!(owner, repo, label = name, "created label");
            }
        }
        Ok(())
    }

    pub async fn react_to_comment(
        &self,
        owner: &str,
        repo: &str,
        comment_id: i64,
        reaction: &str,
    ) -> Result<(), forgejo_api::ForgejoError> {
        self.api
            .issue_post_comment_reaction(
                owner,
                repo,
                comment_id,
                EditReactionOption {
                    content: Some(reaction.to_owned()),
                },
            )
            .send()
            .await?;
        tracing::info!(owner, repo, comment_id, reaction, "reacted to comment");
        Ok(())
    }

    pub async fn get_raw_file(
        &self,
        owner: &str,
        repo: &str,
        filepath: &str,
        git_ref: &str,
    ) -> anyhow::Result<String> {
        let url = format!(
            "{}/api/v1/repos/{}/{}/raw/{}?ref={}",
            self.base_url, owner, repo, filepath, git_ref
        );
        let resp = reqwest::Client::new()
            .get(&url)
            .header("Authorization", format!("token {}", self.token))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        Ok(resp)
    }

    pub async fn get_pr_combined_status(
        &self,
        owner: &str,
        repo: &str,
        pr: i64,
    ) -> Option<PrCombinedStatus> {
        let pr_data = self
            .api
            .repo_get_pull_request(owner, repo, pr)
            .send()
            .await
            .ok()?;

        let sha = pr_data.head.and_then(|h| h.sha)?;
        let (_, combined) = self
            .api
            .repo_get_combined_status_by_ref(owner, repo, &sha)
            .send()
            .await
            .ok()?;

        Some(PrCombinedStatus {
            state: combined.state?,
            total_count: combined.total_count.unwrap_or(0),
        })
    }

    pub async fn get_combined_status_by_ref(
        &self,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Option<PrCombinedStatus> {
        let (_, combined) = self
            .api
            .repo_get_combined_status_by_ref(owner, repo, sha)
            .send()
            .await
            .ok()?;

        Some(PrCombinedStatus {
            state: combined.state?,
            total_count: combined.total_count.unwrap_or(0),
        })
    }

    pub async fn get_pr_head_ref(
        &self,
        owner: &str,
        repo: &str,
        pr: i64,
    ) -> anyhow::Result<String> {
        let pr_data = self
            .api
            .repo_get_pull_request(owner, repo, pr)
            .send()
            .await?;
        pr_data
            .head
            .and_then(|h| h.r#ref)
            .ok_or_else(|| anyhow::anyhow!("PR head ref not found"))
    }

    /// Returns whether the bot has already left a comment carrying `marker` on
    /// this issue/PR — the stateless "have I already acted?" check.
    pub async fn has_acted(
        &self,
        owner: &str,
        repo: &str,
        issue: i64,
        marker: &Marker,
    ) -> anyhow::Result<bool> {
        Ok(self
            .find_bot_comment_with_marker(owner, repo, issue, &marker.to_string())
            .await?
            .is_some())
    }

    async fn fetch_bot_comments_with_marker(
        &self,
        owner: &str,
        repo: &str,
        issue: i64,
        marker: &str,
    ) -> anyhow::Result<Option<BotComment>> {
        let url = format!(
            "{}/api/v1/repos/{}/{}/issues/{}/comments",
            self.base_url, owner, repo, issue
        );
        let resp: Vec<serde_json::Value> = reqwest::Client::new()
            .get(&url)
            .header("Authorization", format!("token {}", self.token))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        for comment in resp {
            let body = comment["body"].as_str().unwrap_or("");
            let user = comment["user"]["login"].as_str().unwrap_or("");
            if user == BOT_USERNAME
                && body.contains(marker)
                && let Some(id) = comment["id"].as_i64()
            {
                return Ok(Some(BotComment {
                    id,
                    body: body.to_string(),
                }));
            }
        }
        Ok(None)
    }

    pub async fn find_bot_comment_with_marker(
        &self,
        owner: &str,
        repo: &str,
        issue: i64,
        marker: &str,
    ) -> anyhow::Result<Option<i64>> {
        Ok(self
            .fetch_bot_comments_with_marker(owner, repo, issue, marker)
            .await?
            .map(|c| c.id))
    }

    pub async fn find_bot_comment_with_marker_and_body(
        &self,
        owner: &str,
        repo: &str,
        issue: i64,
        marker: &str,
    ) -> anyhow::Result<Option<BotComment>> {
        self.fetch_bot_comments_with_marker(owner, repo, issue, marker)
            .await
    }

    pub async fn update_comment(
        &self,
        owner: &str,
        repo: &str,
        comment_id: i64,
        body: &str,
    ) -> Result<(), forgejo_api::ForgejoError> {
        self.api
            .issue_edit_comment(
                owner,
                repo,
                comment_id,
                EditIssueCommentOption {
                    body: body.to_owned(),
                    updated_at: None,
                },
            )
            .send()
            .await?;
        tracing::info!(owner, repo, comment_id, "updated comment");
        Ok(())
    }

    pub async fn comment_or_update(
        &self,
        owner: &str,
        repo: &str,
        pr: i64,
        marker: &Marker,
        body: &str,
    ) -> anyhow::Result<()> {
        match self
            .find_bot_comment_with_marker(owner, repo, pr, &marker.to_string())
            .await?
        {
            Some(comment_id) => {
                self.update_comment(owner, repo, comment_id, body).await?;
            }
            None => {
                self.comment(owner, repo, pr, body).await?;
            }
        }
        Ok(())
    }

    pub async fn set_commit_status(
        &self,
        params: CommitStatusParams<'_>,
    ) -> Result<(), forgejo_api::ForgejoError> {
        let commit_state = match params.state {
            "success" => CommitStatusState::Success,
            "failure" => CommitStatusState::Failure,
            "error" => CommitStatusState::Error,
            "pending" => CommitStatusState::Pending,
            _ => CommitStatusState::Warning,
        };
        let parsed_url = url::Url::parse(params.target_url).ok();

        self.api
            .repo_create_status(
                params.owner,
                params.repo,
                params.sha,
                CreateStatusOption {
                    state: Some(commit_state),
                    context: Some(params.context.to_owned()),
                    description: Some(params.description.to_owned()),
                    target_url: parsed_url,
                },
            )
            .send()
            .await?;
        tracing::info!(
            owner = params.owner,
            repo = params.repo,
            sha = params.sha,
            state = params.state,
            context = params.context,
            "set commit status"
        );
        Ok(())
    }

    pub async fn create_pull_request(
        &self,
        owner: &str,
        repo: &str,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
    ) -> Result<PullRequest, forgejo_api::ForgejoError> {
        let pr = self
            .api
            .repo_create_pull_request(
                owner,
                repo,
                CreatePullRequestOption {
                    title: Some(title.to_owned()),
                    body: Some(body.to_owned()),
                    head: Some(head.to_owned()),
                    base: Some(base.to_owned()),
                    assignee: None,
                    assignees: None,
                    due_date: None,
                    labels: None,
                    milestone: None,
                },
            )
            .send()
            .await?;
        tracing::info!(
            owner,
            repo,
            pr = pr.number,
            head,
            base,
            "created pull request"
        );
        Ok(pr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_deserialization_with_files() {
        let json = r#"{"sha":"abc1234567890def","created":"2025-01-01T00:00:00Z","html_url":"","url":"","files":[{"filename":"system-components/janitor-bot.application.yaml"},{"filename":"platform-services/janitor-bot/values.yaml"}]}"#;

        let commit: Commit = serde_json::from_str(json).expect("should deserialize");
        let files: Vec<String> = commit
            .files
            .unwrap_or_default()
            .into_iter()
            .filter_map(|f| f.filename)
            .collect();

        assert_eq!(
            files,
            vec![
                "system-components/janitor-bot.application.yaml",
                "platform-services/janitor-bot/values.yaml"
            ]
        );
    }

    #[test]
    fn comment_deserialization() {
        let json = r#"{"id":1,"body":"test","created_at":"2026-01-07T10:00:00Z","updated_at":"2026-01-07T10:00:00Z","html_url":"","issue_url":"","pull_request_url":"","url":""}"#;
        serde_json::from_str::<Comment>(json).expect("Comment should deserialize");
    }

    #[test]
    fn issue_deserialization_minimal() {
        let json = r#"{"closed_at":null,"created_at":null,"due_date":null,"updated_at":null,"html_url":"","url":""}"#;
        serde_json::from_str::<Issue>(json).expect("Issue should deserialize");
    }

    #[test]
    fn pullrequest_list_deserialization() {
        let json = r#"[{"number":99,"body":"<!-- metadata:{\"service\":\"foo\"} -->","user":{"login":"ci-image-updater","avatar_url":"","html_url":"","created":"2020-01-01T00:00:00Z","last_login":"2020-01-01T00:00:00Z"},"head":{"label":"","ref":"branch","sha":""},"created_at":"2026-01-07T08:00:00Z","closed_at":null,"due_date":null,"merged_at":null,"updated_at":null,"requested_reviewers":[],"diff_url":"","html_url":"","patch_url":"","url":""}]"#;
        let prs: Vec<PullRequest> = serde_json::from_str(json).expect("PR list should deserialize");
        assert_eq!(
            prs[0].body.as_deref(),
            Some("<!-- metadata:{\"service\":\"foo\"} -->")
        );
        assert!(prs[0].created_at.is_some());
    }
}
