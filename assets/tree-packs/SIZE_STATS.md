# Tree pack - size tiers

Heights (blocks) are what a model fills, not its file's padded height: small <=6, medium 7-12, big 13-20, tall 21-28, giant 29 and up.

Trees ask for the height the canopy map measured (or an OSM `height` tag) and get the nearest model; the `--max-tree-size` cap and the 1:1 gate for giants still apply. Counts are SPECIES (distinct) / SCHEMS (models).

| realm | small | medium | big | tall | giant |
|---|---|---|---|---|---|
| afr | 48sp / 98 | 27sp / 52 | 40sp / 79 | 17sp / 21 | 10sp / 19 |
| asn | 49sp / 116 | 25sp / 52 | 35sp / 91 | 23sp / 35 | 22sp / 35 |
| aus | 81sp / 173 | 31sp / 62 | 51sp / 104 | 31sp / 51 | 23sp / 33 |
| ena | 69sp / 148 | 35sp / 80 | 44sp / 90 | 28sp / 49 | 25sp / 41 |
| eur | 48sp / 106 | 30sp / 62 | 35sp / 82 | 20sp / 42 | 13sp / 23 |
| fl | 29sp / 69 | 18sp / 51 | 16sp / 33 | 11sp / 20 | 9sp / 15 |
| ind | 69sp / 139 | 28sp / 63 | 37sp / 73 | 23sp / 41 | 17sp / 24 |
| sam | 87sp / 167 | 51sp / 113 | 45sp / 91 | 24sp / 33 | 23sp / 42 |
| vnplus | 12sp / 73 | 11sp / 137 | 9sp / 183 | 10sp / 89 | 5sp / 37 |
| wna | 30sp / 57 | 15sp / 41 | 22sp / 43 | 17sp / 27 | 12sp / 17 |
| **ALL** | **522sp / 1146** | **271sp / 713** | **334sp / 869** | **204sp / 408** | **159sp / 286** |
