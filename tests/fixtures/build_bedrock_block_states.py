"""Writes tests/fixtures/bedrock_block_states.json from a Bedrock block palette.

The palette is minecraft-data's blockStates.json for one Bedrock version, e.g.
https://raw.githubusercontent.com/PrismarineJS/minecraft-data/master/data/bedrock/1.26.30/blockStates.json
For every block it keeps the state names with their type and the values seen, which
is what src/bedrock_block_map.rs is tested against.

    python3 tests/fixtures/build_bedrock_block_states.py blockStates.json 1.26.30
"""
import json
import sys

palette_path, version = sys.argv[1], sys.argv[2]
palette = json.load(open(palette_path))
blocks = {}
for entry in palette:
    states = blocks.setdefault("minecraft:" + entry["name"], {})
    for key, spec in entry["states"].items():
        slot = states.setdefault(key, {"type": spec["type"], "values": set()})
        slot["values"].add(spec["value"])

out = {
    "version": version,
    "block_state_version": sorted({e["version"] for e in palette}),
    "blocks": {
        name: {
            key: {"type": spec["type"], "values": sorted(spec["values"], key=lambda v: (str(type(v)), v))}
            for key, spec in sorted(states.items())
        }
        for name, states in sorted(blocks.items())
    },
}
with open("tests/fixtures/bedrock_block_states.json", "w") as f:
    json.dump(out, f, separators=(",", ":"), sort_keys=False)
    f.write("\n")
