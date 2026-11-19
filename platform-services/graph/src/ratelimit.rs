//! Limits operations per caller: the JWT subject when authenticated, otherwise the client IP the
//! gateway puts in X-Real-IP. Counts are per router pod.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hive_router::{
    GraphQLError, PlanExecutionOutput, async_trait,
    executor::execution::client_request_details::JwtRequestDetails,
    http::StatusCode,
    plugins::{
        hooks::{
            on_graphql_analysis::{OnGraphqlAnalysisHookPayload, OnGraphqlAnalysisHookResult},
            on_plugin_init::{OnPluginInitPayload, OnPluginInitResult},
        },
        plugin_trait::{FromGraphQLErrorToResponse, RouterPlugin},
    },
    sonic_rs::JsonValueTrait,
};
use moka::future::Cache;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct RateLimitConfig {
    authenticated: Limit,
    anonymous: Limit,
}

#[derive(Deserialize, Clone, Copy)]
struct Limit {
    requests: u32,
    #[serde(with = "hive_router::humantime_serde")]
    per: Duration,
}

struct Limiter {
    limit: Limit,
    callers: Cache<String, Arc<Mutex<Instant>>>,
}

impl Limiter {
    fn new(limit: Limit) -> Result<Self, String> {
        if limit.requests == 0 || limit.per.is_zero() {
            return Err("ratelimit requests and per must be above zero".into());
        }

        Ok(Self {
            limit,
            callers: Cache::builder()
                .max_capacity(100_000)
                .time_to_idle(limit.per)
                .build(),
        })
    }

    async fn check(&self, key: String) -> Result<(), Duration> {
        let now = Instant::now();
        let tat = self
            .callers
            .get_with(key, async move { Arc::new(Mutex::new(now)) })
            .await;
        let mut tat = tat.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        gcra(&mut tat, now, self.limit)
    }
}

/// Generic cell rate algorithm: up to `requests` at once, refilling one every `per / requests`.
fn gcra(tat: &mut Instant, now: Instant, limit: Limit) -> Result<(), Duration> {
    let interval = limit.per / limit.requests;
    let next = (*tat).max(now) + interval;
    let ahead = next - now;
    if ahead > limit.per {
        return Err(ahead - limit.per);
    }

    *tat = next;
    Ok(())
}

pub struct RateLimitPlugin {
    authenticated: Limiter,
    anonymous: Limiter,
}

#[async_trait]
impl RouterPlugin for RateLimitPlugin {
    type Config = RateLimitConfig;

    fn plugin_name() -> &'static str {
        "ratelimit"
    }

    fn on_plugin_init(payload: OnPluginInitPayload<Self>) -> OnPluginInitResult<Self> {
        let config = payload.config()?;
        payload.initialize_plugin(Self {
            authenticated: Limiter::new(config.authenticated)?,
            anonymous: Limiter::new(config.anonymous)?,
        })
    }

    async fn on_graphql_analysis<'exec>(
        &'exec self,
        payload: &mut OnGraphqlAnalysisHookPayload<'exec>,
    ) -> OnGraphqlAnalysisHookResult {
        let subject = match payload.client_request_details.jwt.as_ref() {
            JwtRequestDetails::Authenticated { claims, .. } => claims
                .get("sub")
                .and_then(|sub| sub.as_str())
                .map(str::to_owned),
            JwtRequestDetails::Unauthenticated => None,
        };

        let result = match subject {
            Some(sub) => self.authenticated.check(format!("user:{sub}")).await,
            None => {
                let ip = payload
                    .router_http_request
                    .headers
                    .get("x-real-ip")
                    .and_then(|ip| ip.to_str().ok())
                    .unwrap_or("unknown");
                self.anonymous.check(format!("ip:{ip}")).await
            }
        };

        match result {
            Ok(()) => OnGraphqlAnalysisHookResult::Proceed,
            Err(retry) => {
                let mut error =
                    GraphQLError::from_message_and_code("too many requests", "RATE_LIMITED");
                error.extensions.extensions.insert(
                    "retryAfterMs".into(),
                    (retry.as_millis() as u64).max(1).into(),
                );
                OnGraphqlAnalysisHookResult::EndWithResponse(
                    PlanExecutionOutput::from_graphql_error_to_response(
                        error,
                        StatusCode::TOO_MANY_REQUESTS,
                    ),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bursts_up_to_the_limit_then_refills() {
        let limit = Limit {
            requests: 2,
            per: Duration::from_secs(1),
        };
        let start = Instant::now();
        let mut tat = start;

        assert!(gcra(&mut tat, start, limit).is_ok());
        assert!(gcra(&mut tat, start, limit).is_ok());
        assert_eq!(
            gcra(&mut tat, start, limit),
            Err(Duration::from_millis(500))
        );

        assert!(gcra(&mut tat, start + Duration::from_millis(500), limit).is_ok());
        assert!(gcra(&mut tat, start + Duration::from_millis(500), limit).is_err());
    }
}
