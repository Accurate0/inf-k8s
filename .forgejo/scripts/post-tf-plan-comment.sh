#!/usr/bin/env bash
set -euo pipefail

: "${FORGEJO_TOKEN:?}"
: "${SERVER_URL:?}"
: "${REPO:?}"
: "${PR_NUMBER:?}"
: "${PLAN_FILE:?}"
: "${RUN_ID:=}"
: "${RUN_URL:=}"
: "${COMMIT_SHA:=}"

MARKER='<!-- terraform-plan -->'
API="${SERVER_URL}/api/v1/repos/${REPO}"
AUTH=(-H "Authorization: token ${FORGEJO_TOKEN}" -H "Content-Type: application/json")

max=60000
plan=$(cat "$PLAN_FILE")
if [ ${#plan} -gt $max ]; then
  plan="${plan:0:$max}"$'\n\n... (truncated)'
fi

metadata=$(jq -nc \
  --arg run_id "$RUN_ID" \
  --arg run_url "$RUN_URL" \
  --arg sha "$COMMIT_SHA" \
  '{run_id: $run_id, run_url: $run_url, sha: $sha}')

body=$(jq -Rs \
  --arg plan "$plan" \
  --arg marker "$MARKER" \
  --arg meta "<!-- metadata:${metadata} -->" \
  '{body: ($marker + "\n" + $meta + "\n### tofu plan\n\n```terraform\n" + $plan + "\n```")}' \
  <<< "")

existing_id=$(curl -sSf "${AUTH[@]}" \
  "${API}/issues/${PR_NUMBER}/comments" \
  | jq -r --arg marker "$MARKER" \
    '[.[] | select(.body | contains($marker))] | first | .id // empty')

if [ -n "$existing_id" ]; then
  echo "Updating existing comment ${existing_id}"
  curl -sSf -X PATCH "${AUTH[@]}" -d "$body" \
    "${API}/issues/comments/${existing_id}" >/dev/null
else
  echo "Posting new comment"
  curl -sSf -X POST "${AUTH[@]}" -d "$body" \
    "${API}/issues/${PR_NUMBER}/comments" >/dev/null
fi
