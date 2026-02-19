use crate::{auth::Permissions, error::AppError};
use axum::http::StatusCode;

#[derive(Clone)]
pub struct PermissionsManager {}

impl PermissionsManager {
    const WILDCARD: &str = "*";

    pub fn new() -> Self {
        Self {}
    }

    pub fn enforce(
        &self,
        perms: &Permissions,
        method: &str,
        namespace: &str,
    ) -> Result<(), AppError> {
        let method_allowed = perms
            .permitted_methods
            .iter()
            .any(|m| m == method || m == Self::WILDCARD);

        let namespace_allowed = perms
            .permitted_namespaces
            .iter()
            .any(|n| n == namespace || n == Self::WILDCARD);

        if method_allowed && namespace_allowed {
            Ok(())
        } else {
            Err(AppError::StatusCode(StatusCode::FORBIDDEN))
        }
    }
}
