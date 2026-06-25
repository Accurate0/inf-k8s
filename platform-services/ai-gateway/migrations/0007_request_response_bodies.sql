-- Full upstream request/response bodies for each call, retained for auditing and replay.
-- Stored as the provider-native bytes sent to / received from upstream (text, since all
-- supported providers exchange JSON or SSE).
ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS request_body TEXT;
ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS response_body TEXT;
