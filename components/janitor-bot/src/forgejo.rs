use forgejo_api::structs::*;
use forgejo_api::{Auth, Forgejo};
use url::Url;

pub struct ForgejoClient {
    api: Forgejo,
}

impl ForgejoClient {
    pub fn from_env() -> anyhow::Result<Self> {
        let base_url = std::env::var("FORGEJO_INSTANCE_URL")?;
        let token = std::env::var("FORGEJO_ACCESS_KEY")?;
        let url = Url::parse(&base_url)?;
        let api = Forgejo::new(Auth::Token(&token), url)?;
        Ok(Self { api })
    }

    pub async fn approve_pr(
        &self,
        owner: &str,
        repo: &str,
        pr: i64,
        body: &str,
    ) -> Result<(), forgejo_api::ForgejoError> {
        self.api
            .repo_create_pull_review(
                owner,
                repo,
                pr,
                CreatePullReviewOptions {
                    event: Some("APPROVED".into()),
                    body: Some(body.into()),
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
        let labels: Vec<serde_json::Value> = label_ids
            .into_iter()
            .map(|id| serde_json::Value::from(id))
            .collect();
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

    pub async fn is_pr_merged(&self, owner: &str, repo: &str, pr: i64) -> bool {
        self.api
            .repo_pull_request_is_merged(owner, repo, pr)
            .send()
            .await
            .is_ok()
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
}
