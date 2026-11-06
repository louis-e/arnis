# Planetary geology grid

`features.grid.gz` is a derived, deterministic overlay for the existing Moon
and Mars elevation pipeline. It is a gzip-compressed 3600 × 1800 raster of
little-endian `u16` flags. Cell centers are spaced 0.1° apart, starting at
90°N, 180°E; longitudes increase eastward and wrap at the dateline. The grid
is an index into mapped geology, not a new elevation model. At the equator a
cell is about 3.0 km across on the Moon and 5.9 km on Mars; east-west spacing
contracts with latitude. Missing, malformed, or out-of-range lookups return no
flags.

## Sources and coverage

| Overlay | Source and mapped content | Source scale / resolution and coverage | Coordinates and terms |
| --- | --- | --- | --- |
| Lunar maria, volcanic channels (rilles), crater rims, ridges and basin rings | [USGS Unified Geologic Map of the Moon, v2](https://astrogeology.usgs.gov/search/map/Moon/Geology/Unified_Geologic_Map_of_the_Moon_GIS_v2/). Mare units come from the `GeoUnits` polygons; line overlays use the map's `Linear_Features` classes `Channel (volcanic)`, `Crest of crater rim`, `Ridge crest` and `Basin ring`. | Global, 1:5,000,000 synthesis. Its vectors were drafted against Kaguya terrain imagery and LOLA polar relief; the source metadata reports ~3 km vertex spacing and up to ~16 km smoothing tolerance. The runtime grid samples them at 0.1°, so it does not preserve finer cartographic detail. | Source shapefiles use Moon2000 Equidistant Cylindrical, 1,737,400 m radius. They are converted to planetocentric latitude and positive-east longitude before rasterization. CC0; cite Fortezzo, Spudis and Harrel (2020). |
| Martian volcanic units, polar units, mapped fluvial structures and rilles | [USGS Global Geologic Map of Mars, SIM 3292](https://pubs.usgs.gov/publication/sim3292), via the [USGS Astrogeology units service](https://astrogeology.usgs.gov/pygeoapi/collections/mars/sim3292_global_geologic_map/units/items) and [structure service](https://astrogeology.usgs.gov/pygeoapi/collections/mars/global_geologic_map/structure/items). Polygon attributes select descriptions containing `volcanic` and `polar`; structure attributes select `origin = 'Fluvial'` and `interpreta = 'Rille'`. | Global, 1:20,000,000. Broad units and landforms only; detailed local features may not appear in this global map. The derived grid is 0.1°. | Mars 2000 geographic coordinates, planetocentric latitude and positive-east longitude, normalized to -180°..180°. The USGS marks the map public domain. |
| Martian dune fields | [Mars Global Digital Dune Database (MGD3)](https://www.usgs.gov/centers/astrogeology-science-center/science/mars-global-digital-dune-database), equatorial `Dune_Field` polygons from the USGS Open-File Report 2007-1158 [GIS package](https://pubs.usgs.gov/of/2007/1158/). | 548 moderate-to-large mapped fields in the equatorial inventory, complete from 65°N to 65°S. It was identified mainly from THEMIS infrared images at 100 m/pixel; smaller dune fields are omitted. Dune flags are suppressed beyond ±65°. | Mars 2000 planetary coordinates, positive-east longitude. USGS data release is CC0. |

The exact mask was built from these vector classes and polygon/line coverage,
not from randomly placed points. Overlapping classes keep all applicable
flags. The raw source packages are not bundled; the 285 KB compressed mask
avoids shipping the 224 MB lunar GIS archive or the source dune imagery.

To rebuild the grid, extract the Moon GIS download and the `GML.zip` from
USGS Open-File Report 2007-1158, then pass the directory containing
`GeoUnits.shp` / `Linear_Features.shp` and the extracted `Dune_Field.gml`:

```sh
node assets/planetary/build_features.cjs path/to/moon-gis path/to/Dune_Field.gml
```

The script uses Node's standard library and fetches the Mars map features
from the linked public USGS services. It rasterizes vector outlines at cell
centers and rewrites `features.grid.gz`.

## Existing elevation and feature limits

Arnis already fetches the [NASA PDS LOLA](https://pds-geosciences.wustl.edu/lro/lro-l-lola-3-rdr-v1/lrolol_1xxx/document/archsis.htm)
lunar DEM (128 pixels/degree, about 237 m/pixel at the equator, global ±90°)
and [MOLA MEGDR](https://pds-geosciences.wustl.edu/missions/mgs/megdr.html)
Mars DEM (128 pixels/degree, about 463 m/pixel at the equator, ±88°) through its existing
provider and cache. This change leaves that fetch, resampling, vertical scale,
and fallback behavior intact. These DEMs supply the physical relief for
impact craters, lunar mountain ranges and crater rims, Olympus Mons and the
Tharsis volcanoes, Valles Marineris, valley networks, and broad polar
topography. The overlays add mapped materials/linework to that relief; they
do not claim to resolve details smaller than the source DEM or the 0.1° mask.

Specific limits:

- The lunar global map supplies mapped crater rims and basin rings, while
  LOLA relief supplies crater depressions. No separate ejecta blanket layer is
  included.
- [NASA LRO reports lunar polar locations with *possible* water-ice
  deposits](https://science.nasa.gov/mission/lro/science-and-data/); its LEND
  instrument detects hydrogen through neutron counts rather than providing a
  definitive surface-ice boundary. No lunar ice blocks are generated.
- The global Mars map supports mapped volcanic/polar units and fluvial
  structures, but does not provide a detailed Jezero delta layer. USGS SIM
  3464 maps the Jezero/Nili Planum area at 1:75,000 from a 6 m/pixel CTX
  mosaic, but its GIS database is 349 MB and is not bundled. MOLA still
  provides the broad local relief. Channel palettes use dry sediment/rock
  blocks, never water.
- Mars polar units are generalized global geologic polygons. `MOLA`-based
  overlays are restricted to ±88°, matching the elevation provider's
  coverage.

The resolver filters flags by celestial body, suppresses MGD3 dune flags
outside their equatorial inventory, and returns zero flags when the grid is
unavailable. The block palette remains a deterministic function of mapped
flags, terrain slope/relief and the existing coordinate hash.

