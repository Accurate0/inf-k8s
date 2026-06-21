-- Per-model pricing, synced from llm-prices.com by `aig prices sync`. Rates are USD per
-- one million tokens; id matches the model string the gateway resolves a request to.
CREATE TABLE IF NOT EXISTS model_prices (
    id                    TEXT PRIMARY KEY,
    input_usd_per_mtok    DOUBLE PRECISION NOT NULL,
    output_usd_per_mtok   DOUBLE PRECISION NOT NULL,
    cached_usd_per_mtok   DOUBLE PRECISION,
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Estimated USD cost of the request, computed from model_prices at record time. Zero when
-- no price is known for the resolved model, or for cache hits (no upstream tokens billed).
ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS cost_usd DOUBLE PRECISION NOT NULL DEFAULT 0;

-- Re-added (dropped in 0003): a cache hit is served from the response cache without an
-- upstream call, so it is recorded with the cached token counts but zero cost.
ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS cache_hit BOOLEAN NOT NULL DEFAULT FALSE;
