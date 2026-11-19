//! Anonymous callers may only run persisted documents, and their errors are stripped down to a
//! code so they can't learn subgraph, type or field names. Persisted document enforcement runs
//! before the router validates the JWT, so it's skipped for anything carrying a token, and a token
//! that then fails validation is rejected instead of being treated as anonymous.

use hive_router::{
    GraphQLError, PlanExecutionOutput, async_trait,
    executor::{
        execution::client_request_details::JwtRequestDetails,
        response::graphql_error::GraphQLErrorExtensions,
    },
    http::StatusCode,
    plugins::{
        hooks::{
            on_graphql_analysis::{OnGraphqlAnalysisHookPayload, OnGraphqlAnalysisHookResult},
            on_graphql_error::{OnGraphQLErrorHookPayload, OnGraphQLErrorHookResult},
            on_graphql_params::{OnGraphQLParamsStartHookPayload, OnGraphQLParamsStartHookResult},
            on_plugin_init::{OnPluginInitPayload, OnPluginInitResult},
        },
        plugin_trait::{FromGraphQLErrorToResponse, RouterPlugin, StartHookPayload},
    },
};

const MESSAGE: &str = "request failed";

const KEEP_MESSAGE: &[&str] = &[
    "UNAUTHENTICATED",
    "INTROSPECTION_DISABLED",
    "PERSISTED_DOCUMENT_ID_REQUIRED",
    "PERSISTED_DOCUMENT_NOT_FOUND",
    "RATE_LIMITED",
];

pub struct AnonymousPlugin;

#[async_trait]
impl RouterPlugin for AnonymousPlugin {
    type Config = ();

    fn plugin_name() -> &'static str {
        "anonymous"
    }

    fn on_plugin_init(payload: OnPluginInitPayload<Self>) -> OnPluginInitResult<Self> {
        payload.initialize_plugin(Self)
    }

    async fn on_graphql_params<'exec>(
        &'exec self,
        payload: OnGraphQLParamsStartHookPayload<'exec>,
    ) -> OnGraphQLParamsStartHookResult<'exec> {
        let credentials = payload
            .router_http_request
            .headers
            .get("authorization")
            .is_some_and(|value| !value.is_empty());
        if credentials && let Ok(mut context) = payload.request_context.write() {
            context.persisted_documents().set_skip_enforcement(true);
        }

        payload.proceed()
    }

    async fn on_graphql_analysis<'exec>(
        &'exec self,
        payload: &mut OnGraphqlAnalysisHookPayload<'exec>,
    ) -> OnGraphqlAnalysisHookResult {
        let skipped = payload
            .request_context
            .read()
            .is_ok_and(|context| context.persisted_documents().skip_enforcement() == Some(true));
        let anonymous = matches!(
            payload.client_request_details.jwt.as_ref(),
            JwtRequestDetails::Unauthenticated
        );
        if !(skipped && anonymous) {
            return OnGraphqlAnalysisHookResult::Proceed;
        }

        OnGraphqlAnalysisHookResult::EndWithResponse(
            PlanExecutionOutput::from_graphql_error_to_response(
                GraphQLError::from_message_and_code("invalid or expired token", "UNAUTHENTICATED"),
                StatusCode::UNAUTHORIZED,
            ),
        )
    }

    fn on_graphql_error<'req>(
        &'req self,
        mut payload: OnGraphQLErrorHookPayload<'req>,
    ) -> OnGraphQLErrorHookResult<'req> {
        // validation errors come before the router checks the jwt, so the status is still unknown
        let authenticated = payload.request_context.read().is_ok_and(|context| {
            match context.authentication().jwt_status() {
                Some(status) => *status,
                None => context.persisted_documents().skip_enforcement() == Some(true),
            }
        });
        if !authenticated {
            mask(&mut payload.error);
        }

        payload.proceed()
    }
}

fn mask(error: &mut GraphQLError) {
    let code = error.extensions.code.take();
    let from_router = error.extensions.service_name.is_none();
    if !(from_router
        && code
            .as_deref()
            .is_some_and(|code| KEEP_MESSAGE.contains(&code)))
    {
        error.message = MESSAGE.to_owned();
    }

    error.locations = None;
    error.extensions = GraphQLErrorExtensions {
        code,
        ..Default::default()
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(message: &str, code: &str) -> GraphQLError {
        let mut error = GraphQLError::from_message_and_code(message, code);
        error.extensions.service_name = Some("registry".into());
        error
    }

    #[test]
    fn anonymous_errors_keep_only_their_code() {
        let mut validation = error(
            "Cannot query field \"secret\" on type \"Query\".",
            "FieldsOnCorrectType",
        );
        mask(&mut validation);
        assert_eq!(validation.message, MESSAGE);
        assert_eq!(
            validation.extensions.code.as_deref(),
            Some("FieldsOnCorrectType")
        );
        assert_eq!(validation.extensions.service_name, None);

        let mut subgraph = error("token required", "UNAUTHENTICATED");
        mask(&mut subgraph);
        assert_eq!(subgraph.message, MESSAGE);
        assert_eq!(subgraph.extensions.service_name, None);

        let mut router = GraphQLError::from_message_and_code("too many requests", "RATE_LIMITED");
        mask(&mut router);
        assert_eq!(router.message, "too many requests");
    }
}
