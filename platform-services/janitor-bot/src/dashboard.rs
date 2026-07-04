use askama::Template;
use forgejo_api::structs::CommitStatusState;

use crate::clients::Clients;
use crate::rules::RulesOrchestrator;

const RENOVATE_AUTHOR: &str = "renovate";

#[derive(Template)]
#[template(path = "renovate_dashboard.html")]
struct RenovateDashboard {
    generated_at: String,
    repos: Vec<RepoView>,
}

struct RepoView {
    slug: String,
    error: Option<String>,
    prs: Vec<PrView>,
}

struct PrView {
    number: i64,
    title: String,
    html_url: String,
    labels: Vec<String>,
    status: &'static str,
    checks: Vec<CheckView>,
}

struct CheckView {
    context: String,
    state: &'static str,
    target_url: String,
}

fn status_word(state: &CommitStatusState) -> &'static str {
    match state {
        CommitStatusState::Success => "success",
        CommitStatusState::Pending => "pending",
        CommitStatusState::Failure => "failure",
        CommitStatusState::Error => "error",
        CommitStatusState::Warning => "warning",
        CommitStatusState::Skipped => "skipped",
    }
}

async fn build_repo_view(clients: &Clients, owner: &str, repo: &str) -> RepoView {
    let slug = format!("{owner}/{repo}");

    let prs = match clients.forgejo.list_open_prs(owner, repo).await {
        Ok(prs) => prs,
        Err(e) => {
            return RepoView {
                slug,
                error: Some(e.to_string()),
                prs: Vec::new(),
            };
        }
    };

    let mut pr_views = Vec::new();
    for pr in prs {
        let is_renovate = pr.user.as_ref().and_then(|u| u.login.as_deref()) == Some(RENOVATE_AUTHOR);
        if !is_renovate {
            continue;
        }

        let combined = match pr.head.as_ref().and_then(|h| h.sha.as_deref()) {
            Some(sha) => {
                clients
                    .forgejo
                    .get_combined_status_by_ref(owner, repo, sha)
                    .await
            }
            None => None,
        };

        let (status, checks) = match combined {
            Some(cs) => {
                let checks = cs
                    .statuses
                    .iter()
                    .map(|s| CheckView {
                        context: s.context.clone(),
                        state: status_word(&s.state),
                        target_url: s.target_url.clone(),
                    })
                    .collect();
                (status_word(&cs.state), checks)
            }
            None => ("none", Vec::new()),
        };

        pr_views.push(PrView {
            number: pr.number.unwrap_or_default(),
            title: pr.title.clone().unwrap_or_default(),
            html_url: pr.html_url.as_ref().map(|u| u.to_string()).unwrap_or_default(),
            labels: pr
                .labels
                .as_ref()
                .map(|ls| ls.iter().filter_map(|l| l.name.clone()).collect())
                .unwrap_or_default(),
            status,
            checks,
        });
    }

    RepoView {
        slug,
        error: None,
        prs: pr_views,
    }
}

pub async fn render_dashboard(clients: &Clients, orchestrator: &RulesOrchestrator) -> String {
    let mut repos = Vec::new();
    for (owner, repo) in orchestrator.watch_repos() {
        repos.push(build_repo_view(clients, owner, repo).await);
    }

    let dashboard = RenovateDashboard {
        generated_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        repos,
    };

    dashboard
        .render()
        .unwrap_or_else(|e| format!("<h1>template error</h1><pre>{e}</pre>"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_renovate_dashboard() {
        let dashboard = RenovateDashboard {
            generated_at: "2026-07-04 00:00:00 UTC".to_string(),
            repos: vec![
                RepoView {
                    slug: "anurag/k8s".to_string(),
                    error: None,
                    prs: vec![PrView {
                        number: 42,
                        title: "chore(deps): update rust docker tag to v1.96.1".to_string(),
                        html_url: "https://git.example/anurag/k8s/pulls/42".to_string(),
                        labels: vec!["renovate".to_string(), "renovate/patch".to_string()],
                        status: "success",
                        checks: vec![
                            CheckView {
                                context: "ci/build".to_string(),
                                state: "success",
                                target_url: "https://git.example/anurag/k8s/actions/runs/1"
                                    .to_string(),
                            },
                            CheckView {
                                context: "ci/lint".to_string(),
                                state: "pending",
                                target_url: String::new(),
                            },
                        ],
                    }],
                },
                RepoView {
                    slug: "anurag/home-gateway".to_string(),
                    error: None,
                    prs: vec![],
                },
                RepoView {
                    slug: "anurag/solar-panels".to_string(),
                    error: Some("connection refused".to_string()),
                    prs: vec![],
                },
            ],
        };

        insta::assert_snapshot!(dashboard.render().unwrap());
    }
}
