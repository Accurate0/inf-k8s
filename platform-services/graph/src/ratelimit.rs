//! Limits operations per caller: the JWT subject when authenticated, otherwise the client IP the
//! gateway puts in X-Real-IP. Each caller also has a cost budget drained by the actual demand control
//! cost of what it ran; once it's in debt, further operations are rejected until it refills. Counts
//! are per router pod.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hive_router::{
    GraphQLError, PlanExecutionOutput, async_trait,
    executor::execution::client_request_details::JwtRequestDetails,
    http::StatusCode,
    plugins::{
        hooks::{
            on_execute::{OnExecuteStartHookPayload, OnExecuteStartHookResult},
            on_graphql_analysis::{OnGraphqlAnalysisHookPayload, OnGraphqlAnalysisHookResult},
            on_plugin_init::{OnPluginInitPayload, OnPluginInitResult},
        },
        plugin_trait::{
            EndHookPayload, FromGraphQLErrorToResponse, RouterPlugin, StartHookPayload,
        },
    },
    query_planner::planner::plan_nodes::PlanNode,
    sonic_rs::JsonValueTrait,
};
use moka::future::Cache;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct RateLimitConfig {
    authenticated: Limit,
    anonymous: Limit,
    cost: Option<CostConfig>,
}

#[derive(Deserialize)]
struct CostConfig {
    authenticated: Budget,
    anonymous: Budget,
}

#[derive(Deserialize, Clone, Copy)]
struct Budget {
    budget: u64,
    #[serde(with = "hive_router::humantime_serde")]
    per: Duration,
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
        gcra(&mut lock(&tat), now, self.limit)
    }
}

struct CostLimiter {
    budget: Budget,
    callers: moka::sync::Cache<String, Arc<Mutex<Instant>>>,
}

impl CostLimiter {
    fn new(budget: Budget) -> Result<Self, String> {
        if budget.budget == 0 || budget.per.is_zero() {
            return Err("ratelimit cost budget and per must be above zero".into());
        }

        Ok(Self {
            budget,
            callers: moka::sync::Cache::builder()
                .max_capacity(100_000)
                .time_to_idle(budget.per * 2)
                .build(),
        })
    }

    fn check(&self, key: &str) -> Result<(), Duration> {
        match self.callers.get(key) {
            Some(tat) => in_debt(*lock(&tat), Instant::now(), self.budget),
            None => Ok(()),
        }
    }

    fn charge(&self, key: String, cost: u64) {
        let now = Instant::now();
        let tat = self.callers.get_with(key, || Arc::new(Mutex::new(now)));
        spend(&mut lock(&tat), now, self.budget, cost);
    }
}

fn lock(tat: &Mutex<Instant>) -> std::sync::MutexGuard<'_, Instant> {
    tat.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A caller is in debt once it has spent more than a full budget ahead of now.
fn in_debt(tat: Instant, now: Instant, budget: Budget) -> Result<(), Duration> {
    let ahead = tat.saturating_duration_since(now);
    if ahead > budget.per {
        return Err(ahead - budget.per);
    }

    Ok(())
}

fn spend(tat: &mut Instant, now: Instant, budget: Budget, cost: u64) {
    let per_unit = budget.per.as_secs_f64() / budget.budget as f64;
    *tat = (*tat).max(now) + Duration::from_secs_f64(per_unit * cost as f64);
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
    cost: Option<(CostLimiter, CostLimiter)>,
}

/// Who analysis charged this operation to, so execution can bill its actual cost.
struct Caller {
    key: String,
    authenticated: bool,
}

impl RateLimitPlugin {
    fn budget(&self, authenticated: bool) -> Option<&CostLimiter> {
        self.cost
            .as_ref()
            .map(|(users, anonymous)| if authenticated { users } else { anonymous })
    }
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
            cost: match config.cost {
                Some(cost) => Some((
                    CostLimiter::new(cost.authenticated)?,
                    CostLimiter::new(cost.anonymous)?,
                )),
                None => None,
            },
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

        let authenticated = subject.is_some();
        let key = match subject {
            Some(sub) => format!("user:{sub}"),
            None => {
                let ip = payload
                    .router_http_request
                    .headers
                    .get("x-real-ip")
                    .and_then(|ip| ip.to_str().ok())
                    .unwrap_or("unknown");
                format!("ip:{ip}")
            }
        };

        let limiter = if authenticated {
            &self.authenticated
        } else {
            &self.anonymous
        };
        if let Err(retry) = limiter.check(key.clone()).await {
            return limited(retry, None);
        }

        if let Some(budget) = self.budget(authenticated) {
            if let Err(retry) = budget.check(&key) {
                return limited(retry, Some("cost"));
            }
            payload.context.insert(Caller { key, authenticated });
        }

        OnGraphqlAnalysisHookResult::Proceed
    }

    async fn on_execute<'exec>(
        &'exec self,
        payload: OnExecuteStartHookPayload<'exec>,
    ) -> OnExecuteStartHookResult<'exec> {
        let subscription = matches!(payload.query_plan.node, Some(PlanNode::Subscription(_)));
        let caller = payload
            .context
            .get_ref::<Caller>()
            .map(|caller| (caller.key.clone(), caller.authenticated));
        let Some((key, authenticated)) = caller.filter(|_| !subscription) else {
            return payload.proceed();
        };

        payload.on_end(move |end| {
            if let (Some(budget), Some(cost)) =
                (self.budget(authenticated), end.demand_control_cost)
            {
                budget.charge(key, cost.actual);
            }
            end.proceed()
        })
    }
}

fn limited(retry: Duration, reason: Option<&str>) -> OnGraphqlAnalysisHookResult {
    let mut error = GraphQLError::from_message_and_code("too many requests", "RATE_LIMITED");
    error.extensions.extensions.insert(
        "retryAfterMs".into(),
        (retry.as_millis() as u64).max(1).into(),
    );
    if let Some(reason) = reason {
        error
            .extensions
            .extensions
            .insert("reason".into(), reason.into());
    }

    OnGraphqlAnalysisHookResult::EndWithResponse(
        PlanExecutionOutput::from_graphql_error_to_response(error, StatusCode::TOO_MANY_REQUESTS),
    )
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

    #[test]
    fn cost_overshoots_once_then_waits_for_the_budget_to_refill() {
        let budget = Budget {
            budget: 100,
            per: Duration::from_secs(10),
        };
        let start = Instant::now();
        let mut tat = start;

        spend(&mut tat, start, budget, 60);
        assert!(in_debt(tat, start, budget).is_ok());

        spend(&mut tat, start, budget, 80);
        assert_eq!(in_debt(tat, start, budget), Err(Duration::from_secs(4)));

        let later = start + Duration::from_secs(4);
        assert!(in_debt(tat, later, budget).is_ok());
        assert!(in_debt(tat, later - Duration::from_millis(1), budget).is_err());
    }
}
