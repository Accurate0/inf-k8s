# ai-gateway

A platform service that sits between every in-cluster project and upstream LLM providers.
Consumers talk to one stable endpoint with a gateway-issued key; the gateway holds the real
provider credentials and adds usage tracking, caching, and routing on top.

**In-cluster endpoint:** `http://api.ai-gateway.svc.cluster.local`

## Goals

- One place that holds upstream provider API keys (Anthropic, OpenAI, OpenRouter, ...)
  instead of each project carrying its own in its Infisical project.
- Per-consumer virtual keys so a leaked key from one project is revocable without
  touching the others, and usage is attributable.
- Spend/usage tracking persisted in Postgres, visible in Grafana.
- Response caching in Dragonfly for repeated identical requests.
- Model/provider routing controlled at runtime via Flipt (kill switches, provider
  failover, per-key model overrides) without redeploying consumers.

## Non-goals

- Not exposed outside the cluster. No HTTPRoute, no Kanidm OAuth2 client. Cluster-local
  consumers only.
- No prompt management, evals, or semantic caching. Exact-match caching only.
- No request/response body persistence — usage metadata only (model, token counts,
  latency, consumer key). Bodies are never written to the database.

## Build vs LiteLLM

LiteLLM provides all of this off the shelf but brings a Python runtime, its own admin UI,
its own migration lifecycle, and a large config surface for a single-tenant cluster. Every
other platform service with logic here is a small Rust service sharing the same workspace
conventions, deployment shape, and CI. The feature set actually needed (passthrough proxy,
key auth, usage rows, cache lookup) is small. Decision: build it as a Rust/axum service,
consistent with janitor-bot and pg-db-controller.

## Architecture

```
consumer pod ──Bearer aig_xxx──▶ ai-gateway (axum, :3000)
                                   │  auth: virtual key lookup (Postgres, cached in-process)
                                   │  route: Flipt evaluation (model/provider overrides)
                                   │  cache: Dragonfly GET (opt-in, exact-match)
                                   ├──▶ upstream provider (streamed or buffered)
                                   │  record: usage row (Postgres), metrics, trace span
                                   └──▶ response to consumer (SSE passthrough for streaming)
```

- Single binary `ai-gateway`, Rust edition 2024, axum + reqwest + sqlx, tokio.
- Provider adapters behind a trait: `anthropic` (native `/v1/messages`) and
  `openai-compatible` (covers OpenAI, OpenRouter, Gemini via its OpenAI-compatible
  endpoint, and most others). Adding a provider is implementing the trait + config.
- Streaming responses are passed through as SSE without buffering; usage is taken from the
  terminal usage event of the stream.

## API surface

| Route | Purpose |
| --- | --- |
| `POST /v1/messages` | Anthropic-compatible passthrough |
| `POST /v1/chat/completions` | OpenAI-compatible passthrough |
| `POST /v1/embeddings` | OpenAI embeddings passthrough |
| `GET /v1/models` | Models the gateway will route, from config + Flipt state |
| `GET /health` | Liveness/readiness |
| `GET /admin/metrics` | Prometheus metrics (ServiceMonitor target) |
| `GET /admin/keys` / `POST /admin/keys` / `DELETE /admin/keys/{id}` | Virtual key management |
| `GET /admin/usage` | Usage summary per key/model/day |

Consumers keep using their existing SDKs (`anthropic`, `async-openai`, etc.) with
`base_url` pointed at the gateway and the virtual key as the API key. The two passthrough
dialects mean no consumer code changes beyond base URL + key.

`/admin/*` is not authenticated by virtual keys; it requires the admin token from the
gateway's own secrets. Key management is expected to happen via `kubectl port-forward` or
a small `just` recipe, not from other services.

## Virtual keys and auth

- Keys look like `aig_<24 random bytes, base62>`. Only a SHA-256 hash is stored.
- A key row carries: name (consumer identity, e.g. `tldr-bot`), hash, optional list of
  allowed models, optional monthly token budget, revoked flag, created/last-used
  timestamps.
- Lookup is in-process-cached with a short TTL so the hot path does not hit Postgres
  per request.
- Consumers store their virtual key in their own Infisical project like any other secret.

## Data model (Postgres via `PostgresDatabase`)

```yaml
apiVersion: inf-k8s.net/v1
kind: PostgresDatabase
metadata:
  name: ai-gateway-database
  namespace: ai-gateway
spec:
  databaseName: ai-gateway
  secretName: ai-gateway-database-secret
  secretNamespace: ai-gateway
```

Tables (sqlx migrations embedded in the binary, run on startup):

- `virtual_keys` — id, name, key_hash, allowed_models[], monthly_token_budget,
  revoked, created_at, last_used_at
- `usage_events` — key_id, key_name, provider, requested_model, resolved_model,
  input_tokens, output_tokens, cache_hit, latency_ms, status, created_at. A
  **TimescaleDB hypertable** (the CNPG image already bundles `timescaledb` and preloads
  `timescaledb.so`): append-only, one row per request, always queried by time range.
  Declared inline via `WITH (tsdb.hypertable, tsdb.partition_column='created_at',
  tsdb.orderby='created_at DESC')` so it lives in the ordinary sqlx migration — no
  separate `create_hypertable()` call, no PRIMARY KEY/FK (a unique constraint would have
  to include the partition column, and an FK on the insert path is undesirable), matching
  the project's other hypertables.
- `usage_daily` — rollup (key_name, model, day, tokens, requests) refreshed by a
  background task, so Grafana queries stay cheap as `usage_events` grows. A TimescaleDB
  continuous aggregate could replace this later; kept as a plain table + task for now so
  the rollup mechanism is independent of the source table's storage.

## Caching (Dragonfly)

- Connection: `redis://dragonfly.dragonfly-system.svc.cluster.local`.
- Opt-in per request via `X-AIG-Cache: <ttl-seconds>` header; never cached by default.
- Cache key: SHA-256 of (resolved provider, resolved model, canonicalized request body).
- Streaming requests bypass the cache entirely.
- Hits are recorded as `cache_hit = true` usage events with zero provider tokens, so cache
  savings are visible in Grafana.

## Flipt integration

- `FLIPT_URL=http://flipt-v2.flipt-v2.svc.cluster.local:8080`, NoOp fallback when unset
  (same pattern as janitor-bot).
- Flags evaluated per request with the virtual key name as entity id:
  - `ai-gateway-enabled` — global kill switch (returns 503 with Retry-After)
  - `ai-gateway-model-override` — variant flag mapping requested model → resolved model,
    enabling provider failover or model upgrades without consumer deploys
  - `ai-gateway-provider` — variant flag selecting between configured upstreams for
    openai-compatible models

## Observability

- Tracing: `OTEL_TRACING_URL=http://monitoring-tempo.monitoring.svc.cluster.local:4318/v1/traces`,
  one span per request with key name, model, cache outcome, provider latency.
- Metrics at `/admin/metrics`: request count/latency histograms by key+model+status,
  token counters, cache hit ratio, upstream error counters.
- Grafana dashboard (phase 2): spend by key, tokens by model over time, cache savings,
  p95 upstream latency.

## Manifests

`platform-services/ai-gateway/manifests/`, mirroring janitor-bot:

- `namespace.yaml` — `ai-gateway`
- `deployment.yaml` — 1 replica, image `ghcr.io/accurate0/ai-gateway`, port 3000,
  runAsNonRoot 10001, readOnlyRootFilesystem, drop ALL caps, `/health` probes, requests
  `10m/32Mi`, `envFrom` secret `ai-gateway-managed-secrets`, env for `FLIPT_URL`,
  `OTEL_TRACING_URL`, `DATABASE_*` from `ai-gateway-database-secret`
- `service.yaml` — Service `api`, port 80 → 3000 (gives the required
  `http://api.ai-gateway.svc.cluster.local`)
- `database.yaml` — the `PostgresDatabase` above
- `secretstore.yaml` — Infisical SecretStore, new Infisical project for upstream provider
  keys + admin token, auth via the replicated `universal-auth-credentials`
- `secret.yaml` — ExternalSecret → `ai-gateway-managed-secrets`
- `servicemonitor.yaml` — `/admin/metrics`, label `release: monitoring`
- `kustomization.yaml` — resources, `commonAnnotations` (`inf-k8s.net/app: ai-gateway`,
  repository), `images` block pinning `ghcr.io/accurate0/ai-gateway` to a commit SHA

`platform-services/ai-gateway/application.yaml` copies kanidm-sync's Application (project
`platform-services`, destination namespace `ai-gateway`, CreateNamespace + ServerSideApply,
automated prune/selfHeal). The `platform-services-apps` app-of-apps discovers it
automatically — no ArgoCD changes needed.

## CI/CD

- `.forgejo/workflows/branch-build-ai-gateway.yaml` — PR validation build (`push: false`,
  `BINARY_NAME=ai-gateway`), paths-filtered to `platform-services/ai-gateway/**`
- `.github/workflows/build-deploy-ai-gateway.yaml` — on main push:
  `Accurate0/workflows/build-push-docker` (rust caching, `BINARY_NAME=ai-gateway`) +
  `cargo test` job + `deploy-app-k8s-v2` with `service-name: ai-gateway`,
  `images: ghcr.io/accurate0/ai-gateway`, `manifests-path:
  platform-services/ai-gateway/manifests`, `FORGEJO_CI_IMAGE_UPDATER` secret
- `Dockerfile` copied from kanidm-sync (same multi-stage rust build taking `BINARY_NAME`)

## Phases

1. **Passthrough proxy + virtual keys.** Scaffold crate, Dockerfile, manifests, CI.
   Anthropic + openai-compatible adapters, streaming passthrough, key auth against
   Postgres, `/health`. Milestone: tldr-bot works through the gateway in a branch.
2. **Usage tracking + dashboard.** `usage_events`/`usage_daily`, `/admin/usage`, metrics,
   Grafana dashboard. Milestone: spend by key visible in Grafana.
3. **Dragonfly caching.** `X-AIG-Cache` support, hit metrics. Milestone: repeated identical
   request served from cache with usage row marked `cache_hit`.
4. **Flipt routing + consumer migration.** Kill switch, model override, provider selection
   flags. Migrate tldr-bot (and any other LLM consumers) to the gateway; remove provider
   keys from their Infisical projects. Milestone: rotating a provider key touches only the
   ai-gateway Infisical project.

## Open questions

- Budget enforcement: when a key exceeds `monthly_token_budget`, hard-fail (429) or
  alert-only? Start alert-only via metrics; enforcement is a flag flip later.
- Cost-in-dollars vs tokens: pricing tables go stale. Start with token counts only;
  dollar estimates can be a Grafana transform with a maintained pricing map if wanted.
- Whether maccas-api or home-gateway have LLM call sites worth migrating in phase 4 —
  audit when phase 4 starts.
