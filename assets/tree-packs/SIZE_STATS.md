# Tree pack - size tiers

Heights (blocks) are what a model fills, not its file's padded height: small <=6, medium 7-12, big 13-20, tall 21-28, giant 29 and up.

Trees ask for the height the canopy map measured (or an OSM `height` tag) and get the nearest model; the `--max-tree-size` cap and the 1:1 gate for giants still apply. Counts are SPECIES (distinct) / SCHEMS (models).

| realm | small | medium | big | tall | giant |
|---|---|---|---|---|---|
| afr | 48sp / 91 | 46sp / 81 | 40sp / 79 | 17sp / 21 | 10sp / 19 |
| asn | 49sp / 97 | 44sp / 93 | 35sp / 91 | 23sp / 35 | 22sp / 35 |
| aus | 81sp / 158 | 79sp / 136 | 51sp / 104 | 31sp / 51 | 23sp / 33 |
| ena | 69sp / 137 | 65sp / 139 | 44sp / 90 | 28sp / 49 | 25sp / 41 |
| eur | 48sp / 89 | 44sp / 97 | 35sp / 82 | 20sp / 42 | 13sp / 23 |
| fl | 29sp / 63 | 27sp / 69 | 16sp / 33 | 11sp / 20 | 9sp / 15 |
| ind | 69sp / 131 | 65sp / 117 | 37sp / 73 | 23sp / 41 | 17sp / 24 |
| sam | 87sp / 157 | 86sp / 167 | 45sp / 91 | 24sp / 33 | 23sp / 42 |
| vnplus | 12sp / 69 | 11sp / 142 | 9sp / 183 | 10sp / 89 | 5sp / 37 |
| wna | 30sp / 45 | 31sp / 72 | 22sp / 43 | 17sp / 27 | 12sp / 17 |
| **ALL** | **522sp / 1037** | **498sp / 1113** | **334sp / 869** | **204sp / 408** | **159sp / 286** |
