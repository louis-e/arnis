#!/usr/bin/env bash
# Golden world-hash harness for generation changes.
#
# Runs deterministic fixture generations (committed .osm.gz files, flat ground,
# no 3D models, no Overture, no canopy download) and compares the
# ARNIS_BLOCK_HASH world hash against tests/golden_hashes.txt.
#
# Usage:
#   scripts/golden_hash.sh              # verify all fixtures against the manifest
#   scripts/golden_hash.sh --update    # rebaseline the manifest (intentional visual change)
#   scripts/golden_hash.sh munich_altstadt levittown   # subset
#
# ARNIS_BIN overrides the binary (default target/release/arnis[.exe]).
# Note: the first run may download land-cover tiles into the cache; subsequent
# runs are offline. The world hash covers placed blocks only, so it is stable
# across machines for identical inputs.
set -euo pipefail
cd "$(dirname "$0")/.."

MANIFEST="tests/golden_hashes.txt"
FIXDIR="tests/fixtures"
BIN="${ARNIS_BIN:-}"
if [[ -z "$BIN" ]]; then
    if [[ -x target/release/arnis.exe ]]; then BIN=target/release/arnis.exe
    elif [[ -x target/release/arnis ]]; then BIN=target/release/arnis
    else echo "error: build target/release/arnis first (cargo build --release)"; exit 1
    fi
fi

UPDATE=0
FIXTURES=()
for arg in "$@"; do
    case "$arg" in
        --update) UPDATE=1 ;;
        *) FIXTURES+=("$arg") ;;
    esac
done
if [[ ${#FIXTURES[@]} -eq 0 ]]; then
    for f in "$FIXDIR"/*.osm.gz; do
        [[ -e "$f" ]] || continue
        name="$(basename "$f" .osm.gz)"
        FIXTURES+=("$name")
    done
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

declare -A EXPECTED
if [[ -f "$MANIFEST" ]]; then
    while IFS=$'\t' read -r name hash; do
        [[ -n "$name" && "${name:0:1}" != "#" ]] && EXPECTED[$name]="$hash"
    done < "$MANIFEST"
fi

FAIL=0
RESULTS=()
for name in "${FIXTURES[@]}"; do
    gz="$FIXDIR/$name.osm.gz"
    if [[ ! -f "$gz" ]]; then echo "SKIP  $name (no fixture $gz)"; continue; fi
    gunzip -c "$gz" > "$TMP/$name.osm"
    mkdir -p "$TMP/world_$name"
    log="$TMP/$name.log"
    if ! ARNIS_BLOCK_HASH=1 "$BIN" \
        --file "$TMP/$name.osm" \
        --output-dir "$TMP/world_$name" \
        --mode geo-only --no-3d --overture=false --canopy-height=false \
        >"$log" 2>&1; then
        echo "ERROR $name (generation failed, log: $log)"; FAIL=1
        tail -5 "$log"; continue
    fi
    hash="$(grep -o 'block_hash=[0-9a-f]*' "$log" | tail -1 | cut -d= -f2)"
    if [[ -z "$hash" ]]; then echo "ERROR $name (no block_hash in output)"; FAIL=1; continue; fi
    RESULTS+=("$name	$hash")
    if [[ $UPDATE -eq 1 ]]; then
        echo "BASE  $name $hash"
    elif [[ "${EXPECTED[$name]:-}" == "$hash" ]]; then
        echo "OK    $name $hash"
    else
        echo "DIFF  $name got=$hash want=${EXPECTED[$name]:-<none>}"; FAIL=1
    fi
done

if [[ $UPDATE -eq 1 ]]; then
    {
        echo "# Golden world hashes (scripts/golden_hash.sh). Regenerate with --update."
        printf '%s\n' "${RESULTS[@]}"
    } > "$MANIFEST"
    echo "manifest updated: $MANIFEST"
    exit 0
fi
exit $FAIL
