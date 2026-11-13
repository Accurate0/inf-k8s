use std::collections::BTreeSet;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{Duration, Utc};
use registry_api::{
    DOCKER_CONFIG_MEDIA_TYPE, ECOSYSTEM_CLI, HarborAccess, HarborAction, HarborToken,
    HarborTokenRequest,
};
use serde_json::json;

use super::audit::{self, Event};
use crate::{
    auth::{
        Principal,
        grants::{allowed, validate_request},
        policy::authorize_publish,
    },
    config::HarborConfig,
    error::{ApiError, Result},
    harbor::Harbor,
    state::AppState,
};

fn configured(state: &AppState) -> Result<(&Harbor, &HarborConfig)> {
    match (state.harbor.as_ref(), state.config.harbor.as_ref()) {
        (Some(harbor), Some(config)) => Ok((harbor, config)),
        _ => Err(ApiError::Harbor(
            "the harbor token exchange is not configured".to_owned(),
        )),
    }
}

fn robot_name(principal: &Principal) -> String {
    let mut slug = String::new();
    for c in principal.actor().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_matches('-').chars().take(24).collect();

    format!(
        "exch-{}-{}-{:08x}",
        principal.kind(),
        slug.trim_end_matches('-'),
        rand::random::<u32>()
    )
}

fn describe(principal: &Principal) -> String {
    match principal {
        Principal::Forgejo(claims) => format!(
            "{} run {}",
            principal.actor(),
            claims.run_id.as_deref().unwrap_or("?")
        ),
        _ => principal.actor(),
    }
}

async fn authorize(
    state: &AppState,
    config: &HarborConfig,
    principal: &Principal,
    item: &HarborAccess,
) -> Result<()> {
    let mut granted = allowed(&config.grants, principal, &item.project);

    if let Some(package) = &item.package {
        if item.project != config.packages_project {
            return Err(ApiError::BadRequest(format!(
                "package only applies to the {} project",
                config.packages_project
            )));
        }
        authorize_publish(state, principal, ECOSYSTEM_CLI, package).await?;
        granted.extend([HarborAction::Pull, HarborAction::Push]);
    }

    let missing: BTreeSet<HarborAction> = item
        .actions
        .iter()
        .copied()
        .filter(|a| !granted.contains(a))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }

    let missing: Vec<String> = missing
        .iter()
        .map(|a| {
            serde_json::to_value(a)
                .unwrap_or_default()
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    Err(ApiError::Forbidden(format!(
        "{} has no {} grant on harbor project {}",
        principal.actor(),
        missing.join("/"),
        item.project
    )))
}

pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    Json(request): Json<HarborTokenRequest>,
) -> Result<Response> {
    let (harbor, config) = configured(&state)?;
    validate_request(&request.access).map_err(ApiError::BadRequest)?;

    for item in &request.access {
        authorize(&state, config, &principal, item).await?;
    }

    let name = robot_name(&principal);
    let robot = harbor
        .create_robot(
            &name,
            &describe(&principal),
            config.robot_duration_days,
            &request.access,
        )
        .await?;
    let expires_at = robot.expiry(Utc::now() + Duration::days(config.robot_duration_days));
    let access = serde_json::to_value(&request.access).unwrap_or_default();

    let recorded = sqlx::query!(
        "INSERT INTO core_harbor_robots (id, username, actor, actor_kind, access, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
        robot.id,
        robot.name,
        principal.actor(),
        principal.kind(),
        access,
        expires_at
    )
    .execute(&state.pool)
    .await;
    if let Err(e) = recorded {
        if let Err(cleanup) = harbor.delete_robot(robot.id).await {
            tracing::error!(
                robot = robot.name,
                "failed to delete unrecorded robot: {cleanup}"
            );
        }
        return Err(e.into());
    }

    audit::record_as(
        &state.pool,
        &principal,
        Event {
            action: "harbor.token",
            ecosystem: None,
            package: None,
            version: None,
            detail: json!({ "username": robot.name, "access": access, "expires_at": expires_at }),
        },
    )
    .await?;
    tracing::info!(
        robot = robot.name,
        actor = principal.actor(),
        "issued harbor robot"
    );

    let wants_docker_config = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains(DOCKER_CONFIG_MEDIA_TYPE));
    if wants_docker_config {
        let auth = STANDARD.encode(format!("{}:{}", robot.name, robot.secret));
        let body = json!({ "auths": { &config.registry: { "auth": auth } } });
        return Ok((
            [(header::CONTENT_TYPE, DOCKER_CONFIG_MEDIA_TYPE)],
            body.to_string(),
        )
            .into_response());
    }

    Ok(Json(HarborToken {
        registry: config.registry.clone(),
        username: robot.name,
        password: robot.secret,
        expires_at,
        access: request.access,
    })
    .into_response())
}

pub async fn revoke(
    State(state): State<AppState>,
    principal: Principal,
    Path(username): Path<String>,
) -> Result<StatusCode> {
    let (harbor, _) = configured(&state)?;

    let robot = sqlx::query!(
        "SELECT id, actor FROM core_harbor_robots WHERE username = $1 AND deleted_at IS NULL",
        username
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("no live robot named {username}")))?;

    if robot.actor != principal.actor() && !principal.is_admin() {
        return Err(ApiError::Forbidden(format!(
            "{username} was issued to {}",
            robot.actor
        )));
    }

    harbor.delete_robot(robot.id).await?;
    sqlx::query!(
        "UPDATE core_harbor_robots SET deleted_at = now() WHERE id = $1",
        robot.id
    )
    .execute(&state.pool)
    .await?;

    audit::record_as(
        &state.pool,
        &principal,
        Event {
            action: "harbor.revoke",
            ecosystem: None,
            package: None,
            version: None,
            detail: json!({ "username": username }),
        },
    )
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn robot_names_are_harbor_safe() {
        let name = robot_name(&Principal::Forgejo(crate::auth::ForgejoClaims {
            repository: "Anurag/Some.Repo_With-A-Very-Long-Name".to_owned(),
            ..Default::default()
        }));
        assert!(
            name.starts_with("exch-forgejo-forgejo-anurag-some-repo-"),
            "{name}"
        );
        assert_eq!(name.len(), "exch-forgejo-".len() + 24 + 9, "{name}");
        assert!(
            name.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "{name}"
        );
    }
}
