use forgejo_api::structs::*;
use forgejo_api::{Auth, Forgejo};
use url::Url;

pub struct ForgejoClient {
    api: Forgejo,
}

impl ForgejoClient {
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
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
}
