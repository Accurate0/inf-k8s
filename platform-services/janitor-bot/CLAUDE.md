# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`janitor-bot` is a Rust (edition 2024) webhook service that automates PR/repo housekeeping
on a self-hosted Forgejo instance, mirrored to GitHub. It listens for Forgejo, GitHub, and
ArgoCD webhooks, evaluates a declarative ruleset, and takes actions (label, comment, merge,
approve, set commit status, create issues, run `argocd diff`). It also polls open PRs on a
cron schedule every 10 minutes. Watched repo is hardcoded: `anurag/k8s` (`WATCH_REPOS` /
`FORGEJO_OWNER` in `src/main.rs`).

## Commands

```
cargo build                    # build (build.rs validates rules at compile time — see below)
cargo run --bin janitor-bot    # run the server (needs env vars below); listens on 0.0.0.0:3000
cargo test                     # runs tests/integration.rs (snapshot tests via insta)
cargo test <name>              # run a single test case (rstest case names, e.g. `pr-merge`)
cargo clippy
cargo run --bin gen-schema     # regenerate rule.schema.json + rules.schema.json from Rust types
```

Required env vars to run: `FORGEJO_INSTANCE_URL`, `FORGEJO_ACCESS_KEY`, `GITHUB_TOKEN`,
`FORGEJO_INCOMING_WEBHOOK_AUTH`, `GITHUB_WEBHOOK_SECRET`. Optional: `ARGOCD_WEBHOOK_SECRET`,
`FEATURE_FLAGS_URL` (feature flags; falls back to a NoOp provider when unset).

Snapshot tests: review/accept changes with `cargo insta review` (snapshots in
`tests/snapshots/`, fixtures in `tests/fixtures/<event_type>/*.yaml`).

## Architecture

**Event flow:** `routes.rs` (axum handlers, HMAC signature verification) → `event.rs`
(parses webhook JSON into typed `*Event` structs + `BotEvent` enum) → `RulesOrchestrator`
(`rules/mod.rs`) → `actions.rs` execution. `command.rs` handles PR/issue comment commands
(e.g. `janitor merge`, `janitor explain`, `janitor ignore`) separately from the rule engine.

**Rules are data, not code.** `config.yaml` is the entry point; it `!include`s individual
files from `rules/`. `build.rs` resolves the includes into `OUT_DIR/rules.merged.yaml`,
which is `include_str!`'d into the binary at `rules/mod.rs` — these baked-in rules are the
source of truth and the always-available fallback.

**Rule loading (`load_rules` in `rules/mod.rs`):** at startup the bot prefers an external
ruleset over the baked-in one. If `RULES_CONFIGMAP_PATH` is set, it resolves `!include`s at
that path via the same `yaml_include::Transformer` `build.rs` uses, so a ConfigMap can
bundle the raw `config.yaml` + `rules/*.yaml`. A malformed ConfigMap panics — the pod fails
to start and the previous ReplicaSet keeps running. When the env var is unset, it falls back
to the baked-in rules. Rules are loaded once and never reloaded in-process. The ConfigMap is
generated from the existing source files by the root `kustomization.yaml`
(`configMapGenerator`); its name carries a content hash, so editing a rule rolls the
Deployment. The deployment remaps the flat ConfigMap keys back into a `rules/` subdir with
volume `items` so the includes resolve.

**Build-time validation (`build.rs`):** every build (1) validates merged rules against
`rules.schema.json`, and (2) parses each action group's `when` expression and fails the
build if it references a variable not declared in that rule's `variables`. Set
`SKIP_SCHEMA_VALIDATION` to skip step 1. The schema files are *generated* from the Rust
types in `src/rules/schema.rs` via `gen-schema` — edit the Rust types, then regenerate;
don't hand-edit the `.json` schemas.

**Rule evaluation pipeline** (per `RuleDef`, sorted by `priority` desc):
1. `matches` — matcher tree (`all`/`any`/`not` + leaf matchers in `rules/matchers/`)
   gates whether the rule applies to the event.
2. `variables` — named boolean matchers evaluated into a `HashMap<String,bool>`.
3. `actions` — either flat, or conditional groups whose `when` is a boolean expression
   (`src/rules/expr.rs`, supports `&&`/`||`/`!`/parens) over the variables.

`explain_*` methods mirror `evaluate_*` but return matched rules without executing —
this backs the `janitor explain` command. `MatcherCache` dedupes expensive matcher calls
(e.g. API lookups) within a single event evaluation. Per-PR `Mutex` locks (`pr_locks`)
serialize concurrent evaluations of the same PR; workflow events share one global lock.

**Rule enable states:** a rule's `enabled` field supports active / dry-run / disabled —
dry-run logs `[dry-run] would execute` instead of calling the action.

**Clients** (`clients.rs`): `ForgejoClient`, `GitHubClient`, `ArgocdClient`,
`FeatureFlagClient`, each constructed `from_env()`. `git.rs` uses `git2` for local clone
operations (conflict detection / revert).

## Testing approach

`tests/integration.rs` is fixture-driven: each fixture YAML declares an event `type`,
`payload`, expected outbound HTTP `mocks` (matched via `wiremock`), and an optional `now`
timestamp (the orchestrator's clock is injectable via `with_clock`). The test feeds the
event through the real `AppState`/orchestrator and snapshots the response plus all captured
external requests. Add a new behavior by adding a fixture + accepting its snapshot.

## Deployment

`Dockerfile` is parameterized by `ARG BINARY_NAME` (multi-binary repo build). The final
image also bundles the `argocd` CLI (used by the `argocd_diff` action). Image updates land
via automated `[skip ci]` commits to the parent `inf-k8s` repo.
