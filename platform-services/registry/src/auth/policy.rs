use registry_api::Source;
use uuid::Uuid;
use wildmatch::WildMatch;

use super::{ForgejoClaims, Principal};
use crate::{error::ApiError, state::AppState};

#[derive(Debug, Clone)]
pub struct Publisher {
    pub actor: String,
    pub kind: &'static str,
    pub source: Option<Source>,
}

#[derive(Debug, Clone)]
pub struct TrustRule {
    pub id: Uuid,
    pub repository: String,
    pub repository_id: Option<String>,
    pub ref_pattern: Option<String>,
    pub workflow: Option<String>,
}

impl TrustRule {
    pub fn matches(&self, claims: &ForgejoClaims) -> bool {
        let repository = claims.repository.to_lowercase();
        if self.repository.to_lowercase() != repository {
            return false;
        }

        if let Some(pinned) = &self.repository_id
            && claims.repository_id.as_ref() != Some(pinned)
        {
            return false;
        }

        if let Some(pattern) = &self.ref_pattern {
            let Some(git_ref) = &claims.git_ref else {
                return false;
            };
            if !WildMatch::new(pattern).matches(git_ref) {
                return false;
            }
        }

        if let Some(workflow) = &self.workflow {
            let Some(workflow_ref) = &claims.workflow_ref else {
                return false;
            };
            let workflow_ref = workflow_ref.to_lowercase();
            let matches_dir = |dir: &str| {
                workflow_ref
                    .starts_with(&format!("{repository}/{dir}/{}@", workflow.to_lowercase()))
            };
            if !matches_dir(".forgejo/workflows") && !matches_dir(".gitea/workflows") {
                return false;
            }
        }

        true
    }
}

pub async fn authorize_publish(
    state: &AppState,
    principal: &Principal,
    ecosystem: &str,
    package: &str,
) -> Result<Publisher, ApiError> {
    let publisher = |source| Publisher {
        actor: principal.actor(),
        kind: principal.kind(),
        source,
    };

    match principal {
        Principal::User { admin: true, .. } => Ok(publisher(None)),
        Principal::User { name, .. } => Err(ApiError::Forbidden(format!(
            "{name} may not publish {package}; use a publish token or trusted publishing"
        ))),
        Principal::Token(token) if token.allows(ecosystem, package) => Ok(publisher(None)),
        Principal::Token(token) => Err(ApiError::Forbidden(format!(
            "token {} is not scoped to {package}",
            token.name
        ))),
        Principal::Forgejo(claims) => {
            if claims
                .event_name
                .as_deref()
                .is_some_and(|e| e.starts_with("pull_request"))
            {
                return Err(ApiError::Forbidden(
                    "pull request runs may not publish".to_owned(),
                ));
            }

            let issuer = state.auth.forgejo_issuer().unwrap_or_default();
            let rules = sqlx::query_as!(
                TrustRule,
                "SELECT id, repository, repository_id, ref_pattern, workflow
                 FROM core_trusted_publishers
                 WHERE ecosystem = $1 AND package = $2 AND issuer = $3",
                ecosystem,
                package,
                issuer
            )
            .fetch_all(&state.pool)
            .await?;

            let Some(rule) = rules.iter().find(|rule| rule.matches(claims)) else {
                return Err(ApiError::Forbidden(format!(
                    "{} (ref {}) is not a trusted publisher of {package}",
                    claims.repository,
                    claims.git_ref.as_deref().unwrap_or("?"),
                )));
            };

            if rule.repository_id.is_none()
                && let Some(repository_id) = &claims.repository_id
            {
                sqlx::query!(
                    "UPDATE core_trusted_publishers SET repository_id = $2
                     WHERE id = $1 AND repository_id IS NULL",
                    rule.id,
                    repository_id
                )
                .execute(&state.pool)
                .await?;
                tracing::info!(
                    package,
                    repository = claims.repository,
                    repository_id,
                    "pinned trusted publisher repository id"
                );
            }

            Ok(publisher(Some(Source {
                repository: Some(claims.repository.clone()),
                git_ref: claims.git_ref.clone(),
                sha: claims.sha.clone(),
                run_id: claims.run_id.clone(),
            })))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims() -> ForgejoClaims {
        ForgejoClaims {
            repository: "anurag/k8s".to_owned(),
            repository_id: Some("12".to_owned()),
            git_ref: Some("refs/heads/main".to_owned()),
            workflow_ref: Some(
                "anurag/k8s/.forgejo/workflows/publish-cli-ff.yaml@refs/heads/main".to_owned(),
            ),
            event_name: Some("push".to_owned()),
            ..ForgejoClaims::default()
        }
    }

    fn rule() -> TrustRule {
        TrustRule {
            id: Uuid::nil(),
            repository: "Anurag/K8s".to_owned(),
            repository_id: None,
            ref_pattern: None,
            workflow: None,
        }
    }

    #[test]
    fn repository_must_match() {
        assert!(rule().matches(&claims()));
        assert!(
            !TrustRule {
                repository: "anurag/other".to_owned(),
                ..rule()
            }
            .matches(&claims())
        );
    }

    #[test]
    fn pinned_repository_id_must_match() {
        assert!(
            TrustRule {
                repository_id: Some("12".to_owned()),
                ..rule()
            }
            .matches(&claims())
        );
        assert!(
            !TrustRule {
                repository_id: Some("99".to_owned()),
                ..rule()
            }
            .matches(&claims())
        );
    }

    #[test]
    fn ref_pattern_is_a_glob() {
        let tags = TrustRule {
            ref_pattern: Some("refs/tags/v*".to_owned()),
            ..rule()
        };
        assert!(!tags.matches(&claims()));
        assert!(tags.matches(&ForgejoClaims {
            git_ref: Some("refs/tags/v1.2.0".to_owned()),
            ..claims()
        }));
        assert!(
            TrustRule {
                ref_pattern: Some("refs/heads/main".to_owned()),
                ..rule()
            }
            .matches(&claims())
        );
    }

    #[test]
    fn workflow_matches_the_file_path() {
        let ff = TrustRule {
            workflow: Some("publish-cli-ff.yaml".to_owned()),
            ..rule()
        };
        assert!(ff.matches(&claims()));
        assert!(
            !TrustRule {
                workflow: Some("publish-cli-ai.yaml".to_owned()),
                ..rule()
            }
            .matches(&claims())
        );
        assert!(!ff.matches(&ForgejoClaims {
            workflow_ref: None,
            ..claims()
        }));
    }
}
