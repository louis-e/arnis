# Pipeline Operations

## Test Run Results

### Run: Times Square, New York City

- **Date:** 2026-04-26
- **Location:** Times Square, NYC
- **Bounding box:** `40.7570,-73.9870,40.7600,-73.9840`
- **Edition:** Java (Anvil format)
- **Scale:** 1.0 (default, 1 block per meter)
- **Ground level:** -62 (default)
- **Build mode:** `--release` (LTO thin, overflow checks enabled)
- **Features:** `--no-default-features` (CLI-only, no GUI/Bedrock)

### Generation Metrics

| Metric | Value |
|---|---|
| Total wall time | ~19 seconds |
| World dimensions | 252 x 333 blocks (X x Z) |
| Region files | 1 (`r.0.0.mca`) |
| Region file size | 6.3 MB |
| Total output size | 7.0 MB |
| OSM elements parsed | 2,326 |
| Overture extra buildings | 0 |

### Pipeline Stages

1. **Fetching data** - Downloaded OSM data via Overpass API (fallback endpoint used after rate limit on primary)
2. **Parsing data** - Parsed 2,326 OSM elements
3. **Overture Maps** - Scanned 1 STAC partition, downloaded 4.3 MB of Overture data, found 0 additional non-OSM buildings
4. **Transforming map** - Applied coordinate transformation
5. **Processing terrain** - Processed terrain features
6. **Generating ground** - Generated ground layer
7. **Saving world** - Wrote Anvil region files and world metadata

### Output Validation

- Region file `r.0.0.mca`: 6,340,608 bytes, valid Anvil header with chunk location table
- `metadata.json`: Correct geographic bounds matching input bbox
- `level.dat`: Present (Minecraft world metadata)
- `icon.png`: Present (world thumbnail)

### Observations

- The primary Overpass API endpoint (`api.arnismc.com`) returned a rate limit on first attempt; the pipeline automatically retried using the fallback endpoint (`z.overpass-api.de`), which succeeded.
- For this small bounding box, all generated chunks fit within a single region file (`r.0.0.mca`).
- No terrain elevation was requested (`--terrain` not set), so the world is flat at ground level -62.
- The Overture Maps integration downloaded 4.3 MB of parquet data but found no additional buildings beyond what OSM already provides for Times Square.
