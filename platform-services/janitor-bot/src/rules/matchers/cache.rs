use std::any::Any;
use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;

use forgejo_api::structs::PullRequest;

use crate::clients::Clients;
use crate::event::{BotEvent, PrEvent};
use crate::forgejo::PrCombinedStatus;

use super::{LeafMatcher, Resource, parse_pr_metadata};

pub struct ResourceCache {
    pub(super) matcher_results: moka::sync::Cache<LeafMatcher, bool>,
    values: moka::sync::Cache<String, Arc<dyn Any + Send + Sync>>,
}

impl Default for ResourceCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceCache {
    pub fn new() -> Self {
        Self {
            matcher_results: moka::sync::Cache::builder().build(),
            values: moka::sync::Cache::builder().build(),
        }
    }

    pub async fn get_or_compute<T, F, Fut>(&self, key: &str, compute: F) -> T
    where
        T: Clone + Send + Sync + 'static,
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        if let Some(v) = self.values.get(key)
            && let Some(t) = v.downcast_ref::<T>()
        {
            return t.clone();
        }

        let computed = compute().await;
        self.values
            .insert(key.to_owned(), Arc::new(computed.clone()));
        computed
    }

    #[tracing::instrument(skip_all, name = "cache.prefetch", fields(resources = ?resources))]
    pub async fn prefetch(
        &self,
        clients: &Clients,
        event: &BotEvent<'_>,
        resources: &HashSet<Resource>,
    ) {
        let BotEvent::ForgejoPr(pr) = event else {
            return;
        };

        let need_pr = resources.contains(&Resource::PullRequest)
            || resources.contains(&Resource::CombinedStatus);
        let need_reviews = resources.contains(&Resource::Reviews);
        let need_open_prs = resources.contains(&Resource::OpenPrs);
        let need_changed_files = resources.contains(&Resource::PullRequestChangedFiles);

        if need_changed_files {
            get_changed_files_cached(clients, self, pr).await;
        }
        if need_pr {
            get_pr_cached(clients, self, pr).await;
        }
        if need_reviews {
            get_reviews_cached(clients, self, pr).await;
        }
        if need_open_prs {
            fetch_open_prs(clients, self, pr).await;
        }

        if resources.contains(&Resource::CombinedStatus) {
            combined_status_cached(clients, self, pr).await;
        }
    }
}

pub(crate) async fn get_changed_files_cached(
    clients: &Clients,
    cache: &ResourceCache,
    pr: &PrEvent,
) -> Vec<String> {
    let key = format!("changed_files:{}/{}:{}", pr.owner, pr.repo, pr.pr_number);

    cache
        .get_or_compute(&key, || async {
            match clients
                .forgejo
                .get_pr_changed_files(&pr.owner, &pr.repo, pr.pr_number as i64)
                .await
            {
                Ok(files) => files,
                Err(e) => {
                    tracing::warn!(pr = pr.pr_number, "failed to fetch changed files: {e}");
                    Vec::new()
                }
            }
        })
        .await
}

pub(super) async fn get_pr_cached(
    clients: &Clients,
    cache: &ResourceCache,
    pr: &PrEvent,
) -> Option<PullRequest> {
    let key = format!("pr:{}/{}:{}", pr.owner, pr.repo, pr.pr_number);

    cache
        .get_or_compute(&key, || async {
            match clients
                .forgejo
                .get_pr(&pr.owner, &pr.repo, pr.pr_number as i64)
                .await
            {
                Ok(p) => Some(p),
                Err(e) => {
                    tracing::warn!(pr = pr.pr_number, "failed to fetch PR: {e}");
                    None
                }
            }
        })
        .await
}

pub(super) async fn get_reviews_cached(
    clients: &Clients,
    cache: &ResourceCache,
    pr: &PrEvent,
) -> bool {
    let key = format!("reviews:{}/{}:{}", pr.owner, pr.repo, pr.pr_number);

    cache
        .get_or_compute(&key, || async {
            clients
                .forgejo
                .is_pr_approved_by_bot(&pr.owner, &pr.repo, pr.pr_number as i64)
                .await
        })
        .await
}

pub(super) async fn combined_status_cached(
    clients: &Clients,
    cache: &ResourceCache,
    pr: &PrEvent,
) -> Option<PrCombinedStatus> {
    let key = format!("combined_status:{}/{}:{}", pr.owner, pr.repo, pr.pr_number);

    cache
        .get_or_compute(&key, || async {
            let pr_data = get_pr_cached(clients, cache, pr).await?;
            let sha = pr_data.head.and_then(|h| h.sha)?;

            clients
                .forgejo
                .get_combined_status_by_ref(&pr.owner, &pr.repo, &sha)
                .await
        })
        .await
}

async fn fetch_open_prs(clients: &Clients, cache: &ResourceCache, pr: &PrEvent) {
    let key = format!("open_prs:{}/{}", pr.owner, pr.repo);

    let _: Option<Vec<PullRequest>> = cache
        .get_or_compute(&key, || async {
            match clients.forgejo.list_open_prs(&pr.owner, &pr.repo).await {
                Ok(prs) => Some(prs),
                Err(e) => {
                    tracing::warn!("failed to list open PRs: {e}");
                    None
                }
            }
        })
        .await;
}

pub(super) async fn is_latest_by_metadata(
    clients: &Clients,
    cache: &ResourceCache,
    pr: &PrEvent,
    fields: &[String],
) -> bool {
    let Some(current) = get_pr_cached(clients, cache, pr).await else {
        return true;
    };

    let current_body = current.body.as_deref().unwrap_or("");

    let Some(current_meta) = parse_pr_metadata(current_body) else {
        tracing::debug!(pr = pr.pr_number, "no metadata in current PR");
        return true;
    };

    let current_field_values: Vec<_> = fields
        .iter()
        .filter_map(|f| {
            current_meta
                .get(f)
                .and_then(|v| v.as_str())
                .map(|s| (f.as_str(), s.to_owned()))
        })
        .collect();

    if current_field_values.len() != fields.len() {
        tracing::debug!(pr = pr.pr_number, "missing metadata fields");
        return true;
    }

    let current_created = current.created_at;

    // List all open PRs (cached)
    let key = format!("open_prs:{}/{}", pr.owner, pr.repo);

    let open_prs: Option<Vec<PullRequest>> = cache
        .get_or_compute(&key, || async {
            match clients.forgejo.list_open_prs(&pr.owner, &pr.repo).await {
                Ok(prs) => Some(prs),
                Err(e) => {
                    tracing::warn!("failed to list open PRs: {e}");
                    None
                }
            }
        })
        .await;

    let Some(open_prs) = open_prs else {
        return true;
    };

    for other in &open_prs {
        let other_number = other.number.unwrap_or(0) as u64;
        if other_number == pr.pr_number {
            continue;
        }

        let other_author = other
            .user
            .as_ref()
            .and_then(|u| u.login.as_deref())
            .unwrap_or("");

        if other_author != pr.author {
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

        if other.created_at > current_created {
            return false;
        }
    }

    true
}
