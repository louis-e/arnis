# Where these textures come from

All of them are **CC0**, so they can be redistributed, used commercially, and
need no attribution. Arnis credits them anyway, in License and Credits, because
someone should be able to find the originals.

## Sources

* **Free Urban Textures: Buildings, Apartments, Shop Fronts** by **Scouser**,
  <https://opengameart.org/content/free-urban-textures-buildings-apartments-shop-fronts>.
  The 109 photographs: apartment blocks, offices, shop fronts, warehouses,
  churches, garages and industrial walls.
* **TextureCan**, <https://www.texturecan.com/>, five tiling PBR materials whose
  colour maps are used and whose normal, roughness and ambient occlusion maps
  are not, Minecraft having nothing to drive them with:
  <https://www.texturecan.com/details/315/>,
  <https://www.texturecan.com/details/316/>,
  <https://www.texturecan.com/details/357/>,
  <https://www.texturecan.com/details/360/>,
  <https://www.texturecan.com/details/563/>.

## What was done to them

`build_facades.py` regenerates this directory from the originals: it crops each
photograph to a whole number of window bays where that makes it tile, measures
the seam it achieved, resamples to `PPM` pixels per real metre, and writes JPEG
plus `manifest.json`. The manifest records what each image spans in metres,
which building categories it suits, whether it tiles, and whether it carries a
ground floor that has to sit at the bottom.

Run it with the source folder as the first argument. Nothing here is edited by
hand, so a replacement set is a rebuild rather than a merge.
