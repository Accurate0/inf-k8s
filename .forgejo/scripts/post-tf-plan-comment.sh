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
: "${PLAN_EXIT_CODE:=0}"

MARKER='<!-- terraform-plan -->'
API="${SERVER_URL}/api/v1/repos/${REPO}"
AUTH=(-H "Authorization: token ${FORGEJO_TOKEN}" -H "Content-Type: application/json")

max=60000
plan=$(cat "$PLAN_FILE")
truncated=""
if [ ${#plan} -gt $max ]; then
  plan="${plan:0:$max}"$'\n\n... (truncated)'
  truncated=" (truncated)"
fi

summary_line=$(grep -E "^(Plan:|No changes\.|Apply complete!|Error:)" "$PLAN_FILE" | tail -n 1 || true)
if [ -z "$summary_line" ]; then
  summary_line="Plan output below"
fi

if [ "$PLAN_EXIT_CODE" = "0" ]; then
  status_text="Success"
else
  status_text="Failed"
fi

metadata=$(jq -nc \
  --arg run_id "$RUN_ID" \
  --arg run_url "$RUN_URL" \
  --arg sha "$COMMIT_SHA" \
  --arg exit "$PLAN_EXIT_CODE" \
  '{run_id: $run_id, run_url: $run_url, sha: $sha, exit_code: $exit}')

short_sha="${COMMIT_SHA:0:7}"

body=$(jq -Rs \
  --arg marker "$MARKER" \
  --arg meta "<!-- metadata:${metadata} -->" \
  --arg status_text "$status_text" \
  --arg summary "$summary_line" \
  --arg short_sha "$short_sha" \
  --arg run_url "$RUN_URL" \
  --arg truncated "$truncated" \
  --arg plan "$plan" \
  '{body: (
    $marker + "\n" +
    $meta + "\n" +
    "## Terraform Plan\n\n" +
    "> " + $status_text + " for `" + $short_sha + "` — [run log](" + $run_url + ")\n\n" +
    "> " + $summary + "\n\n" +
    "<details>\n<summary>Plan output" + $truncated + "</summary>\n\n" +
    "```terraform\n" + $plan + "\n```\n" +
    "</details>\n"
  )}' \
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
