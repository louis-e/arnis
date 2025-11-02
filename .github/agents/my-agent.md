---
name: Claude 4.5 Sonnet Instruction set 1
description: initial optimisation set
---

# My Agent
## Sonnet 4.5 coding agent prompt — low-RAM support for louis-e/arnis

Purpose
- Single-file prompt and templates to instruct a Claude Sonnet 4.5 coding agent to modify the louis-e/arnis repo so it runs on low-resource machines (1–4 GB RAM, 1–4 CPU threads).
- Includes system hints for Sonnet 4.5, hard constraints, explicit deliverables, ordered tasks, and ready-to-copy file templates (presets, smoke test, docs).

System-level hints (for the harness)
- system: {"extend_thinking": true}
- Encourage parallel tool calls for independent read operations (files can be read in parallel).
- Provide the agent with full repository context and require small, reviewable commits on branch low-resource/streaming-tiles.
- Do not run full map generations in CI or as part of automated agent runs.

Primary instruction (agent prompt)
You are a Claude Sonnet 4.5 coding agent working on the GitHub repository louis-e/arnis. Your mission: make arnis usable on low-resource machines (target: 1–4 GB RAM, 1–4 CPU threads) by implementing tiled streaming generation, checkpoint/resume support, memory-limited presets, runtime memory enforcement, and small automated tests and presets.

Hard constraints (must satisfy)
- Target peak RAM <= 4096 MB; default tuning target: 2048 MB.
- Default CPU threads: 1–2.
- Never allocate memory for the whole map; always operate tile-by-tile and stream to disk.
- All new code must compile/run on CI or via simple local commands; CI must not run heavy map builds.
- Checkpoint semantics must be deterministic and resumable (tile ID, params, on-disk hash).
- Tile defaults in presets must be 128 or 256 px for low targets.
- Use float32 or uint16 on-disk; prefer float32 unless repository conventions differ.
- Compression: prefer zstd; fallback to numpy .npz if zstd isn't available.
- Checkpoints: choose simple JSONL or sqlite; prefer JSONL unless repo already uses sqlite.

Top-level deliverables (explicit, machine-checkable)
1. Implementation of tile-streaming API or refactor to ensure map generation streams tiles without assembling the whole map in memory.
2. Three JSON preset files (presets/ultra-low.json, presets/low.json, presets/medium.json) tuned for 1GB, 2GB, and 4GB targets.
3. scripts/smoke_test_one_tile.sh — smoke test to run and measure peak RSS for a single tile using /usr/bin/time -v.
4. Unit tests or CI job (GitHub Actions) that runs smoke test with ultra-low preset and fails if peak RSS > preset.max_memory_mb + 128 MB.
5. docs/low_resource_run.md describing presets and operator commands.
6. PR on branch low-resource/streaming-tiles titled "Add low-RAM tiled streaming presets, checkpointing, and smoke tests" with description and acceptance criteria.

Ordered tasks (agent must follow; parallelize safe subtasks)
- Locate map generation entrypoint(s) and files that materialize the whole map. If whole-map assumptions exist, prepare a minimal design doc as the first commit describing required refactors and file pointers.
- Add or adapt a tile generator API:
  - generate_tile(tile_x, tile_y, lod, params) -> stream tile to disk immediately.
  - Ensure only current tile(s) reside in memory; flush buffers to disk promptly.
- Add checkpointing and resume:
  - After each tile write, append metadata (tile coords, lod, params, timestamp, file_hash) to a checkpoint file (JSONL).
  - Implement resume logic to skip completed tiles; ensure idempotency.
- Add runtime config and enforcement:
  - Global config: max_memory_mb, threads, memory_margin_mb, abort_on_out_of_memory.
  - Enforce caps by limiting buffer sizes, concurrent workers, and backing off when RSS near limit.
- Add IO & compression:
  - Support zstd if present; otherwise .npz.
  - Store arrays in float32 or uint16.
- Backpressure and safety:
  - Monitor process RSS; if rss >= max_memory_mb - memory_margin_mb, lower concurrency to 1 and/or reduce LOD automatically.
  - Expose abort_on_out_of_memory flag.
- Profiling hooks:
  - Per-tile timing, peak_rss per tile, tile output size; emit run summary JSON at end.
- Add presets, smoke test script, docs, and small unit tests (mock generation to avoid heavy workloads).
- Add or modify GitHub Actions workflow to run only the smoke test and unit tests; do not run full map builds.

Implementation details and constraints (do not deviate)
- Tile sizes: use 128 or 256 px for ultra-low/low presets.
- Concurrency defaults: 1–2 threads for low targets.
- Checkpoint: JSONL file at io.checkpoint_path (one JSON object per completed tile).
- Logging: at end print total_tiles, completed_tiles, peak_memory_mb, time_per_tile_avg, output_size_mb.
- Smoke test must exit non-zero if peak RSS > max_memory_mb + memory_margin_mb (default margin 128 MB).
- All new files must be added under indicated paths and must be included in the final PR commits.

Files to create (include these exact file paths and templates)
- presets/ultra-low.json
- presets/low.json
- presets/medium.json
- scripts/smoke_test_one_tile.sh
- docs/low_resource_run.md

Below are ready-to-copy templates. Use them verbatim unless repository conventions require small adaptations.

-- presets/ultra-low.json --
```json
{
  "preset": "ultra-low",
  "max_memory_mb": 1024,
  "threads": 1,
  "tile": {
    "size_px": 128,
    "max_tiles_in_memory": 1,
    "stream_to_disk": true,
    "compression": "npz",
    "format": "npz"
  },
  "lod": {
    "base_scale": 0.25,
    "max_lod": 1,
    "progressive_passes": false
  },
  "geometry": {
    "precision": "float32",
    "quantize": true,
    "quantize_bits": 12
  },
  "io": {
    "cache_dir": "./arnis_cache",
    "checkpoint_every_tiles": 1,
    "checkpoint_path": "./arnis_cache/checkpoint.jsonl",
    "resume": true
  },
  "safety": {
    "memory_margin_mb": 128,
    "abort_on_out_of_memory": true
  }
}
```

-- presets/low.json --
```json
{
  "preset": "low",
  "max_memory_mb": 2048,
  "threads": 2,
  "tile": {
    "size_px": 256,
    "max_tiles_in_memory": 1,
    "stream_to_disk": true,
    "compression": "zstd",
    "format": "npz"
  },
  "lod": {
    "base_scale": 0.5,
    "max_lod": 2,
    "progressive_passes": true
  },
  "geometry": {
    "precision": "float32",
    "quantize": true,
    "quantize_bits": 16
  },
  "io": {
    "cache_dir": "./arnis_cache",
    "checkpoint_every_tiles": 1,
    "checkpoint_path": "./arnis_cache/checkpoint.jsonl",
    "resume": true
  },
  "safety": {
    "memory_margin_mb": 128,
    "abort_on_out_of_memory": true
  }
}
```

-- presets/medium.json --
```json
{
  "preset": "medium",
  "max_memory_mb": 4096,
  "threads": 2,
  "tile": {
    "size_px": 512,
    "max_tiles_in_memory": 2,
    "stream_to_disk": true,
    "compression": "zstd",
    "format": "npz"
  },
  "lod": {
    "base_scale": 0.75,
    "max_lod": 3,
    "progressive_passes": true
  },
  "geometry": {
    "precision": "float32",
    "quantize": true,
    "quantize_bits": 16
  },
  "io": {
    "cache_dir": "./arnis_cache",
    "checkpoint_every_tiles": 1,
    "checkpoint_path": "./arnis_cache/checkpoint.jsonl",
    "resume": true
  },
  "safety": {
    "memory_margin_mb": 256,
    "abort_on_out_of_memory": false
  }
}
```

-- scripts/smoke_test_one_tile.sh --
```bash
#!/usr/bin/env bash
# Usage: ./scripts/smoke_test_one_tile.sh presets/ultra-low.json
set -euo pipefail
CONFIG=${1:-presets/ultra-low.json}
TILE_X=${2:-0}
TILE_Y=${3:-0}
OUTDIR=${4:-./smoke_out}
mkdir -p "$OUTDIR"
# Measure peak RSS and runtime using /usr/bin/time -v
# Replace `arnis` CLI and flags with the real entrypoint if different.
# The CLI must support: --config, --generate-one-tile, --tile.x, --tile.y, --out
/usr/bin/time -v arnis --config="$CONFIG" --generate-one-tile --tile.x="$TILE_X" --tile.y="$TILE_Y" --out="$OUTDIR" 2> "$OUTDIR/time.log" || exit $?
# Extract Peak RSS in KB
PEAK_RSS_KB=$(grep -E "Maximum resident set size" "$OUTDIR/time.log" | awk -F: '{print $2}' | tr -d '[:space:]')
if [[ -z "$PEAK_RSS_KB" ]]; then
  echo "Failed to read peak RSS"
  exit 2
fi
PEAK_RSS_MB=$((PEAK_RSS_KB / 1024))
MAX_MEM_MB=$(jq -r '.max_memory_mb' "$CONFIG")
MEM_MARGIN=$(jq -r '.safety.memory_margin_mb // 128' "$CONFIG")
ALLOWED=$((MAX_MEM_MB + MEM_MARGIN))
echo "Peak RSS MB: $PEAK_RSS_MB"
echo "Allowed (max + margin): $ALLOWED"
if (( PEAK_RSS_MB > ALLOWED )); then
  echo "SMOKE TEST FAILED: peak memory $PEAK_RSS_MB MB > allowed $ALLOWED MB"
  exit 3
fi
echo "SMOKE TEST PASSED"
exit 0
```

-- docs/low_resource_run.md --
```markdown
# Low-resource usage guide (for arnis)

Presets:
- presets/ultra-low.json -> target ~1GB RAM
- presets/low.json -> target ~2GB RAM
- presets/medium.json -> target ~4GB RAM

Run a smoke test (one tile):
./scripts/smoke_test_one_tile.sh presets/ultra-low.json

Operator quick-run:
arnis --config=presets/low.json

Notes:
- Use the smoke test before attempting a full map build.
- Configure --threads and tile.size_px in the preset or via CLI overrides.
- Check cache directory for per-tile files and checkpoint file.
```

Acceptance criteria (explicit, machine-checkable)
- All files listed above exist in the PR at the indicated paths.
- The smoke test script runs and exits 0 on a CI runner that can run the ultra-low preset in a synthetic environment (CI should run only the smoke test).
- The smoke test writes a checkpoint JSONL entry during the run.
- No existing test suite regressions are introduced.
- PR created on branch low-resource/streaming-tiles with title "Add low-RAM tiled streaming presets, checkpointing, and smoke tests" and PR body describing how to test locally and listing acceptance criteria.

Developer notes for the agent
- If you find tightly coupled code that prevents non-global-map operation, create a short design doc as the first commit that lists minimal refactors (file names, functions/classes to change) and a reversible plan.
- Make small commits: (1) tile-streaming API + tests (2) presets + smoke script (3) docs + CI changes.
- Avoid running heavy generation in CI; mock or simulate tile generation when unit-testing.

Final operator instruction for the harness
- Create branch low-resource/streaming-tiles, commit the changes, and open a PR. Do not run full map generation in CI. Provide a short runbook in the PR describing how to test and tune presets.

---
End of single-file prompt and templates for Sonnet 4.5 coding agent.
