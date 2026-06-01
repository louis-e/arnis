#!/usr/bin/env bash
# Read-only gh wrapper for the issue bot: issue/PR/release lookups and searches only.
set -euo pipefail
case "${1:-} ${2:-}" in
  "issue view" | "issue list" | "search issues" \
  | "pr view" | "pr list" | "search prs" \
  | "release view" | "release list")
    exec gh "$@"
    ;;
  *)
    echo "dedupe-gh.sh: read-only lookup not allowed: $*" >&2
    exit 1
    ;;
esac
