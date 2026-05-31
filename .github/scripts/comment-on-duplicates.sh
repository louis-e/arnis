#!/usr/bin/env bash
# Posts a fixed "possible duplicates" comment on the triggering issue. Args: 1-3 issue numbers.
# The base issue is read from the event payload, so a hijacked prompt can't retarget it.
set -euo pipefail

repo="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY not set}"
base="$(jq -r '.issue.number // .inputs.issue_number // empty' "${GITHUB_EVENT_PATH:?GITHUB_EVENT_PATH not set}")"
[[ "$base" =~ ^[0-9]+$ ]] || { echo "no triggering issue number in event payload" >&2; exit 1; }

(($# >= 1 && $# <= 3)) || { echo "pass 1-3 duplicate issue numbers" >&2; exit 1; }
for d in "$@"; do
  [[ "$d" =~ ^[0-9]+$ ]] || { echo "not an issue number: $d" >&2; exit 1; }
done

if (($# == 1)); then header="Found 1 possible duplicate issue:"; else header="Found $# possible duplicate issues:"; fi
body="$header"$'\n\n'
i=1
for d in "$@"; do
  body+="${i}. https://github.com/${repo}/issues/${d}"$'\n'
  ((i++))
done

gh issue comment "$base" --repo "$repo" --body "$body"
