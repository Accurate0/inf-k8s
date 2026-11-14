#!/usr/bin/env bash
# Import Accurate0 GitHub repos into git.anurag.sh and push-mirror them back to GitHub.
#   forgejo-import.sh [--dry-run]
#   forgejo-import.sh --disable-gh-actions <repo>...
set -euo pipefail

# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"

forgejo="https://git.anurag.sh/api/v1"
owner="anurag"
github_owner="Accurate0"

if [ "${1:-}" = "--disable-gh-actions" ]; then
  shift
  for repo in "$@"; do
    gh api -X PUT "repos/$github_owner/$repo/actions/permissions" -F enabled=false
    info "disabled github actions on $github_owner/$repo"
  done
  exit 0
fi

dry_run=false
[ "${1:-}" = "--dry-run" ] && dry_run=true

: "${FORGEJO_TOKEN:?FORGEJO_TOKEN required (write:repository, read:user)}"
: "${GITHUB_TOKEN:=$(gh auth token)}"
$dry_run || : "${MIRROR_TOKEN:?MIRROR_TOKEN required (github PAT, contents rw)}"

fj() {
  local method=$1 path=$2
  shift 2
  curl -fsS --max-time 0 -X "$method" \
    -H "Authorization: token $FORGEJO_TOKEN" \
    -H "Content-Type: application/json" \
    "$forgejo$path" "$@"
}

existing=$(
  page=1
  while :; do
    batch=$(fj GET "/user/repos?limit=50&page=$page")
    [ "$(jq length <<<"$batch")" -eq 0 ] && break
    jq -r '.[] | select(.owner.login == "'"$owner"'") | .name' <<<"$batch"
    page=$((page + 1))
  done
)

mirrored=$(
  for repo in $existing; do
    fj GET "/repos/$owner/$repo/push_mirrors" |
      jq -r '.[].remote_address' |
      sed -nE "s#.*github\.com[:/]$github_owner/([^/]+)\$#\1#p" |
      sed 's/\.git$//'
  done
)

github_repos=$(gh repo list "$github_owner" --limit 500 \
  --json name,isPrivate,isArchived,isFork,description,repositoryTopics \
  --jq '[.[] | select(.isFork | not)]')

migrated=() skipped=() failed=()

while read -r repo; do
  name=$(jq -r .name <<<"$repo")

  if grep -qixF "$name" <<<"$existing"$'\n'"$mirrored"; then
    skipped+=("$name")
    continue
  fi

  private=$(jq .isPrivate <<<"$repo")
  archived=$(jq .isArchived <<<"$repo")

  if $dry_run; then
    info "would migrate $name (private=$private archived=$archived)"
    migrated+=("$name")
    continue
  fi

  info "migrating $name"

  migrate=$(jq -n --argjson repo "$repo" --arg owner "$owner" --arg token "$GITHUB_TOKEN" '{
    clone_addr: "https://github.com/'"$github_owner"'/\($repo.name).git",
    auth_token: $token,
    service: "github",
    repo_owner: $owner,
    repo_name: $repo.name,
    private: $repo.isPrivate,
    description: ($repo.description // ""),
    mirror: false,
    releases: true,
    issues: false,
    pull_requests: false,
    labels: false,
    milestones: false,
    wiki: false
  }')

  if ! fj POST /repos/migrate -d "$migrate" >/dev/null; then
    err "migrate failed: $name"
    failed+=("$name")
    continue
  fi

  fj PATCH "/repos/$owner/$name" -d '{
    "has_issues": true,
    "has_pull_requests": true,
    "has_wiki": true,
    "has_projects": true,
    "has_releases": true,
    "has_packages": true,
    "has_actions": true
  }' >/dev/null

  topics=$(jq '{topics: [.repositoryTopics // [] | .[].name]}' <<<"$repo")
  fj PUT "/repos/$owner/$name/topics" -d "$topics" >/dev/null

  if [ "$archived" = "true" ]; then
    fj PATCH "/repos/$owner/$name" -d '{"archived": true}' >/dev/null
  else
    mirror=$(jq -n --arg name "$name" --arg token "$MIRROR_TOKEN" '{
      remote_address: "https://github.com/'"$github_owner"'/\($name).git",
      remote_username: "'"$github_owner"'",
      remote_password: $token,
      interval: "8h0m0s",
      sync_on_commit: true
    }')
    if ! fj POST "/repos/$owner/$name/push_mirrors" -d "$mirror" >/dev/null; then
      warn "push mirror failed: $name"
      failed+=("$name (mirror)")
    fi
  fi

  migrated+=("$name")
done < <(jq -c '.[]' <<<"$github_repos")

printf '\n'
info "migrated: ${#migrated[@]}"
info "skipped:  ${#skipped[@]} (${skipped[*]:-})"
[ "${#failed[@]}" -eq 0 ] || err "failed: ${failed[*]}"
