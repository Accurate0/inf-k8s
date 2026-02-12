use crate::{auth::Permissions, error::AppError};
use axum::http::StatusCode;

#[derive(Clone)]
pub struct PermissionsManager {}

impl PermissionsManager {
    pub fn new() -> Self {
        Self {}
    }

    pub fn enforce(
        &self,
        perms: &Permissions,
        method: &str,
        namespace: &str,
    ) -> Result<(), AppError> {
        let method_allowed = perms.permitted_methods.iter().any(|m| m == method);
        let namespace_allowed = perms.permitted_namespaces.iter().any(|n| n == namespace);

        if method_allowed && namespace_allowed {
            Ok(())
        } else {
            Err(AppError::StatusCode(StatusCode::FORBIDDEN))
        }
    }
}
