use forgejo_api::structs::*;
use forgejo_api::{Auth, Forgejo};
use url::Url;

pub(crate) const BOT_USERNAME: &str = "janitor";

pub struct ForgejoClient {
    api: Forgejo,
    pub base_url: String,
    pub token: String,
}

impl ForgejoClient {
    pub fn from_env() -> anyhow::Result<Self> {
        let base_url = std::env::var("FORGEJO_INSTANCE_URL")?;
        let token = std::env::var("FORGEJO_ACCESS_KEY")?;
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
        let existing_names: std::collections::HashSet<String> =
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
