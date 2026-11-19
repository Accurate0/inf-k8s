//! Adds `extensions.diagnostics` (request id, trace id, phase and per-subgraph timings) to
//! responses for callers with a valid JWT, along with an `x-request-id` header.

use std::time::{Duration, Instant};

use hive_router::{
    async_trait,
    executor::execution::client_request_details::JwtRequestDetails,
    ntex::http::header::{HeaderName, HeaderValue},
    plugins::{
        hooks::{
            on_execute::{DemandControlCost, OnExecuteStartHookPayload, OnExecuteStartHookResult},
            on_graphql_analysis::{OnGraphqlAnalysisHookPayload, OnGraphqlAnalysisHookResult},
            on_http_request::{OnHttpRequestHookFuture, OnHttpRequestHookPayload},
            on_plugin_init::{OnPluginInitPayload, OnPluginInitResult},
            on_query_plan::{OnQueryPlanStartHookPayload, OnQueryPlanStartHookResult},
            on_subgraph_execute::{
                OnSubgraphExecuteStartHookPayload, OnSubgraphExecuteStartHookResult,
            },
        },
        plugin_context::PluginContext,
        plugin_trait::{EndHookPayload, RouterPlugin, StartHookPayload},
    },
    query_planner::planner::plan_nodes::PlanNode,
    telemetry::logging::request_id::REQUEST_IDENTIFIERS,
};
use serde::Serialize;

pub struct DiagnosticsPlugin;

struct Started(Instant);

#[derive(Default)]
struct Timings {
    plan: Option<Duration>,
    fetches: Vec<Fetch>,
}

struct Fetch {
    name: String,
    start: Duration,
    duration: Duration,
    errors: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Diagnostics {
    request_id: Option<String>,
    trace_id: Option<String>,
    duration_ms: f64,
    plan_ms: Option<f64>,
    execute_ms: f64,
    subgraphs: Vec<SubgraphTiming>,
    cost: Option<Cost>,
}

#[derive(Serialize)]
struct Cost {
    estimated: u64,
    actual: u64,
    max: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SubgraphTiming {
    name: String,
    start_ms: f64,
    duration_ms: f64,
    errors: usize,
}

#[async_trait]
impl RouterPlugin for DiagnosticsPlugin {
    type Config = ();

    fn plugin_name() -> &'static str {
        "diagnostics"
    }

    fn on_plugin_init(payload: OnPluginInitPayload<Self>) -> OnPluginInitResult<Self> {
        payload.initialize_plugin(Self)
    }

    fn on_http_request<'req>(
        &'req self,
        payload: OnHttpRequestHookPayload<'req>,
    ) -> OnHttpRequestHookFuture<'req> {
        Box::pin(async move {
            // a websocket shares one context across all its operations, so it gets no timings
            let upgrade = payload
                .router_http_request
                .headers()
                .contains_key("upgrade");
            if !upgrade {
                payload.context.insert(Started(Instant::now()));
            }

            let Some(value) = identifiers()
                .0
                .and_then(|id| HeaderValue::from_str(&id).ok())
            else {
                return payload.proceed();
            };

            payload.on_end(move |end| {
                if !end.context.contains::<Timings>() {
                    return end.proceed();
                }

                end.map_response(|mut response| {
                    response
                        .response_mut()
                        .headers_mut()
                        .insert(HeaderName::from_static("x-request-id"), value);
                    response
                })
                .proceed()
            })
        })
    }

    async fn on_graphql_analysis<'exec>(
        &'exec self,
        payload: &mut OnGraphqlAnalysisHookPayload<'exec>,
    ) -> OnGraphqlAnalysisHookResult {
        let authenticated = matches!(
            payload.client_request_details.jwt.as_ref(),
            JwtRequestDetails::Authenticated { .. }
        );
        if authenticated && payload.context.contains::<Started>() {
            payload.context.insert(Timings::default());
        }

        OnGraphqlAnalysisHookResult::Proceed
    }

    async fn on_query_plan<'exec>(
        &'exec self,
        payload: OnQueryPlanStartHookPayload<'exec>,
    ) -> OnQueryPlanStartHookResult<'exec> {
        if !payload.context.contains::<Timings>() {
            return payload.proceed();
        }

        let context = payload.context;
        let started = Instant::now();
        payload.on_end(move |end| {
            if let Some(mut timings) = context.get_mut::<Timings>() {
                timings.plan = Some(started.elapsed());
            }
            end.proceed()
        })
    }

    async fn on_subgraph_execute<'exec>(
        &'exec self,
        payload: OnSubgraphExecuteStartHookPayload<'exec>,
    ) -> OnSubgraphExecuteStartHookResult<'exec> {
        if !payload.context.contains::<Timings>() {
            return payload.proceed();
        }

        let name = payload.subgraph_name.to_owned();
        let start = since_start(payload.context);
        let started = Instant::now();
        payload.on_end(move |end| {
            let errors = end.execution_result.errors.as_ref().map_or(0, Vec::len);
            if let Some(mut timings) = end.context.get_mut::<Timings>() {
                timings.fetches.push(Fetch {
                    name,
                    start,
                    duration: started.elapsed(),
                    errors,
                });
            }
            end.proceed()
        })
    }

    async fn on_execute<'exec>(
        &'exec self,
        payload: OnExecuteStartHookPayload<'exec>,
    ) -> OnExecuteStartHookResult<'exec> {
        let subscription = matches!(payload.query_plan.node, Some(PlanNode::Subscription(_)));
        if subscription || !payload.context.contains::<Timings>() {
            return payload.proceed();
        }

        let context = payload.context;
        let started = Instant::now();
        payload.on_end(move |mut end| {
            let execute = started.elapsed();
            if let Some(diagnostics) = diagnostics(context, execute, end.demand_control_cost) {
                end.add_extension("diagnostics", diagnostics);
            }
            end.proceed()
        })
    }
}

fn identifiers() -> (Option<String>, Option<String>) {
    REQUEST_IDENTIFIERS
        .try_with(|ids| {
            (
                Some(ids.req_id().to_owned()),
                ids.trace_id().map(str::to_owned),
            )
        })
        .unwrap_or_default()
}

fn since_start(context: &PluginContext) -> Duration {
    context
        .get_ref::<Started>()
        .map(|started| started.0.elapsed())
        .unwrap_or_default()
}

fn diagnostics(
    context: &PluginContext,
    execute: Duration,
    cost: Option<DemandControlCost>,
) -> Option<Diagnostics> {
    let duration = since_start(context);
    let mut timings = context.get_mut::<Timings>()?;
    let mut fetches = std::mem::take(&mut timings.fetches);
    fetches.sort_by_key(|fetch| fetch.start);

    let (request_id, trace_id) = identifiers();
    Some(Diagnostics {
        request_id,
        trace_id,
        duration_ms: ms(duration),
        plan_ms: timings.plan.map(ms),
        execute_ms: ms(execute),
        subgraphs: fetches
            .into_iter()
            .map(|fetch| SubgraphTiming {
                name: fetch.name,
                start_ms: ms(fetch.start),
                duration_ms: ms(fetch.duration),
                errors: fetch.errors,
            })
            .collect(),
        cost: cost.map(|cost| Cost {
            estimated: cost.estimated,
            actual: cost.actual,
            max: cost.max,
        }),
    })
}

fn ms(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 10_000.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn milliseconds_round_to_a_tenth() {
        assert_eq!(ms(Duration::from_micros(1_249)), 1.2);
        assert_eq!(ms(Duration::from_micros(1_260)), 1.3);
        assert_eq!(ms(Duration::ZERO), 0.0);
    }
}
