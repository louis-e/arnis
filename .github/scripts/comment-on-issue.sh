#!/usr/bin/env bash
# Posts one bot comment on the triggering issue: an optional grounded resolution note and/or
# up to 3 duplicate links, then a close hint. Posts nothing if both are empty.
#
# Usage: comment-on-issue.sh [--resolution "<text>"] [--duplicates <n1> [n2] [n3]]
# The target issue is read from the event payload, so the prompt can't retarget it.
set -euo pipefail

repo="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY not set}"
base="$(jq -r '.issue.number // .inputs.issue_number // empty' "${GITHUB_EVENT_PATH:?GITHUB_EVENT_PATH not set}")"
[[ "$base" =~ ^[0-9]+$ ]] || { echo "no triggering issue number in event payload" >&2; exit 1; }

dups=()
resolution=""
while (($#)); do
  case "$1" in
    --duplicates)
      shift
      while (($#)) && [[ "$1" != --* ]]; do
        [[ "$1" =~ ^[0-9]+$ ]] || { echo "not an issue number: $1" >&2; exit 1; }
        dups+=("$1"); shift
      done
      ;;
    --resolution)
      shift
      # Only take the next arg if it's a real value, not another flag (or end of args).
      if (($#)) && [[ "$1" != --* ]]; then
        resolution="$1"; shift
      fi
      ;;
    *)
      # Tolerate bare issue numbers as duplicates; reject anything else.
      if [[ "$1" =~ ^[0-9]+$ ]]; then dups+=("$1"); shift
      else echo "unknown argument: $1" >&2; exit 1; fi
      ;;
  esac
done

# Drop the triggering issue and repeats from duplicates.
filtered=()
for d in "${dups[@]-}"; do
  [[ -z "$d" || "$d" == "$base" ]] && continue
  [[ " ${filtered[*]-} " == *" $d "* ]] && continue
  filtered+=("$d")
done
((${#filtered[@]} <= 3)) || { echo "at most 3 duplicates" >&2; exit 1; }

# Sanitize free-text resolution: strip control chars (keep tab/newline/CR), neutralize @mentions
# with a zero-width space, cap length.
if [[ -n "$resolution" ]]; then
  resolution="$(printf '%s' "$resolution" | tr -d '\000-\010\013\014\016-\037')"
  zwsp=$'\xe2\x80\x8b'
  resolution="${resolution//@/@$zwsp}"
  ((${#resolution} > 1000)) && resolution="${resolution:0:1000}…"
fi

if ((${#filtered[@]} == 0)) && [[ -z "$resolution" ]]; then
  echo "nothing grounded to post; doing nothing"
  exit 0
fi

body=""
[[ -n "$resolution" ]] && body+="$resolution"$'\n\n'
if ((${#filtered[@]} > 0)); then
  if ((${#filtered[@]} == 1)); then body+="Found 1 possible duplicate issue:"$'\n\n'
  else body+="Found ${#filtered[@]} possible duplicate issues:"$'\n\n'; fi
  i=1
  for d in "${filtered[@]}"; do body+="${i}. https://github.com/${repo}/issues/${d}"$'\n'; ((i++)); done
  body+=$'\n'
fi
body+="If this resolves your issue, please close it."$'\n\n'
body+="_🤖 Automated triage — may be inaccurate; a maintainer will follow up._"

gh issue comment "$base" --repo "$repo" --body "$body"
