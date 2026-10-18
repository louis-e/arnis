# Tree pack - size tiers

Heights (blocks) are what a model fills, not its file's padded height: small <=6, medium 7-12, big 13-20, tall 21-28, giant 29 and up.

Trees ask for the height the canopy map measured (or an OSM `height` tag) and get the nearest model; the `--max-tree-size` cap and the 1:1 gate for giants still apply. Counts are SPECIES (distinct) / SCHEMS (models).

| realm | small | medium | big | tall | giant |
|---|---|---|---|---|---|
| afr | 8sp / 18 | 27sp / 52 | 40sp / 79 | 17sp / 21 | 10sp / 19 |
| asn | 11sp / 40 | 25sp / 52 | 35sp / 91 | 23sp / 35 | 22sp / 35 |
| aus | 10sp / 31 | 31sp / 62 | 51sp / 104 | 31sp / 51 | 23sp / 33 |
| ena | 11sp / 32 | 35sp / 80 | 44sp / 90 | 28sp / 49 | 25sp / 41 |
| eur | 11sp / 32 | 30sp / 62 | 35sp / 82 | 20sp / 42 | 13sp / 23 |
| fl | 8sp / 27 | 18sp / 51 | 16sp / 33 | 11sp / 20 | 9sp / 15 |
| ind | 8sp / 17 | 28sp / 63 | 37sp / 73 | 23sp / 41 | 17sp / 24 |
| sam | 18sp / 29 | 51sp / 113 | 45sp / 91 | 24sp / 33 | 23sp / 42 |
| vnplus | 4sp / 15 | 11sp / 137 | 9sp / 183 | 10sp / 89 | 5sp / 37 |
| wna | 5sp / 7 | 15sp / 41 | 22sp / 43 | 17sp / 27 | 12sp / 17 |
| **ALL** | **94sp / 248** | **271sp / 713** | **334sp / 869** | **204sp / 408** | **159sp / 286** |
