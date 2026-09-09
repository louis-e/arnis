"""Rebuild assets/building-facades from a source photograph set.

    python assets/building-facades/build_facades.py [source_dir]

`source_dir` defaults to `buildingfacadetextures/` beside the repository root
and holds the photographs plus the `others_*.zip` PBR sets, whose `color` map
is the only file used (Minecraft has no use for normal, roughness or ao).

TABLE holds one row per source photograph, in source order:
  name, categories, metres_wide, metres_tall, storeys, ground, tile, pre, note

  pre    (l, t, r, b) fractions trimmed before anything else, by eye: quay
         walls, foreground grass, angled balcony returns, downpipes.
  tile   "auto"  run the seam-minimising crop search
         "grad"  flatten the left-right lighting gradient first, then "auto"
         "keep"  already tiles, leave the pixels alone
         "no"    a one-off unit that is placed once, do not crop for tiling
  metres_wide is what the image spans AFTER `pre` and the tiling crop, so it
  is set from the storey count and the post-crop aspect, not from the file.
"""

import glob
import json
import os
import shutil
import sys
import tempfile
import zipfile

import numpy as np
from PIL import Image

Image.MAX_IMAGE_PIXELS = None

DST = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(DST))
SRC = os.path.join(ROOT, "buildingfacadetextures")
if len(sys.argv) > 1:
    SRC = sys.argv[1]
# Filled by unpack_pbr(): the `color` maps out of the five others_*.zip sets.
PBR = ""

# The PBR sets are named by their archive id; the shipped tile is named for
# what it is, and PROVENANCE.md carries the mapping back to the archive.
RENAME = {
    "others_0021_color_1k.png": "tileable_glass_curtain_wall.png",
    "others_0022_color_1k.png": "tileable_brick_loft.png",
    "others_0025_color_1k.png": "tileable_precast_panels.png",
    "others_0026_color_1k.png": "tileable_brick_mill.png",
    "others_0029_color_1k.png": "tileable_warehouse_office.png",
}

# Pixels per metre in the shipped tile, chosen to match what the pipeline
# actually asks for.
#
# This used to be 8.0, reasoning from `paintings.rs`'s TEX_PX_PER_M. That is the
# Mapillary wall texture's resolution and the preset path never goes through it:
# `building_facades::px_per_m` asks for `px_per_block * scale`, clamped to
# MAX_PX_PER_M = 32, so a world at scale 2 with the default 16 px per block
# wants 32 px/m and was being handed 8, then upscaled 4x. That is the blur.
#
# 32 is the ceiling the code will ever ask for, so nothing finer can reach the
# screen. The photographs carry it: their native resolution is a median 121 px/m
# and 105 of 109 are at or above 32, so this is a downscale for almost all of
# them and only four are resampled up a little.
PPM = 32.0

# Measured: 37 dB PSNR median over the set, worst 32 dB, against pixels that
# reach the screen at 4 to 8 per block.
JPEG_QUALITY = 92

# A texture is called tiling when the wrap seam is this fraction of the
# distance between two unrelated columns of the same photograph. Below ~0.5 the
# seam is lost in the facade's own variation.
TILE_OK = 0.55

R, HO, FA = "Residential", "House", "Farm"
CO, OF, HT = "Commercial", "Office", "Hotel"
IN, WA = "Industrial", "Warehouse"
SC, HP, RE = "School", "Hospital", "Religious"
TB, GS, GC = "TallBuilding", "GlassySkyscraper", "GlassCornerSkyscraper"
GR, CS, MS = "GridSkyscraper", "ContemporarySkyscraper", "ModernSkyscraper"
MA, HI, TO = "MasonrySkyscraper", "Historic", "Tower"
GA, SH, GH = "Garage", "Shed", "Greenhouse"
DE = "Default"

N = (0.0, 0.0, 0.0, 0.0)

TABLE = [
    ("apartment_block5.png", [R, OF, DE], 17.0, 6.4, 2, False, "auto", N,
     "brick and grey panel, full-height glazed flats, two storeys"),
    ("apartment_block6.png", [R, TB], 23.5, 40.0, 14, False, "no", N,
     "whole 14 storey brick tower with its own returns; place it once"),
    ("apartment_block7.png", [R, DE], 21.8, 11.6, 4, False, "auto", N,
     "brick walk-up with recessed balcony bays"),
    ("apartment_block8.png", [R, DE], 23.2, 11.6, 4, False, "auto", N,
     "brick walk-up with an open stair core"),
    ("apartments1.png", [R], 26.0, 16.8, 6, False, "auto", N,
     "brick and timber balconies either side of a white stair core"),
    ("apartments2-2.png", [R, TB], 41.0, 32.4, 12, False, "keep", N,
     "1970s slab, brick and cream bands, tiles almost perfectly"),
    ("apartments2.png", [R, TB], 41.5, 43.0, 16, False, "keep", N,
     "16 storey brick slab, tiles almost perfectly"),
    ("apartments2_side.png", [R, TB], 13.0, 46.0, 17, False, "auto", N,
     "narrow render gable of a tower, one window column"),
    ("apartments4.png", [R, TB], 17.1, 20.8, 7, False, "auto", (0.155, 0.0, 0.135, 0.0),
     "concrete panel block; the angled balcony returns are cropped off"),
    ("apartments5.png", [R, TB], 28.9, 36.4, 13, False, "auto", N,
     "13 storey brick tower with recessed balconies"),
    ("apartments6.png", [R, TB], 29.8, 33.6, 12, False, "auto", N,
     "brick and timber balconies, white stair core"),
    ("apartments7.png", [R, TB], 28.2, 41.0, 15, False, "auto", N,
     "15 storey brick tower, dark spandrel bands"),
    ("apartments8.png", [R, TB], 30.8, 30.8, 11, False, "keep", N,
     "brick tower with a balcony core, tiles almost perfectly"),
    ("apartments9.png", [R], 27.5, 19.6, 7, False, "auto", N,
     "white render with brown recessed bays"),
    ("building_5c.png", [CO, DE], 8.2, 5.5, 1, True, "auto", N,
     "brick with a green shopfront and a door"),
    ("building_church_side1.png", [RE], 12.2, 9.0, 1, False, "auto", N,
     "two gothic nave windows in coursed sandstone"),
    ("building_church_side_bottom1.png", [RE, HI], 10.0, 10.0, 1, True, "keep", N,
     "gothic window over a stone plinth"),
    ("building_church_side_bottom2.png", [RE, HI], 11.2, 10.0, 1, True, "auto", N,
     "two gothic windows over a stone plinth"),
    ("building_church_side_top.png", [RE, HI], 16.0, 8.0, 1, False, "keep", N,
     "traceried clerestory windows, tiles cleanly"),
    ("building_derelict1.png", [IN, WA, CO], 15.0, 7.5, 2, False, "auto", N,
     "boarded and broken windows in tile and panel, derelict"),
    ("building_dks-1.png", [IN, WA, HI], 10.8, 16.0, 4, True, "auto", N,
     "red brick mill, green steel windows, four storeys"),
    ("building_dks-2.png", [IN, WA, HI], 10.8, 16.0, 4, True, "auto", N,
     "red brick mill, sister elevation to dks-1"),
    ("building_dock.png", [IN, WA, OF], 27.8, 10.0, 2, False, "auto", (0.0, 0.0, 0.0, 0.28),
     "clad shed over a dock office band; the quay wall is cropped off"),
    ("building_dock2.png", [WA, IN, HI], 17.0, 8.5, 2, False, "auto", N,
     "brick warehouse, segmental heads, pilaster"),
    ("building_dock_apartments.png", [WA, HI, R], 24.7, 15.0, 4, False, "auto", N,
     "victorian dock warehouse, arched windows and a cornice"),
    ("building_dock_apartments2.png", [R, HT, OF, TB], 32.2, 23.2, 8, False, "keep", N,
     "plain concrete block, small punched windows, tiles almost perfectly"),
    ("building_empty.png", [CO, OF], 11.9, 7.0, 2, True, "auto", N,
     "1960s civic band, relief panels over a boarded shopfront"),
    ("building_factory.png", [IN, WA], 8.8, 11.0, 2, False, "auto", N,
     "red brick with two glass block factory windows"),
    ("building_front3.png", [IN, CO, DE], 7.0, 7.0, 2, False, "auto", N,
     "painted render wall, one window, paint failing at the base"),
    ("building_front4.png", [OF, SC], 3.2, 5.0, 1, False, "keep", N,
     "single window bay in dark brick under a concrete band"),
    ("building_front5.png", [IN, WA], 6.9, 5.5, 1, False, "auto", N,
     "brick with a caged industrial window"),
    ("building_front8.png", [IN, WA, GA], 7.3, 5.5, 1, True, "auto", N,
     "blue profiled steel with a roller shutter and a door"),
    ("building_garage.png", [GA, SH, CO], 11.0, 4.0, 1, True, "keep", N,
     "render lock-up with a door and a roller shutter, tiles cleanly"),
    ("building_h_windows.png", [SC, OF, HP], 7.0, 7.0, 2, False, "auto", N,
     "maroon tile cladding with tall two-storey windows"),
    ("building_house1.png", [HO, R], 6.2, 6.2, 2, True, "auto", N,
     "rendered terrace house over a red brick plinth, boarded windows"),
    ("building_hsp.png", [OF, SC, HP], 6.3, 4.2, 1, False, "auto", N,
     "concrete band with a continuous ribbon window"),
    ("building_jmu.png", [SC, OF, MS], 23.0, 16.0, 4, False, "keep", N,
     "dark brick university block, four ribbon windows, tiles cleanly"),
    ("building_jmu2.png", [SC, OF, MS], 26.0, 14.4, 4, False, "keep", N,
     "same building as jmu without the blank top band, tiles cleanly"),
    ("building_l2.png", [OF, CO, HI], 14.0, 9.0, 3, False, "auto", N,
     "white faience office over stone piers"),
    ("building_lb1.png", [HI, CO, OF, MA], 10.2, 8.0, 2, False, "keep", N,
     "edwardian stone with big arched windows and a balustrade"),
    ("building_lh1.png", [GR, GS, OF, TB], 19.5, 13.0, 4, False, "auto", N,
     "full curtain wall grid with lit office floors"),
    ("building_liver.png", [HI, MA, OF, TB], 12.3, 24.6, 7, True, "keep", N,
     "portland stone classical office, arcaded top storey"),
    ("building_mirrored.png", [GS, MS, TB, OF], 30.4, 44.0, 13, False, "auto", N,
     "bronze mirror glass tower between brick piers"),
    ("building_modern.png", [OF, SC, HP], 7.2, 7.2, 2, False, "auto", N,
     "precast concrete panels with rounded window openings"),
    ("building_modern2.png", [OF, CO, MS], 13.6, 6.8, 2, False, "auto", N,
     "terracotta bands with blue ribbon windows"),
    ("building_modern3.png", [OF, SC], 8.9, 4.5, 1, False, "keep", N,
     "ribbed precast panels with square windows"),
    ("building_modern_side.png", [OF, CO, MS, DE], 15.0, 10.8, 3, False, "grad", (0.03, 0.05, 0.03, 0.02),
     "blank pink cladding, for a windowless flank"),
    ("building_office.png", [OF, CO, SC], 17.0, 8.5, 2, False, "auto", N,
     "red brick office, ribbon windows and a clerestory"),
    ("building_office10.png", [OF, TB], 16.6, 19.8, 6, False, "keep", N,
     "1960s concrete and glass, six storeys"),
    ("building_office11.png", [OF], 19.7, 16.5, 5, False, "auto", N,
     "brown brick with continuous ribbon windows"),
    ("building_office12.png", [OF, SC, CO], 7.1, 8.0, 2, False, "keep", N,
     "buff brick with large paired windows"),
    ("building_office13.png", [OF, IN, CO], 7.2, 7.2, 2, False, "keep", N,
     "grey ribbed cladding with ribbon windows"),
    ("building_office2.png", [OF, TB], 14.4, 28.8, 9, False, "auto", N,
     "narrow 1960s office slab, nine storeys of ribbon window"),
    ("building_office3.png", [OF, HT, SC], 16.5, 16.5, 5, False, "auto", N,
     "white tile with red framed windows"),
    ("building_office4.png", [OF, GS, GR], 18.1, 7.0, 2, False, "auto", N,
     "blue mullion curtain wall, two storeys"),
    ("building_office5.png", [OF], 15.8, 10.2, 3, False, "no", N,
     "ribbon windows between dark brick spandrels"),
    ("building_office7.png", [OF, TB], 24.6, 18.0, 5, False, "auto", N,
     "concrete frame office, paired windows, top parapet"),
    ("building_office8.png", [OF], 16.0, 16.0, 5, False, "auto", N,
     "1960s ribbon window office, five storeys"),
    ("building_office9.png", [OF, GR, CS, TB], 21.9, 13.2, 4, False, "auto", N,
     "deep concrete grid with recessed windows"),
    ("building_oldfirm.png", [HI, OF, CO, SC], 4.6, 8.4, 2, False, "keep", N,
     "victorian red brick with arched sash windows"),
    ("building_portacabin.png", [SH], 6.0, 3.0, 1, True, "auto", N,
     "portacabin units, the only site hut in the set"),
    ("building_pub_old.png", [CO, HI], 9.0, 4.5, 1, True, "auto", N,
     "old pub in faience tile, boarded windows and a door"),
    ("building_showroom_vacant.png", [CO, WA], 5.0, 5.0, 1, True, "keep", N,
     "vacant showroom boarded with OSB under a clad fascia"),
    ("building_side.png", [HI, RE, SC], 6.05, 11.0, 2, True, "keep", N,
     "pink brick arched gable bay over a blue engineering brick plinth"),
    ("building_side2.png", [OF, SC, HP, R], 9.2, 7.2, 2, False, "auto", (0.0, 0.0, 0.0, 0.22),
     "modern brick and louvred windows; grass and a garden wall cropped off"),
    ("building_side3.png", [HO, R, SH], 5.2, 4.0, 1, False, "keep", N,
     "pebbledash render with three small windows"),
    ("building_side4.png", [WA, IN], 6.2, 5.0, 1, True, "grad", N,
     "white profiled cladding on a brick plinth"),
    ("building_side5.png", [CO, IN], 5.9, 4.5, 1, True, "keep", N,
     "brick with barred windows under a blue fascia"),
    ("building_side6.png", [WA, IN], 6.0, 6.0, 1, True, "grad", N,
     "grey profiled cladding over a buff brick base"),
    ("building_side7.png", [SC, HI, R, OF], 8.1, 6.8, 2, False, "auto", (0.08, 0.0, 0.0, 0.0),
     "red brick with sash windows; the downpipe is cropped off"),
    ("building_side_dks1.png", [IN, WA], 7.3, 8.0, 2, True, "keep", N,
     "factory steel glazing over a brick base"),
    ("building_side_long.png", [WA, IN, SC], 18.0, 4.5, 1, True, "no", N,
     "long low brick shed with small windows and a door"),
    ("loading_bays.png", [WA, IN], 6.9, 6.0, 1, True, "keep", N,
     "two dock levellers, brick base and bollards"),
    ("restaurant_window.png", [CO, HI], 3.5, 5.0, 1, True, "keep", N,
     "arched restaurant window in stone over a brick base"),
    ("shop_front10.png", [CO], 8.7, 3.4, 1, True, "auto", N,
     "bakery, half shuttered, timber doors"),
    ("shop_front11.png", [CO], 7.0, 3.6, 1, True, "auto", N,
     "blue sandwich bar with a deep fascia"),
    ("shop_front12.png", [CO], 5.9, 4.2, 1, True, "auto", N,
     "green painted shop with a caged window"),
    ("shop_front13.png", [CO], 5.6, 3.6, 1, True, "auto", N,
     "closed shutter under an ice cream fascia"),
    ("shop_front14.png", [CO], 4.9, 4.2, 1, True, "no", N,
     "red and yellow takeaway front"),
    ("shop_front15.png", [CO], 4.9, 4.2, 1, True, "no", N,
     "takeaway in red brick with a timber door"),
    ("shop_front16.png", [CO], 5.0, 4.2, 1, True, "no", N,
     "boarded internet cafe with a white door"),
    ("shop_front17_derelict.png", [CO], 4.7, 4.2, 1, True, "no", N,
     "derelict shop boarded with mismatched ply"),
    ("shop_front2.png", [CO], 5.4, 4.2, 1, True, "auto", N,
     "flooring shop, yellow fascia and a blue stall riser"),
    ("shop_front3.png", [CO, R], 10.1, 7.5, 2, True, "auto", N,
     "white painted shop with two flats above"),
    ("shop_front4.png", [CO], 6.2, 4.2, 1, True, "auto", N,
     "red shutter under a painted name board"),
    ("shop_front5.png", [CO], 7.3, 4.0, 1, True, "auto", N,
     "timber shopfront between brick piers"),
    ("shop_front6.png", [CO], 12.0, 4.0, 1, True, "no", N,
     "restaurant with awnings over a tiled stall riser"),
    ("shop_front7.png", [CO, GA, SH], 5.6, 4.0, 1, True, "no", N,
     "weathered timber cladding with a window and a wide door"),
    ("shop_front8.png", [CO], 4.6, 4.4, 1, True, "no", N,
     "tiled takeaway front with an arched window"),
    ("shop_front9.png", [CO], 7.7, 3.4, 1, True, "keep", N,
     "pharmacy, full width glazing under a white fascia"),
    ("shop_front_top1.png", [CO, R], 5.8, 3.6, 1, False, "keep", N,
     "the first floor above a shop, render and two sash windows"),
    ("shopfront_neon.png", [CO], 5.6, 4.4, 1, True, "keep", N,
     "sign maker with a lit neon board and deep glazing"),
    ("shutters_1.png", [CO, GA, WA], 5.1, 3.6, 1, True, "auto", N,
     "single roller shutter between painted brick piers"),
    ("shutters_door.png", [WA, IN, GA], 4.5, 4.5, 1, True, "no", N,
     "wide roller shutter with a steel personnel door"),
    ("shutters_door2.png", [GA, WA], 2.0, 4.0, 1, True, "no", N,
     "narrow shutter in a red painted frame"),
    ("shutters_large1.png", [WA, GA, IN], 5.6, 4.5, 1, True, "keep", N,
     "large shutter in a concrete frame"),
    ("shutters_large3.png", [IN, WA, SH, GA], 4.9, 4.0, 1, False, "auto", N,
     "green vertical profiled steel wall"),
    ("shutters_large4.png", [IN, WA, SH], 4.5, 4.5, 1, False, "grad", N,
     "blue vertical profiled steel wall"),
    ("shutters_large_dirty.png", [WA, GA, IN], 5.7, 3.6, 1, True, "keep", N,
     "dirty shutter in a red frame"),
    ("wall_shutter1.png", [GA, WA, CO], 4.2, 5.0, 1, True, "keep", N,
     "red blockwork with a shutter over a dark plinth"),
    ("wall_shutter2.png", [GA, CO, WA], 5.9, 4.5, 1, True, "keep", N,
     "painted brick with a shutter under a blue fascia"),
    ("wall_shutter3.png", [GA, WA, IN], 6.6, 4.5, 1, True, "keep", N,
     "old brick with a shutter, patched and repointed"),
    ("wall_steel_corrugated_2tone.png", [WA, IN, SH], 4.8, 6.0, 1, False, "keep", N,
     "white corrugated steel with a red band and eaves"),
    ("wall_steel_corrugated_door.png", [WA, IN, GA], 6.7, 5.0, 1, True, "keep", N,
     "corrugated steel with a painted disc and a door"),
    ("warehouse_front.png", [WA, IN], 22.0, 6.0, 1, True, "keep", N,
     "long clad warehouse with red doors, tiles cleanly"),
    ("warehouse_shutters1.png", [WA, IN], 8.2, 5.5, 1, True, "auto", N,
     "brick merchant's warehouse with a wide shutter and signage"),
    ("Building-Facade-Texture_01.jpg", [R, OF, HT, TB], 24.8, 33.0, 11, False, "keep", N,
     "white panel slab with rows of small windows"),
    ("Building-Facade-Texture_02.jpg", [R, OF, CS, TB], 14.7, 21.7, 7, False, "keep", N,
     "pale stone cladding with irregular deep-set windows"),
    ("Building-Facade-Texture_03.jpg", [GS, GR, OF, TB], 22.4, 28.0, 8, False, "auto", N,
     "blue glass office tower, spandrel bands"),
    ("others_0021_color_1k.jpg", [GS, GR, MS, OF, TB], 25.2, 25.2, 7, False, "keep", N,
     "seamless curtain wall, blue glass between concrete piers (PBR set others_0021)"),
    ("others_0022_color_1k.jpg", [HI, OF, R, TB], 34.0, 34.0, 10, False, "auto", N,
     "seamless brick and steel loft, ten storeys of subdivided sashes (PBR set others_0022)"),
    ("others_0025_color_1k.jpg", [OF, HP, SC, R, TB], 19.8, 19.8, 6, False, "auto", N,
     "seamless precast panels with staggered windows and louvres (PBR set others_0025)"),
    ("others_0026_color_1k.jpg", [IN, WA, HI, SC], 20.0, 20.0, 5, False, "keep", N,
     "seamless red brick mill with arched windows (PBR set others_0026)"),
    ("others_0029_color_1k.jpg", [OF, IN, WA, GR, TB], 18.0, 18.0, 5, False, "keep", N,
     "seamless concrete pier and glazing, abandoned warehouse (PBR set others_0029)"),
]

REJECTED = {
    "church_arch_windows.png":
        "the gable is cut out on black, so the corners of any rectangle are "
        "black; it wants an alpha mask, which this mechanism has no place for",
    "building_construction.png":
        "a crane mast and an open hoist shaft run down the middle and the "
        "boarded panels read as damage; the clean left third is two bays wide",
    "building_center.png":
        "the lower two thirds is a dark speckled panel that reads as tarmac, "
        "and the glazing is squeezed into the top fifth",
    "building_front2.png":
        "heavy dirt streaking down the render, unrelated left and right, and a "
        "1024 px wall for three small windows; the seam stays at 2.3",
    "building_office13-end.png":
        "the end bay of the same building as building_office13, a corner stair "
        "core; tiled it repeats a stair core every five metres",
    "building_modern_side.png":
        "a blank pink cladding wall with nothing on it; on a street elevation "
        "it reads as worse than the blocks it replaces, and the flat shell "
        "leaves it with no procedural detail either",
}

def col_seam_ratio(a):
    """Wrap seam between the first and last column, over the typical distance
    between two unrelated columns of the same photograph.

    The denominator is what makes the number mean something: 8 grey levels
    across the seam is nothing on a brick wall and glaring on flat render. The
    offsets are fixed rather than sampled so that two candidate crops of the
    same photograph can be compared without sampling noise deciding it."""
    h, w, _ = a.shape
    seam = float(np.abs(a[:, w - 1] - a[:, 0]).mean())
    parts = []
    for frac in (0.2, 0.33, 0.5):
        k = max(1, int(w * frac))
        parts.append(float(np.abs(a[:, k:] - a[:, :w - k]).mean()))
    rand = float(np.mean(parts))
    return seam / max(rand, 1e-6), seam, rand


def tile_crop(a, min_frac=0.55, max_start=0.45, band=4, lam=0.5, top=80):
    """The crop [x0, x1) whose two edges match best, traded off against how
    much width it throws away. A facade is periodic, so the minimum lands on a
    whole number of window bays.

    The band distance only proposes candidates. Each one is then scored by the
    seam that will actually be measured on the shipped tile, because a band of
    four columns can match where the single edge columns do not, and an
    unchecked proposal made a third of this set worse rather than better."""
    h, w, _ = a.shape
    rows = np.linspace(0, h - 1, min(96, h)).astype(int)
    small = a[rows]                                   # rows x w x 3
    nb = w - band
    if nb < 8:
        return 0, w
    feats = np.stack([small[:, x:x + band].ravel() for x in range(nb)])
    feats = feats.astype(np.float32)
    sq = (feats * feats).sum(1)
    d2 = sq[:, None] + sq[None, :] - 2.0 * (feats @ feats.T)
    np.maximum(d2, 0.0, out=d2)
    dist = np.sqrt(d2 / feats.shape[1])               # per-channel RMS

    xs = np.arange(nb)
    width = xs[None, :] - xs[:, None]                 # b - a
    ok = (width >= min_frac * w) & (xs[:, None] <= max_start * w)
    if not ok.any():
        return 0, w
    # One `rand` for the whole photograph, so every candidate is scored on the
    # same scale and the sampling noise cannot decide the winner.
    seam0, rand = col_seam_ratio(a)[1:]
    cost = np.where(ok, dist / max(rand, 1e-6) + lam * (1.0 - width / w), np.inf)

    def edge(sub):
        return float(np.abs(sub[:, -1] - sub[:, 0]).mean()) / max(rand, 1e-6)

    # Uncropped is a candidate like any other, and it keeps every pixel.
    best_score, best = seam0 / max(rand, 1e-6), (0, w)
    for i in np.argsort(cost, axis=None)[:top]:
        x0, x1 = divmod(int(i), nb)
        if not np.isfinite(cost[x0, x1]):
            break
        score = edge(a[:, x0:x1]) + lam * (1.0 - (x1 - x0) / w)
        if score < best_score:
            best_score, best = score, (x0, x1)
    return best


def flatten_gradient(im):
    """Divide out the low-frequency left-to-right brightness ramp.

    On a blank clad wall the only thing standing between the photograph and a
    clean tile is the sun: one end is lit and the other is not, so the wrap
    shows as a bar of shading no crop can remove. Only used on walls with no
    composition to destroy."""
    a = np.asarray(im, dtype=np.float32)
    prof = a.mean(axis=(0, 2))                        # one value per column
    # A box blur wide enough to pass the lighting and stop at the panel joints.
    k = max(3, (len(prof) // 4) | 1)
    pad = np.pad(prof, k // 2, mode="edge")
    smooth = np.convolve(pad, np.ones(k) / k, mode="valid")[:len(prof)]
    gain = np.clip(smooth.mean() / np.maximum(smooth, 1e-3), 0.6, 1.6)
    out = np.clip(a * gain[None, :, None], 0, 255).astype(np.uint8)
    return Image.fromarray(out)


def unpack_pbr(dest):
    """The `color` map out of every others_*.zip, and nothing else."""
    n = 0
    for z in sorted(glob.glob(os.path.join(SRC, "others_*.zip"))):
        with zipfile.ZipFile(z) as zf:
            for member in zf.namelist():
                if "__MACOSX" in member or "color" not in member:
                    continue
                with zf.open(member) as fh,                         open(os.path.join(dest, os.path.basename(member)), "wb") as out:
                    shutil.copyfileobj(fh, out)
                n += 1
    return n


def main():
    global PBR
    PBR = tempfile.mkdtemp(prefix="arnis-facade-pbr-")
    unpack_pbr(PBR)
    try:
        return build()
    finally:
        shutil.rmtree(PBR, ignore_errors=True)


def build():
    os.makedirs(DST, exist_ok=True)
    entries = []
    report = []
    total_bytes = 0
    for (name, cats, mw, mh, storeys, ground, mode, pre, note) in TABLE:
        root = SRC if os.path.exists(os.path.join(SRC, name)) else PBR
        im = Image.open(os.path.join(root, name)).convert("RGB")
        W0, H0 = im.size
        l, t, r, b = pre
        if any(pre):
            im = im.crop((round(l * W0), round(t * H0),
                          W0 - round(r * W0), H0 - round(b * H0)))
        W1, H1 = im.size

        def measure(img):
            w = img.width if img.width <= 640 else 640
            h = max(1, round(img.height * w / img.width))
            small = img if img.size == (w, h) else img.resize((w, h), Image.LANCZOS)
            return col_seam_ratio(np.asarray(small, dtype=np.float32))[0]

        before = measure(im)
        if mode == "grad":
            im = flatten_gradient(im)

        # The search runs on a reduced copy; a 4 px band there is 8 to 12 px of
        # the original, which is what a window mullion measures.
        work = im if W1 <= 640 else im.resize(
            (640, max(1, round(H1 * 640 / W1))), Image.LANCZOS)

        x0, x1 = 0, W1
        if mode in ("auto", "grad"):
            sx0, sx1 = tile_crop(np.asarray(work, dtype=np.float32))
            sc = W1 / work.width
            x0, x1 = round(sx0 * sc), round(sx1 * sc)
            if x1 - x0 < 16:
                x0, x1 = 0, W1
        cropped = im.crop((x0, 0, x1, H1))
        after = measure(cropped)

        # The search scores candidates against the whole photograph's column
        # variation; a narrow crop has less variation of its own, so a crop can
        # win the search and still measure worse on the tile that ships. The
        # shipped measurement is the one that counts.
        if after > before:
            cropped, x0, x1, after = im, 0, W1, measure(im)

        # metres_wide was written for the post-crop image; if the search kept
        # less than planned, scale the width so the metre scale stays honest.
        kept = (x1 - x0) / W1
        # the aspect the row assumed, versus what the crop actually produced
        planned_ar = mw / mh
        actual_ar = cropped.width / cropped.height
        mw_out = round(mh * actual_ar, 1)

        tw = max(16, round(mw_out * PPM))
        th = max(16, round(mh * PPM))
        out = cropped.resize((tw, th), Image.LANCZOS)
        stem = os.path.splitext(name)[0] + ".png"
        stem = RENAME.get(stem, stem)
        # JPEG, not PNG: these are photographs, and PNG spends three times the
        # bytes on them for detail no one can see at 4 to 8 px per block.
        stem = os.path.splitext(stem)[0] + ".jpg"
        path = os.path.join(DST, stem)
        out.convert("RGB").save(path, quality=JPEG_QUALITY, optimize=True, subsampling=0)
        size = os.path.getsize(path)
        total_bytes += size

        # The flag is the measurement, always. "no" only says the crop
        # search must leave this photograph alone.
        tiles = after < TILE_OK
        entries.append({
            "file": stem,
            "categories": cats,
            "metres_wide": mw_out,
            "metres_tall": mh,
            "storeys": storeys,
            "tiles_horizontally": bool(tiles),
            "has_ground_floor": bool(ground),
            "note": note,
        })
        report.append((stem, W0, H0, tw, th, before, after, kept, tiles, size,
                       mode, planned_ar, actual_ar))

    manifest = {"version": 1, "textures": entries}
    with open(os.path.join(DST, "manifest.json"), "w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")

    print(f"{'file':40s} {'src':>11s} {'out':>10s} {'seam':>13s} {'kept':>5s} tile   KB")
    for (n, w0, h0, tw, th, b4, af, kept, tiles, size, mode, par, aar) in report:
        flag = "yes" if tiles else " no"
        print(f"{n:40s} {w0:5d}x{h0:5d} {tw:4d}x{th:4d} "
              f"{b4:5.2f}->{af:5.2f} {kept:5.0%} {flag}  {size/1024:6.1f}")
    print(f"\n{len(report)} textures, {total_bytes/1e6:.2f} MB")
    print(f"rejected {len(REJECTED)}")


if __name__ == "__main__":
    sys.exit(main())
