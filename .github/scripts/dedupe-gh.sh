#!/usr/bin/env bash
# Read-only gh wrapper for the duplicate finder: only issue lookups and searches are allowed.
set -euo pipefail
case "${1:-} ${2:-}" in
  "issue view" | "issue list" | "search issues")
    exec gh "$@"
    ;;
  *)
    echo "dedupe-gh.sh: only 'issue view', 'issue list', 'search issues' allowed (got: $*)" >&2
    exit 1
    ;;
esac
