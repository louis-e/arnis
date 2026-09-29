"""Rebuild the bundled climate grids and the ecoregion tree table.

    python assets/climate/build_grids.py [--ecoregions Ecoregions2017.zip] [--koppen koppen_0p1.tif]

Writes, next to this script:
  koppen.grid        Koppen-Geiger class per 0.1 deg cell (u8), Beck et al. 2023,
                     1991-2020 map `koppen_geiger_0p1.tif` or its raw 3600x1800 u8 dump.
  ecoregions.grid    RESOLVE Ecoregions 2017 ECO_ID per 1 arc-minute cell (u16),
                     downloaded from storage.googleapis.com/teow2016 when not given.
  ecoregions.tsv     id, biome, realm, pack, tree mix, name for every ecoregion.

Grid container, little endian: b"AGRD", version u8, bytes per cell u8, tile side u16,
cols u32, rows u32, cell degrees f64, then (tiles + 1) u32 offsets into the zstd frames
that follow. Tiles run row-major from the north-west corner; an all-zero tile has no
frame. Needs numpy, geopandas, rasterio and zstandard.

A tree mix is `;`-separated entries `[^~]pack:Community[!Species...]*weight`. `^` entries
only grow on montane cells, `~` entries only on wet ground (wetland, mangrove, swamp),
`!Species` drops a species (a genus name drops the whole genus).
"""

import argparse
import json
import os
import re
import struct
import sys
import tempfile
import urllib.request

import numpy as np
import zstandard as zstd

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
PACKS = os.path.join(ROOT, "assets", "tree-packs")
ECO_URL = "https://storage.googleapis.com/teow2016/Ecoregions2017.zip"
TILE_DEG = 10
REALMS = {
    "Afrotropic": "AT",
    "Antarctica": "AN",
    "Australasia": "AA",
    "Indomalayan": "IM",
    "Nearctic": "NA",
    "Neotropic": "NT",
    "Oceania": "OC",
    "Palearctic": "PA",
}

MIX = {}


def put(ids, mix):
    for i in ids:
        assert i not in MIX, f"ecoregion {i} mapped twice"
        MIX[i] = mix


# Afrotropic
put([30, 11, 23, 6], "afr:Guinean West African Rainforest*5; afr:African Fan Palms*1; ~afr:African Fan Palms*2")
put([14], "afr:Guinean West African Rainforest*4; afr:African Fan Palms*1; ^afr:Mountain Rainforest*3")
put([22], "afr:Guinean West African Rainforest*3; afr:African Fan Palms*2; ~afr:African Fan Palms*3")
put([7], "afr:Congo Basin Rainforest*4; afr:Guinean West African Rainforest*1; afr:African Fan Palms*1")
put([27], "afr:Congo Basin Rainforest*3; aus:Coconut Palms*1")
put([21, 2], "afr:Mountain Rainforest*3; afr:Congo Basin Rainforest*2; ^afr:Mountain Rainforest*3")
put([5, 26, 3, 24], "afr:Congo Basin Rainforest*5; afr:African Fan Palms*1; ~afr:African Fan Palms*2")
put([29, 10], "afr:Congo Basin Rainforest*3; afr:African Fan Palms*2; ~afr:African Fan Palms*3")
put([1, 8, 9], "afr:Mountain Rainforest*3; afr:Congo Basin Rainforest*2; ^afr:Mountain Rainforest*3")
put([12], "afr:Mountain Rainforest*4; afr:East African Savanna*1; aus:Temperate Eucalypt Forest (Victoria)*1; ^afr:Mountain Rainforest*3")
put([15], "afr:Mountain Rainforest*4; afr:Fevertree Acacias*1; ^afr:Mountain Rainforest*2")
put([16], "afr:Mountain Rainforest*2; afr:South African Veld!Adansonia!Colophospermum*2; afr:Fevertree Acacias*1")
put([19], "afr:Fevertree Acacias*2; afr:South African Veld*2; afr:East African Savanna*1; ~afr:Fevertree Acacias*2")
put([28, 25], "afr:East African Savanna*3; afr:Congo Basin Rainforest*1; afr:African Fan Palms*1; aus:Coconut Palms*1")
put([4, 13, 20], "afr:Malagasy Rainforest*3; aus:Pacific Island Beach Forest*1; aus:Coconut Palms*1")
put([18], "afr:Malagasy Rainforest*3; afr:Malagasy Highlands*2; ^afr:Malagasy Highlands*3")
put([17], "afr:Malagasy Rainforest*5; afr:African Fan Palms*1; ^afr:Malagasy Highlands*2")
put([31], "afr:Sahelian Scrub and Semidesert!Adansonia*3; afr:Canary Island Date Palms*1")
put([33], "afr:Central-Southern African Miombo Woodlands*4; afr:Congo Basin Rainforest*1")
put([32], "afr:Malagasy Dry Forest*5; afr:Malagasy Spiny Forest*1")
put([37, 60], "afr:East African Savanna!Adansonia*1; aus:Pacific Island Beach Forest*1")
put([62, 43], "afr:Sudanian Savanna*5; afr:African Fan Palms*1")
put([44, 49], "afr:Sudanian Savanna*3; afr:Guinean West African Rainforest*2; afr:African Fan Palms*1")
put([52, 58, 63], "afr:Sudanian Savanna*2; afr:Congo Basin Rainforest*3; afr:African Fan Palms*1")
put([61], "afr:East African Savanna*2; afr:Congo Basin Rainforest*2; afr:Fevertree Acacias*1")
put([53], "afr:Sahelian Scrub and Semidesert*5; afr:Sudanian Savanna*1; afr:African Fan Palms*1")
put([36, 39, 42, 35, 46, 64, 66], "afr:Central-Southern African Miombo Woodlands*5; afr:South African Veld*1")
put([34, 65], "afr:Central-Southern African Miombo Woodlands*3; afr:South African Veld*2")
put([47, 38, 48], "afr:South African Veld*5; afr:Fevertree Acacias*1")
put([41, 40], "afr:South African Veld!Adansonia!Colophospermum*3; afr:Mountain Rainforest*1; ^afr:Mountain Rainforest*2")
put([54, 50, 57, 51, 55, 45], "afr:East African Savanna*5; afr:Fevertree Acacias*1; ~afr:Fevertree Acacias*2")
put([59, 56], "afr:Sahelian Scrub and Semidesert!Adansonia*3; afr:East African Savanna!Adansonia*1")
put([68, 67], "aus:New Zealand South Island*1")
put([71, 72], "afr:Sahelian Scrub and Semidesert*3; afr:African Fan Palms*2; afr:Sudanian Savanna*1; ~afr:African Fan Palms*2")
put([74], "afr:Sudanian Savanna*2; afr:African Fan Palms*2; ~afr:African Fan Palms*3")
put([76, 75], "afr:Central-Southern African Miombo Woodlands*2; afr:Fevertree Acacias*2; afr:African Fan Palms*1; ~afr:Fevertree Acacias*2")
put([70, 73], "afr:South African Veld*3; afr:African Fan Palms*1")
put([69], "afr:East African Savanna*3; afr:Fevertree Acacias*1")
put([82], "afr:Sudanian Savanna*3; ^afr:Mountain Rainforest*2")
put([77], "afr:Central-Southern African Miombo Woodlands*2; afr:Mountain Rainforest*2; ^afr:Mountain Rainforest*2")
put([81], "afr:South African Veld!Adansonia!Colophospermum*3; aus:Temperate Eucalypt Forest (Victoria)*1; sam:Pampas*1")
put([86, 78, 80], "afr:Mountain Rainforest*3; ^afr:Mountain Rainforest*2")
put([79], "afr:Mountain Rainforest*3; afr:East African Savanna*1; aus:Temperate Eucalypt Forest (Victoria)*2; ^afr:Mountain Rainforest*2")
put([85, 87, 84], "afr:Mountain Rainforest*3; afr:Central-Southern African Miombo Woodlands*2; ^afr:Mountain Rainforest*2")
put([83], "afr:Malagasy Highlands*3")
put([90, 89], "afr:Mountain Rainforest*2; eur:Western Mediterranean*2; ^afr:Mountain Rainforest*2")
put([88], "afr:South African Veld!Adansonia!Colophospermum*2; afr:Mountain Rainforest*1")
put([98, 103, 102, 94, 110, 101, 97], "afr:South African Veld!Adansonia!Colophospermum*3")
put([104], "afr:South African Veld*3; afr:Central-Southern African Miombo Woodlands*1")
put([96, 91], "aus:Pacific Island Beach Forest*2; afr:East African Savanna!Adansonia*1")
put([92, 93, 95, 106], "afr:Sahelian Scrub and Semidesert!Adansonia*3; afr:East African Savanna*2")
put([105], "afr:Sahelian Scrub and Semidesert!Adansonia*2; afr:East African Savanna!Adansonia*1")
put([108, 107, 109], "afr:Sahelian Scrub and Semidesert!Adansonia*3; afr:East African Savanna!Adansonia*1; afr:Canary Island Date Palms*1")
put([99, 100], "afr:Malagasy Spiny Forest*5; afr:Malagasy Dry Forest*1")
put([113], "afr:Guinean West African Rainforest*2; afr:African Fan Palms*1; aus:Coconut Palms*1; ~sam:Mangroves!Schinus*4")
put([111], "afr:Congo Basin Rainforest*2; afr:African Fan Palms*1; aus:Coconut Palms*1; ~sam:Mangroves!Schinus*4")
put([116], "afr:Fevertree Acacias*1; afr:South African Veld*1; ~ind:Southeast Asian Mangroves!Nypa!Phoenix*2; ~aus:Grey Mangroves*2")
put([112], "afr:East African Savanna*2; aus:Coconut Palms*1; ~ind:Southeast Asian Mangroves!Nypa!Phoenix*3; ~aus:Grey Mangroves*1")
put([114], "afr:Malagasy Dry Forest*2; aus:Coconut Palms*1; ~ind:Southeast Asian Mangroves!Nypa!Phoenix*3; ~aus:Grey Mangroves*1")
put([115], "afr:Sahelian Scrub and Semidesert!Adansonia*2; afr:Canary Island Date Palms*1; ~aus:Grey Mangroves*4")

# Palearctic: Europe and the Mediterranean
TEMPERATE_OAKS = "eur:Oak forest!Quercus_suber!Quercus_coccifera"
OPEN_WOODS = "eur:Open woodlands!Quercus_coccifera"
put([645], "eur:Western Mediterranean*3; asn:Japanese Temperate Rainforest*1")
put([668], "eur:Western Mediterranean*3; afr:Canary Island Date Palms*1")
put([648], f"eur:Misc. Deciduous forest*3; {TEMPERATE_OAKS}*2; eur:Birch and aspen*1; aus:Temperate Eucalypt Forest (Victoria)*1; ~eur:Swamp and riparian forest*2; ^eur:Beech-yew Alpine foothills*2")
put([672], "eur:Birch and aspen*3; eur:Misc. Deciduous forest*2; eur:Bialowieza forest*1; ~eur:Swamp and riparian forest*2")
put([651], f"eur:Misc. Deciduous forest*3; {OPEN_WOODS}*2; eur:Birch and aspen*1; ~eur:Swamp and riparian forest*2")
put([663], f"eur:Misc. Deciduous forest*4; {OPEN_WOODS}*2; eur:Birch and aspen*1; eur:Dark oak or walnut*1; ~eur:Swamp and riparian forest*2")
put([664], f"eur:Misc. Deciduous forest*4; {OPEN_WOODS}*2; eur:Bialowieza forest*1; eur:Birch and aspen*1; ~eur:Swamp and riparian forest*2")
put([686], f"eur:Misc. Deciduous forest*4; eur:Beech-yew Alpine foothills*2; eur:Bialowieza forest*1; {OPEN_WOODS}*1; eur:Dark oak or walnut*1; ~eur:Swamp and riparian forest*2; ^eur:Alpine forest (mature)*2")
put([676], "eur:Beech-yew Alpine foothills*2; eur:Misc. Deciduous forest*2; eur:Alpine forest (mature)*2; ^eur:Alpine forest (short)*2; ^eur:Alpine forest (mature)*1")
put([644], f"eur:Beech-yew Alpine foothills*3; eur:Misc. Deciduous forest*2; {TEMPERATE_OAKS}*1; ^eur:Balkan coniferous forest*2")
put([675], f"eur:Misc. Deciduous forest*2; {OPEN_WOODS}*2; eur:Dark oak or walnut*1; eur:Swamp and riparian forest*1; ~eur:Swamp and riparian forest*3")
put([647], "eur:Misc. Deciduous forest*3; eur:Bialowieza forest*2; eur:Birch and aspen*2; eur:Scandinavian taiga (mature)*1; ~eur:Swamp and riparian forest*2")
put([660], "eur:Beech-yew Alpine foothills*2; eur:Balkan coniferous forest*2; eur:Misc. Deciduous forest*2; ^eur:Balkan coniferous forest*2")
put([674], f"{OPEN_WOODS}*3; eur:Misc. Deciduous forest*2; eur:Dark oak or walnut*1; ~eur:Swamp and riparian forest*3")
put([646], f"eur:Misc. Deciduous forest*3; {OPEN_WOODS}*2; eur:Eastern Mediterranean*1; ~eur:Swamp and riparian forest*1; ^eur:Balkan coniferous forest*2")
put([678], "eur:Balkan coniferous forest*3; eur:Beech-yew Alpine foothills*2; eur:Misc. Deciduous forest*1; ^eur:Balkan coniferous forest*2")
put([654], f"eur:Misc. Deciduous forest*3; eur:Bialowieza forest*2; eur:Birch and aspen*2; eur:Scandinavian taiga (mature)*1; {OPEN_WOODS}*1; ~eur:Swamp and riparian forest*2")
put([679], "eur:Bialowieza forest*3; eur:Birch and aspen*3; eur:Scandinavian taiga (mature)*2; eur:Misc. Deciduous forest*1; ~eur:Swamp and riparian forest*2")
put([652], f"eur:Middle East montane conifer forest*2; {OPEN_WOODS}*2; asn:Zagros Mountain forest-steppe*1; ~eur:Swamp and riparian forest*2")
put([665], "eur:Misc. Deciduous forest*3; eur:Beech-yew Alpine foothills*2; eur:Dark oak or walnut*1; ^eur:Balkan coniferous forest*2")
put([662], f"{OPEN_WOODS}*2; asn:Zagros Mountain forest-steppe*2; eur:Middle East montane conifer forest*1")
put([658], f"eur:Eastern Mediterranean*2; eur:Misc. Deciduous forest*2; eur:Middle East montane conifer forest*1; {OPEN_WOODS}*1")
put([661], f"{OPEN_WOODS}*3; eur:Birch and aspen*2; eur:Misc. Deciduous forest*2; ~eur:Swamp and riparian forest*2")
put([650], "eur:Misc. Deciduous forest*2; eur:Beech-yew Alpine foothills*2; eur:Balkan coniferous forest*1; ^eur:Alpine forest (mature)*1; ^eur:Balkan coniferous forest*1")
put([649], "eur:Misc. Deciduous forest*3; eur:Dark oak or walnut*1; eur:Beech-yew Alpine foothills*1; ~eur:Swamp and riparian forest*2")
put([691], "eur:Scandinavian taiga (mature)*3; eur:Birch and aspen*2; ^eur:Scandinavian taiga (short)*2")
put([701], "eur:Middle East montane conifer forest*3; eur:Western Mediterranean*2; ^eur:Middle East montane conifer forest*2")
put([689], "eur:Alpine forest (mature)*3; eur:Beech-yew Alpine foothills*2; eur:Misc. Deciduous forest*2; eur:Birch and aspen*1; ~eur:Swamp and riparian forest*2; ^eur:Alpine forest (mature)*3; ^eur:Alpine forest (short)*2")
put([708], "eur:Scandinavian taiga (mature)*3; eur:Birch and aspen*3; eur:Misc. Deciduous forest*1; ^eur:Scandinavian taiga (short)*2")
put([692], "eur:Beech-yew Alpine foothills*2; eur:Alpine forest (mature)*2; eur:Misc. Deciduous forest*2; eur:Bialowieza forest*1; ^eur:Alpine forest (mature)*2; ^eur:Alpine forest (short)*1")
put([703], "eur:Balkan coniferous forest*2; eur:Beech-yew Alpine foothills*2; eur:Middle East montane conifer forest*2; eur:Misc. Deciduous forest*1")
put([711], "eur:Birch and aspen*4; eur:Scandinavian taiga (short)*1")
put([717], "eur:Scandinavian taiga (mature)*4; eur:Birch and aspen*3; eur:Scandinavian taiga (short)*1; eur:Bialowieza forest*1; ~eur:Swamp and riparian forest*2")
put([719], "eur:Scandinavian taiga (mature)*3; eur:Birch and aspen*2; ^eur:Scandinavian taiga (short)*2")
put([729], "eur:Birch and aspen*1")
put([725], f"eur:Middle East montane conifer forest*1; {OPEN_WOODS}*1; asn:Zagros Mountain forest-steppe*1; ~eur:Swamp and riparian forest*2")
put([735], f"{OPEN_WOODS}*3; eur:Birch and aspen*1; eur:Dark oak or walnut*1; ~eur:Swamp and riparian forest*3")
put([780], "eur:Birch and aspen*5; eur:Scandinavian taiga (short)*1")
put([774, 776], "eur:Birch and aspen*3; eur:Scandinavian taiga (short)*1")
put([787], "afr:Canary Island Date Palms*3; eur:Western Mediterranean*2; ^eur:Middle East montane conifer forest*2")
put([796], "afr:Maghreb Mediterranean*5; afr:Sahelian Scrub and Semidesert!Adansonia*1")
put([805], f"eur:Western Mediterranean*4; eur:Oak forest*2; aus:Temperate Eucalypt Forest (Victoria)*1; ~eur:Swamp and riparian forest*2")
put([798], "eur:Western Mediterranean*4; eur:Oak forest*1; afr:Canary Island Date Palms*1; afr:Maghreb Mediterranean*1; ^eur:Middle East montane conifer forest*2")
put([793], "eur:Western Mediterranean*3; eur:Oak forest*2; eur:Eastern Mediterranean*1; ~eur:Swamp and riparian forest*2; ^eur:Balkan coniferous forest*2")
put([800], f"eur:Misc. Deciduous forest*2; {TEMPERATE_OAKS}*2; eur:Birch and aspen*1; eur:Western Mediterranean*1; aus:Temperate Eucalypt Forest (Victoria)*1; ^eur:Alpine forest (short)*2")
put([792], "eur:Balkan coniferous forest*2; eur:Western Mediterranean*2; eur:Middle East montane conifer forest*1")
put([797], "afr:Maghreb Mediterranean*2; eur:Western Mediterranean*2; eur:Eastern Mediterranean*1; afr:Canary Island Date Palms*1")
put([803], "eur:Western Mediterranean*3; eur:Eastern Mediterranean*1; afr:Canary Island Date Palms*1")
put([799], "eur:Western Mediterranean*4; eur:Oak forest*1; eur:Eastern Mediterranean*1; afr:Canary Island Date Palms*1; ~eur:Swamp and riparian forest*2")
put([788], "eur:Western Mediterranean*2; eur:Balkan coniferous forest*2; eur:Beech-yew Alpine foothills*1")
put([795], "eur:Western Mediterranean*4; eur:Eastern Mediterranean*2; eur:Oak forest*1; afr:Canary Island Date Palms*1; ~eur:Swamp and riparian forest*2")
put([802], "eur:Beech-yew Alpine foothills*2; eur:Western Mediterranean*2; eur:Balkan coniferous forest*1; eur:Misc. Deciduous forest*1")
put([806], "eur:Western Mediterranean*3; eur:Eastern Mediterranean*2; afr:Canary Island Date Palms*1")
put([794], f"eur:Misc. Deciduous forest*2; {OPEN_WOODS}*2; eur:Eastern Mediterranean*1; ^eur:Balkan coniferous forest*2")
put([801], "eur:Balkan coniferous forest*3; eur:Beech-yew Alpine foothills*1; eur:Eastern Mediterranean*1; eur:Misc. Deciduous forest*1")
put([785], "eur:Eastern Mediterranean*5; eur:Middle East montane conifer forest*1; afr:Canary Island Date Palms*1; ^eur:Middle East montane conifer forest*2")
put([789], "eur:Eastern Mediterranean*4; afr:Canary Island Date Palms*1; ^eur:Middle East montane conifer forest*2")
put([786], f"eur:Middle East montane conifer forest*3; eur:Balkan coniferous forest*1; {OPEN_WOODS}*1")
put([790], "eur:Eastern Mediterranean*3; eur:Middle East montane conifer forest*2; afr:Canary Island Date Palms*1")
put([804], "eur:Middle East montane conifer forest*4; eur:Eastern Mediterranean*1")
put([791], "eur:Eastern Mediterranean*5; eur:Middle East montane conifer forest*1; afr:Canary Island Date Palms*1; ^eur:Middle East montane conifer forest*2")

# Palearctic: North Africa, Arabia, Iran and Central Asia
SAHARA = "afr:Sahelian Scrub and Semidesert!Adansonia*3; afr:Canary Island Date Palms*1"
ARABIA = "ind:Thar Desert Scrub*3; afr:Sahelian Scrub and Semidesert!Adansonia*2; afr:Canary Island Date Palms*2"
GOBI = "asn:Gobi Desert Shrubs!Quercus"
put([839, 842, 845, 822, 846, 844, 823], SAHARA)
put([833], "afr:Maghreb Mediterranean*2; afr:Sahelian Scrub and Semidesert!Adansonia*1; afr:Canary Island Date Palms*1; eur:Western Mediterranean*1")
put([836], "afr:Sahelian Scrub and Semidesert!Adansonia*3; ind:Thar Desert Scrub*1; afr:Canary Island Date Palms*1")
put([837, 832, 831, 809, 811, 810, 840, 821], ARABIA)
put([722], "ind:Thar Desert Scrub*3; afr:Sahelian Scrub and Semidesert!Adansonia*2; afr:Canary Island Date Palms*1")
put([723], "asn:Balochistan Xeric woodland*2; afr:Sahelian Scrub and Semidesert!Adansonia*1")
put([745], "afr:Sahelian Scrub and Semidesert!Adansonia*1; afr:Canary Island Date Palms*1")
put([744], "afr:Canary Island Date Palms*3; afr:Sahelian Scrub and Semidesert!Adansonia*2; aus:Riverside trees*1; ~asn:Mesopotamian river and wetland forest*2")
put([747], "asn:Mesopotamian river and wetland forest*3; afr:Canary Island Date Palms*2")
put([830], "asn:Mesopotamian river and wetland forest*2; afr:Canary Island Date Palms*2; ind:Thar Desert Scrub*1")
put([739], "asn:Zagros Mountain forest-steppe*2; asn:Balochistan Xeric woodland*1; eur:Eastern Mediterranean*1; ~asn:Mesopotamian river and wetland forest*2")
put([688], "asn:Zagros Mountain forest-steppe*5; asn:Balochistan Xeric woodland*1")
put([695], "asn:Zagros Mountain forest-steppe*3; asn:Balochistan Xeric woodland*1; eur:Middle East montane conifer forest*1; ~asn:Central-Asian Riverside Tugay Forest*2")
put([727], "asn:Zagros Mountain forest-steppe*2; asn:Birch Forest*1; ~asn:Central-Asian Riverside Tugay Forest*2")
put([756, 757], "asn:Zagros Mountain forest-steppe*2; asn:Balochistan Xeric woodland*2")
put([812], "eur:Eastern Mediterranean*2; asn:Central-Asian Riverside Tugay Forest*1; asn:Zagros Mountain forest-steppe*1")
put([829], f"asn:Central-Asian Riverside Tugay Forest*2; asn:Zagros Mountain forest-steppe*1; {GOBI}*1")
put([820], "asn:Zagros Mountain forest-steppe*3; asn:Balochistan Xeric woodland*1; asn:Central-Asian Riverside Tugay Forest*1; eur:Middle East montane conifer forest*1")
put([841], "ind:Thar Desert Scrub*2; afr:Canary Island Date Palms*2; afr:Sahelian Scrub and Semidesert!Adansonia*1")
put([815], f"asn:Central-Asian Riverside Tugay Forest*2; {GOBI}*2")
put([819], f"{GOBI}*2; asn:Central-Asian Riverside Tugay Forest*1; ~asn:Central-Asian Riverside Tugay Forest*3")
put([816, 807, 834, 813], "asn:Balochistan Xeric woodland*3; asn:Zagros Mountain forest-steppe*1; ~asn:Central-Asian Riverside Tugay Forest*2")
put([838], "ind:Thar Desert Scrub*2; asn:Balochistan Xeric woodland*1; afr:Canary Island Date Palms*1")
put([814], "asn:Balochistan Xeric woodland*4; asn:Zagros Mountain forest-steppe*1")
put([828, 817], f"{GOBI}*3; asn:Central-Asian Riverside Tugay Forest*1")
put([818], "asn:Central-Asian Riverside Tugay Forest*5")
put([843], f"asn:Central-Asian Riverside Tugay Forest*3; {GOBI}*2")
put([827, 835, 826, 808, 825, 824], f"{GOBI}*3; asn:Central-Asian Riverside Tugay Forest*1")
put([684], f"asn:Central-Asian Riverside Tugay Forest*4; {GOBI}*2")
put([758], "eur:Middle East montane conifer forest*2; asn:Balochistan Xeric woodland*1; afr:Maghreb Mediterranean*1")
put([731], "asn:Birch Forest*4; asn:Siberian Taiga*1; ~asn:Central-Asian Riverside Tugay Forest*2")
put([721], "asn:Zagros Mountain forest-steppe*2; asn:Balochistan Xeric woodland*1; eur:Dark oak or walnut*1; asn:Central-Asian Riverside Tugay Forest*1; ~asn:Central-Asian Riverside Tugay Forest*2")
put([733, 732], f"asn:Birch Forest*2; {GOBI}*1; ~asn:Central-Asian Riverside Tugay Forest*2")
put([730], "asn:Zagros Mountain forest-steppe*2; asn:Balochistan Xeric woodland*2; eur:Dark oak or walnut*1; ~asn:Central-Asian Riverside Tugay Forest*2")
put([740], f"{GOBI}*2; eur:Dark oak or walnut*1; asn:Birch Forest*1; asn:Central-Asian Riverside Tugay Forest*1; ^asn:Siberian Taiga*2")
put([724, 728], f"{GOBI}*2; asn:Birch Forest*1; ~asn:Central-Asian Riverside Tugay Forest*2")
put([752, 766, 753], "asn:Balochistan Xeric woodland*2; asn:High Mountain Krummholz*1")
put([762, 754, 750, 759, 768], "asn:High Mountain Krummholz*3; ~asn:Central-Asian Riverside Tugay Forest*1")
put([770, 765], "asn:High Mountain Krummholz*2; asn:Central-Asian Riverside Tugay Forest*1; ~asn:Central-Asian Riverside Tugay Forest*2")
put([760, 769, 751], "ind:Himalayan Coniferous Forest*3; asn:High Mountain Krummholz*1")
put([767], "asn:Siberian Taiga*2; asn:High Mountain Krummholz*1; asn:Birch Forest*1")
put([749, 764, 755], "asn:Siberian Taiga*2; asn:Birch Forest*1")
put([763], "asn:High Mountain Krummholz*2; asn:Siberian Taiga*1")
put([761], f"{GOBI}*3; asn:Yellow River Plain*1")

# Palearctic: Siberia and East Asia
put([687], "asn:Birch Forest*4; asn:Siberian Taiga*2")
put([680], "asn:Yangtze River Plain*3; ind:Jiangnan Subtropical Laurel Forest*2; ^asn:Central Chinese Mountain Forest*2")
put([677], "asn:Central Chinese Mountain Forest*3; asn:Yellow River Plain*2; ^asn:Central Chinese Mountain Forest*2")
put([659], "asn:Central Chinese Mountain Forest*2; asn:Yangtze River Plain*2; ind:Jiangnan Subtropical Laurel Forest*1")
put([653], f"asn:Yellow River Plain*3; {GOBI}*1; ^asn:Central Chinese Mountain Forest*1")
put([657], "asn:Yangtze River Plain*4; ind:Jiangnan Subtropical Laurel Forest*2; asn:Weeping Willows*1; ~asn:Weeping Willows*2; ~ind:Chinese Water Cypress*1")
put([667], "asn:Yellow River Plain*5; asn:Weeping Willows*1; ~asn:Weeping Willows*2; ^asn:Central Chinese Mountain Forest*1")
put([673], "asn:Manchu-Ussur-Korean Mixed Forest*2; asn:Yellow River Plain*2; asn:Birch Forest*1")
put([669], "asn:Manchu-Ussur-Korean Mixed Forest*4; asn:Birch Forest*1; ^asn:Far Eastern Taiga*2")
put([681], "asn:Southern Japanese Forest*3; asn:Korean Deciduous Forest*2")
put([655], "asn:Korean Deciduous Forest*5; asn:Cherry Grove*1; ^asn:Manchu-Ussur-Korean Mixed Forest*1")
put([656], "asn:Manchu-Ussur-Korean Mixed Forest*4; ^asn:Far Eastern Taiga*2")
put([685], "asn:Manchu-Ussur-Korean Mixed Forest*4; asn:Far Eastern Taiga*1; asn:Birch Forest*1")
put([683], "asn:Honshu Broadleaf Forest*4; asn:Japanese Temperate Rainforest*1; ^asn:Japanese Temperate Rainforest*2")
put([670], "asn:Southern Japanese Forest*3; asn:Japanese Temperate Rainforest*2")
put([682], "asn:Southern Japanese Forest*4; asn:Japanese Temperate Rainforest*2; asn:Cherry Grove*1; ^asn:Honshu Broadleaf Forest*2")
put([671], "asn:Honshu Broadleaf Forest*4; asn:Japanese Temperate Rainforest*1; ^asn:Hokkaido Mixed Forest*1")
put([666], "asn:Hokkaido Mixed Forest*5; asn:Birch Forest*1; ^asn:Far Eastern Taiga*1")
put([643], "ind:Yunnanese Lowland Subtropical Forest*4; ind:South China Montane Laurel Forest*2; ^ind:Southeast Asian Subtropical Mountain Forest*2; ^ind:Himalayan Broadleaf and Pine Forest*1")
put([642], "ind:Jiangnan Subtropical Laurel Forest*3; ind:South China Montane Laurel Forest*2; ind:Yunnanese Lowland Subtropical Forest*1")
put([694], "ind:Himalayan Coniferous Forest*3; ind:Himalayan Broadleaf and Pine Forest*2")
put([709], "asn:Siberian Taiga*3; asn:Birch Forest*1; ~asn:Central-Asian Riverside Tugay Forest*2; ^asn:Siberian Taiga*2; ^asn:High Mountain Krummholz*1")
put([690], "asn:Siberian Taiga*3; asn:Birch Forest*2; ^asn:Siberian Taiga*2")
put([702], "ind:Himalayan Coniferous Forest*4; ind:Himalayan Broadleaf and Pine Forest*1")
put([707], "asn:Siberian Taiga*4; asn:Birch Forest*1")
put([700], "asn:Siberian Taiga*3; asn:Birch Forest*1")
put([704], "ind:Himalayan Coniferous Forest*3; ind:Southeast Asian Subtropical Mountain Forest*2; ind:Yunnanese Lowland Subtropical Forest*1")
put([697], "ind:Himalayan Coniferous Forest*3; ind:South China Montane Laurel Forest*1; ^ind:Himalayan Coniferous Forest*2")
put([706], "asn:Central Chinese Mountain Forest*4; ^asn:High Mountain Krummholz*1")
put([705], "asn:Siberian Taiga*2; asn:High Mountain Krummholz*2; asn:Birch Forest*1")
put([696], f"asn:High Mountain Krummholz*2; asn:Siberian Taiga*1; {GOBI}*1")
put([693], "asn:Far Eastern Taiga*4; asn:Birch Forest*2")
put([699], "asn:Hokkaido Mixed Forest*2; asn:Honshu Broadleaf Forest*2; asn:Japanese Temperate Rainforest*1; ^asn:Bonsai-esque weathered trees*3")
put([698], "asn:Hokkaido Mixed Forest*3; asn:Far Eastern Taiga*2; ^asn:Bonsai-esque weathered trees*1")
put([720], "asn:Siberian Taiga*4; asn:Birch Forest*2")
put([718], "asn:Siberian Taiga*3; asn:Far Eastern Taiga*1; asn:Birch Forest*1")
put([710, 714], "asn:Far Eastern Taiga*4; asn:Siberian Taiga*1; asn:Birch Forest*1")
put([715], "asn:Far Eastern Taiga*4; asn:Birch Forest*1")
put([716], "asn:Far Eastern Taiga*3; asn:Hokkaido Mixed Forest*1")
put([713], "asn:Birch Forest*3; asn:Far Eastern Taiga*1; ^asn:Bonsai-esque weathered trees*1")
put([712], "asn:Birch Forest*3; asn:Far Eastern Taiga*2")
put([738], "asn:Birch Forest*3; asn:Siberian Taiga*2")
put([736], f"asn:Siberian Taiga*2; asn:Birch Forest*1; {GOBI}*1")
put([737, 726], f"asn:Siberian Taiga*2; asn:Birch Forest*2; {GOBI}*1")
put([734], f"{GOBI}*2; asn:Birch Forest*1; asn:Yellow River Plain*1; ~asn:Weeping Willows*2")
put([742, 748], "asn:Yellow River Plain*2; asn:Weeping Willows*1; asn:Central-Asian Riverside Tugay Forest*1; ~asn:Weeping Willows*2")
put([743, 746, 741], "asn:Manchu-Ussur-Korean Mixed Forest*2; asn:Birch Forest*2; ~asn:Weeping Willows*2")
put([784, 781, 782, 777, 771, 775, 773, 779, 772, 778, 783], "asn:Birch Forest*2; asn:Far Eastern Taiga*1; ^asn:Bonsai-esque weathered trees*1")

# Nearctic
EAST_RIVER = "~ena:River & Lowland Hardwoods*2"
BOREAL_W = "wna:canadian taiga*4; wna:birch-aspen*2"
DESERT_NO_SAGUARO = "wna:desert!Carnegiea"
put([324], "wna:baja california*2; wna:desert*1; ena:Caribbean Tropical Dry Forest*2")
put([326, 327], "wna:california broadleaf forest*2; wna:pinyon pine*2; wna:giant sequoia*2")
put([325], "ena:Tropical Beach*2; ena:Pine-Hemlock Coniferous Forest!Abies!Pinus!Tsuga*1; ena:Southern Pine Flatwoods*1")
put([332], f"ena:Midwestern Hardwoods*3; ena:Southern Pine Flatwoods*1; {EAST_RIVER}")
put([341, 340], f"ena:Midwestern Hardwoods*3; ena:Appalachian Hardwoods*2; ena:Southern Pine Flatwoods*1; {EAST_RIVER}")
put([344], "ena:Canadian & New England Hardwoods*2; ena:Pine-Hemlock Coniferous Forest*2; wna:birch-aspen*1; wna:canadian taiga*1; ~wna:canadian taiga*2")
put([337], "ena:River & Lowland Hardwoods*3; ena:Southern Coastal Plain Hardwoods*2; ~ena:Swamp*3")
put([343], f"ena:Midwestern Hardwoods*3; ena:Canadian & New England Hardwoods*1; wna:birch-aspen*1; {EAST_RIVER}")
put([336], f"ena:Midwestern Hardwoods*3; ena:Appalachian Hardwoods*2; ena:Southern Pine Flatwoods*1; {EAST_RIVER}")
put([342], f"ena:Midwestern Hardwoods*3; ena:Canadian & New England Hardwoods*2; {EAST_RIVER}")
put([329, 331], f"ena:Appalachian Hardwoods*5; ena:Pine-Hemlock Coniferous Forest*1; {EAST_RIVER}; ^ena:Pine-Hemlock Coniferous Forest*2")
put([330], f"ena:Appalachian Hardwoods*3; ena:Southern Pine Flatwoods*2; ena:Southern Coastal Plain Hardwoods*1; {EAST_RIVER}")
put([334], f"ena:Canadian & New England Hardwoods*3; ena:Pine-Hemlock Coniferous Forest*1; ena:Midwestern Hardwoods*1; {EAST_RIVER}")
put([333], "ena:Canadian & New England Hardwoods*2; ena:Pine-Hemlock Coniferous Forest*2; wna:birch-aspen*1; wna:canadian taiga*1")
put([328], f"ena:Appalachian Hardwoods*2; ena:Canadian & New England Hardwoods*2; ena:Pine-Hemlock Coniferous Forest*1; ^ena:Pine-Hemlock Coniferous Forest*2")
put([339], f"ena:Appalachian Hardwoods*3; ena:Canadian & New England Hardwoods*2; ena:Pine-Hemlock Coniferous Forest*1; {EAST_RIVER}")
put([338], "ena:Canadian & New England Hardwoods*3; ena:Pine-Hemlock Coniferous Forest*3; wna:birch-aspen*1; ^ena:Pine-Hemlock Coniferous Forest*2")
put([335], "ena:Canadian & New England Hardwoods*2; ena:Pine-Hemlock Coniferous Forest*2; wna:birch-aspen*1")
put([360, 365, 349, 351, 364], "wna:temperate rainforest*5; ~wna:temperate rainforest*1")
put([359], "wna:temperate rainforest*3; wna:california broadleaf forest*1; aus:Temperate Eucalypt Forest (Victoria)*1")
put([358], "wna:temperate rainforest*3; ^wna:temperate rainforest*1; ^wna:canadian taiga*1")
put([355, 350, 345, 361, 367], "wna:canadian taiga*2; wna:birch-aspen*2; wna:temperate rainforest*1; ^wna:canadian taiga*2")
put([357, 352], "wna:temperate rainforest*3; wna:giant sequoia*1; wna:california broadleaf forest*1")
put([354], "wna:giant sequoia*3; wna:temperate rainforest*1; wna:pinyon pine*1")
put([362], "wna:giant sequoia*2; wna:temperate rainforest*1; wna:birch-aspen*1")
put([366], "wna:giant sequoia*4; wna:california broadleaf forest*1; ^wna:giant sequoia*2")
put([348], "wna:giant sequoia*2; wna:temperate rainforest*1; wna:birch-aspen*1")
put([356], "wna:pinyon pine*3; wna:bristlecone pine*1; wna:birch-aspen*1; ^wna:bristlecone pine*2")
put([346], "wna:giant sequoia*3; wna:pinyon pine*2; wna:birch-aspen*1")
put([368], "wna:birch-aspen*2; wna:pinyon pine*1; wna:canadian taiga*1; wna:giant sequoia*1; ^wna:canadian taiga*2")
put([353], "wna:birch-aspen*2; wna:canadian taiga*2; wna:giant sequoia*1; ^wna:canadian taiga*2; ^wna:bristlecone pine*1")
put([363], "ena:Southern Pine Flatwoods*4; ena:Southern Coastal Plain Hardwoods*1; ~ena:Swamp*2")
put([347], "ena:Southern Pine Flatwoods*2; ena:Pine-Hemlock Coniferous Forest*1; ena:Appalachian Hardwoods*1")
put([375, 372, 383, 380, 381, 378, 376, 379, 377, 382], BOREAL_W)
put([369, 371], "wna:canadian taiga*2; wna:birch-aspen*2; wna:temperate rainforest*1")
put([370, 374, 373], "wna:canadian taiga*3; ena:Pine-Hemlock Coniferous Forest*2; wna:birch-aspen*2")
put([384], "ena:Southern Coastal Plain Hardwoods*3; ena:River & Lowland Hardwoods*1; ena:Tropical Beach*1")
put([403], "wna:temperate rainforest*3; wna:california broadleaf forest*2")
put([385], "wna:california broadleaf forest*3; wna:Fremont's cottonwood*1; ~wna:Fremont's cottonwood*2")
put([398], "wna:birch-aspen*1; wna:giant sequoia*1; wna:temperate rainforest*1")
put([394], "wna:birch-aspen*2; wna:Fremont's cottonwood*1; ~wna:Fremont's cottonwood*2")
put([396, 402, 395, 389], "wna:Fremont's cottonwood*2; ena:Midwestern Hardwoods*2; wna:birch-aspen*1; ~wna:Fremont's cottonwood*2")
put([386], "wna:birch-aspen*4; wna:canadian taiga*1; ena:Canadian & New England Hardwoods*1")
put([391], "ena:Southern Coastal Plain Hardwoods*2; ena:Southern Pine Flatwoods*1; ena:Midwestern Hardwoods*1")
put([397], f"ena:Midwestern Hardwoods*2; wna:birch-aspen*2; {EAST_RIVER}")
put([390], f"ena:Midwestern Hardwoods*3; ena:Southern Pine Flatwoods*1; {EAST_RIVER}")
put([401], f"ena:Midwestern Hardwoods*2; ena:Southern Coastal Plain Hardwoods*1; ena:Southern Pine Flatwoods*1; {EAST_RIVER}")
put([392, 388], "ena:Midwestern Hardwoods*4; ~ena:River & Lowland Hardwoods*3")
put([387], f"ena:Midwestern Hardwoods*4; {EAST_RIVER}")
put([399], "ena:Southern Pine Flatwoods*4; ena:Southern Coastal Plain Hardwoods*2; ~ena:Swamp*2")
put([400], "fl:Broadleaf Forest*3; fl:Pine & Conifer*1; ena:Southern Pine Flatwoods*1; ena:Southern Coastal Plain Hardwoods*1; ~fl:Wetland & Coastal*2; ~ena:Swamp*1")
put([393], "ena:Southern Pine Flatwoods*3; ena:Southern Coastal Plain Hardwoods*2; ~ena:Swamp*2")
put([408, 410, 404, 409, 407, 411, 405, 420, 416, 419, 414, 413, 412, 415, 421, 417, 418], "wna:canadian taiga*1; wna:birch-aspen*1")
put([406], "wna:birch-aspen*1")
put([423], "wna:california broadleaf forest*4; wna:giant sequoia*1; ~wna:Fremont's cottonwood*2")
put([425], "wna:california broadleaf forest*3; wna:temperate rainforest*1; wna:giant sequoia*1")
put([424], "wna:california broadleaf forest*3; wna:giant sequoia*1; wna:pinyon pine*1")
put([422], "wna:california broadleaf forest*3; wna:mexican fan palms*2; aus:Temperate Eucalypt Forest (Victoria)*1; ~wna:Fremont's cottonwood*2")
put([434, 430, 438], "wna:pinyon pine*2; wna:Fremont's cottonwood*1; wna:birch-aspen*1; ~wna:Fremont's cottonwood*2")
put([433], "wna:joshua trees*3; wna:mexican fan palms*1; ~wna:Fremont's cottonwood*2")
put([426, 431], "wna:baja california*5; wna:mexican fan palms*1")
put([435], "wna:desert*5; wna:mexican fan palms*1; ~wna:Fremont's cottonwood*2")
put([429], "wna:pinyon pine*4; wna:Fremont's cottonwood*1; ~wna:Fremont's cottonwood*2")
put([428], f"wna:joshua trees*2; {DESERT_NO_SAGUARO}*1; wna:pinyon pine*1; ~wna:Fremont's cottonwood*2")
put([432], f"wna:joshua trees*2; wna:pinyon pine*2; {DESERT_NO_SAGUARO}*1")
put([427], "wna:california broadleaf forest*2; wna:pinyon pine*1; wna:joshua trees*1; aus:Riverside trees*1; afr:Canary Island Date Palms*1")
put([436, 437], f"{DESERT_NO_SAGUARO}*2; ena:Southern Coastal Plain Hardwoods*1; wna:joshua trees*1; ~wna:Fremont's cottonwood*2")

# Neotropic: Mexico, Central America and the Caribbean
CA_RAIN = "ena:Central American Rainforest*5"
CA_MONTANE = "ena:Central American Rainforest*2; wna:california broadleaf forest*1; wna:giant sequoia*1; sam:Northern Andean Mountain Forest*1; ^wna:giant sequoia*2"
ANTILLES = "ena:Central American Rainforest*3; ena:Caribbean Tropical Dry Forest*1; ena:Tropical Beach*1"
CARIB_DRY = "ena:Caribbean Tropical Dry Forest*5; ena:Tropical Beach*1"
MEX_DRY = "ena:Caribbean Tropical Dry Forest*4; ena:Central American Rainforest*1; wna:baja california*1"
PINE_OAK = "wna:california broadleaf forest*2; wna:giant sequoia*2; ena:Central American Rainforest*1"
put([514, 501, 502, 494, 519, 455, 458, 450, 470, 471], CA_RAIN)
put([489], "ena:Central American Rainforest*2; ~ena:Caribbean Mangroves*3")
put([449], "ena:Central American Rainforest*3; ena:Tropical Beach*1")
put([515, 487, 453, 452, 451, 506, 461], CA_MONTANE)
put([472, 459, 468, 495, 475, 517, 510], ANTILLES)
put([533, 544, 545, 534, 522, 547], MEX_DRY)
put([521, 550, 528, 527, 551, 541], "ena:Caribbean Tropical Dry Forest*4; ena:Central American Rainforest*1")
put([530, 535, 532, 543, 537, 548], CARIB_DRY)
put([556], "wna:giant sequoia*2; wna:california broadleaf forest*2")
put([559], "wna:california broadleaf forest*2; wna:giant sequoia*2; wna:pinyon pine*1; aus:Riverside trees*1")
put([558, 557, 553], PINE_OAK)
put([554, 552, 555], "ena:Florida Pine Rockland*3; ena:Southern Pine Flatwoods*1; ena:Caribbean Tropical Dry Forest*1; ena:Tropical Beach*1")
put([564, 573], "ena:Florida Pine Rockland!Serenoa*2; ena:Southern Pine Flatwoods*1; ena:Central American Rainforest*1")
put([579, 580], "ena:Caribbean Tropical Dry Forest*2; ena:Tropical Beach*1; ~ena:Caribbean Mangroves*3")
put([581], "fl:Broadleaf Forest*3; fl:Wetland & Coastal*2; fl:Pine & Conifer*1; ~fl:Wetland & Coastal*3; ~ena:Caribbean Mangroves*1")
put([607], "wna:baja california*4; wna:mexican fan palms*1")
put([610], "wna:baja california!Foqueria*2; wna:joshua trees*1; ena:Caribbean Tropical Dry Forest*1")
put([605], "ena:Caribbean Tropical Dry Forest*2; wna:baja california*1")
put([600], "ena:Caribbean Tropical Dry Forest*3; ena:Tropical Beach*1")
put([614, 617, 613, 612], "ena:Tropical Beach*2; ena:Caribbean Tropical Dry Forest*2; ~ena:Caribbean Mangroves*4")

# Neotropic: South America
AMAZON = "sam:Amazonian Dry-ground Forest*5; sam:Palm Trees*1; ~sam:Amazonian River & Swamp*3"
VARZEA = "sam:Amazonian River & Swamp*4; sam:Amazonian Dry-ground Forest*2; sam:Palm Trees*1; ~sam:Amazonian River & Swamp*3"
GUIANA = "sam:Guyanan Rainforest*5; sam:Palm Trees*1; ~sam:Amazonian River & Swamp*2"
ANDES = "sam:Northern Andean Mountain Forest*4; sam:Colombia-Panamanian Lowland Rainforest*1; sam:Quindio Wax Palms*1; ^sam:Northern Andean Mountain Forest*2"
ATLANTIC = "sam:Mata Atlantica*5; sam:Palm Trees*1; ^sam:Parana Araucaria Forest*2"
THORN = "sam:Southern Caribbean Thorn Scrub*4; sam:Caatinga!Attalea!Acrocomia*1"
put([516, 454, 478, 447], "sam:Colombia-Panamanian Lowland Rainforest*5; sam:Palm Trees*1; ~sam:Amazonian River & Swamp*2")
put([460, 486, 448], ANDES)
put([477], "sam:Northern Andean Mountain Forest*4; sam:Quindio Wax Palms*1; aus:Temperate Eucalypt Forest (Victoria)*1; ^sam:Northern Andean Mountain Forest*2")
put([499, 457, 513], "sam:Northern Andean Mountain Forest*4; sam:Colombia-Panamanian Lowland Rainforest*1; ^sam:Northern Andean Mountain Forest*2")
put([456], "sam:Northern Andean Mountain Forest*2; sam:Colombia-Panamanian Lowland Rainforest*2; sam:Palm Trees*1")
put([493, 444], "sam:Northern Andean Mountain Forest*3; sam:Amazonian Dry-ground Forest*1; ^sam:Northern Andean Mountain Forest*2")
put([504], "sam:Northern Andean Mountain Forest*2; sam:Gran Chaco*1; ^sam:Northern Andean Mountain Forest*2")
put([490, 464], "sam:Guyanan Rainforest*3; sam:Northern Andean Mountain Forest*1")
put([512, 483, 505, 503, 484, 473, 474, 497, 476, 511, 507, 518, 508, 446, 498], AMAZON)
put([469, 496, 482, 467, 480], VARZEA)
put([466, 465], GUIANA)
put([463, 488], "sam:Amazonian River & Swamp*3; sam:Guyanan Rainforest*2; sam:Palm Trees*1; ~sam:Amazonian River & Swamp*3")
put([481], "sam:Amazonian Dry-ground Forest*2; sam:Cerrado*2")
put([439, 443, 442, 492, 491, 445], ATLANTIC)
put([500], "sam:Mata Atlantica*4; sam:Parana Araucaria Forest*1; sam:Palm Trees*1; ~sam:Restinga*2; ^sam:Parana Araucaria Forest*2")
put([440], "sam:Parana Araucaria Forest*5; sam:Mata Atlantica*2")
put([485, 441], "sam:Restinga*5; sam:Mata Atlantica*2; aus:Coconut Palms*1")
put([462, 509], "sam:Restinga*2; aus:Coconut Palms*1")
put([479], "sam:Caatinga!Attalea*2; sam:Southern Caribbean Thorn Scrub*1")
put([531, 549], "sam:Southern Caribbean Thorn Scrub*2; sam:Gran Chaco*1; ena:Caribbean Tropical Dry Forest*1")
put([542, 526, 538, 546, 539, 520, 536], "sam:Colombia-Venezuelan Llanos*2; sam:Colombia-Panamanian Lowland Rainforest*2; sam:Southern Caribbean Thorn Scrub*1")
put([523], "sam:Gran Chaco*2; sam:Pampas*1; aus:Temperate Eucalypt Forest (Victoria)*1")
put([529], "sam:Gran Chaco*2; sam:Cerrado*2; sam:Humid Chaco*1")
put([540], "sam:Palm Trees*3; sam:Cerrado*1; sam:Amazonian Dry-ground Forest*1")
put([524], "sam:Caatinga*2; sam:Mata Atlantica*2; sam:Cerrado*1")
put([525], "sam:Caatinga*5")
put([562, 560], "sam:Valdivian Laurel-leaf Forest*2")
put([563], "sam:Valdivian Laurel-leaf Forest*5; sam:Chilean Araucaria-Nothofagus Forest*1; sam:Chilean Cypresses*1; aus:Temperate Eucalypt Forest (Victoria)*1; ^sam:Chilean Araucaria-Nothofagus Forest*2")
put([561], "sam:Tierra del Fuego*4; sam:Tierra del Fuego Krummholz*1; ^sam:Tierra del Fuego Krummholz*2")
put([568, 604, 609], "sam:Southern Caribbean Thorn Scrub*1")
put([572], "sam:Colombia-Venezuelan Llanos*5; sam:Palm Trees*1; ~sam:Amazonian River & Swamp*2")
put([565], "sam:Cerrado*2; sam:Palm Trees*2; ~sam:Amazonian River & Swamp*2")
put([569], "sam:Gran Chaco*5")
put([571], "sam:Humid Chaco*5; sam:Palm Trees*1")
put([570], "sam:Cerrado*2; sam:Palm Trees*1; sam:Guyanan Rainforest*1")
put([574], "sam:Pampas*4; aus:Temperate Eucalypt Forest (Victoria)*1")
put([567], "sam:Cerrado*5; sam:Palm Trees*1; ~sam:Palm Trees*2")
put([566], "sam:Cerrado*3; sam:Mata Atlantica*1")
put([578], "sam:Tierra del Fuego Krummholz*1; eur:Swamp and riparian forest*2")
put([577], "sam:Gran Chaco!Schinopsis*1; eur:Swamp and riparian forest*1; sam:Pampas*1")
put([575], "sam:Gran Chaco*2; sam:Pampas*2; sam:Humid Chaco*1")
put([576], "sam:Pampas*5; aus:Temperate Eucalypt Forest (Victoria)*1")
put([582], "sam:Colombia-Panamanian Lowland Rainforest*1; sam:Palm Trees*1; sam:Southern Caribbean Thorn Scrub*1; ~sam:Mangroves*3")
put([583], "sam:Amazonian River & Swamp*2; sam:Palm Trees*2; sam:Colombia-Venezuelan Llanos*1")
put([585], "sam:Pantanal*2; sam:Humid Chaco*2; sam:Pampas*1")
put([586], "sam:Humid Chaco*2; sam:Pampas*2; sam:Palm Trees*1")
put([584], "sam:Pantanal*5; sam:Palm Trees*1; sam:Cerrado*1")
put([593, 590, 594, 591], "sam:Northern Andean Mountain Forest*2; sam:Quindio Wax Palms*1; aus:Temperate Eucalypt Forest (Victoria)*1")
put([588, 589, 587], "sam:Northern Andean Mountain Forest*1; aus:Temperate Eucalypt Forest (Victoria)*1")
put([595], "sam:Chilean Araucaria-Nothofagus Forest*1; sam:Tierra del Fuego Krummholz*1")
put([592], "sam:Gran Chaco!Schinopsis*1; sam:Chilean Matorral*1")
put([596], "sam:Chilean Matorral*4; aus:Temperate Eucalypt Forest (Victoria)*1; sam:Pampas*1")
put([601], "sam:Caatinga!Attalea!Acrocomia*1; sam:Southern Caribbean Thorn Scrub*1; ena:Caribbean Tropical Dry Forest*1")
put([608], "sam:Southern Caribbean Thorn Scrub*1; sam:Chilean Matorral*1; sam:Pampas*1; afr:Canary Island Date Palms*1")
put([602, 606, 603, 597], THORN)
put([599], "sam:Southern Caribbean Thorn Scrub*2; ena:Caribbean Tropical Dry Forest*2; ena:Tropical Beach*1")
put([598], "sam:Chilean Matorral*1; afr:Canary Island Date Palms*1")
put([615, 611, 616], "sam:Restinga*2; sam:Palm Trees*1; ~sam:Mangroves*4")

# Indomalayan
MALABAR = "ind:Malabar Coast Moist Tropical Forest*5; ind:Betel Nut Palms*1; aus:Coconut Palms*1; ~ind:Southeast Asian Riparian & Wetland Flora*2"
DECCAN = "ind:Deccan Plateau Tropical Dry Forest*4; ind:North-Central Indian Tropical Dry Forest*1; ind:Borassus Palms*1"
SE_MONTANE = "ind:Southeast Asia Rainforest*2; ind:Irrawaddy Valley Tropical Deciduous Forest*2; ind:Southeast Asia Tropical Dry Forest*1; ^ind:Southeast Asian Subtropical Mountain Forest*2"
SUNDA = "ind:Sumatra-Java-Lesser Sunda Islands Forests*4; ind:Banana Trees*1; aus:Coconut Palms*1; ind:Betel Nut Palms*1"
PHILIPPINES = "ind:Philippines and Maluku Islands Forest*5; aus:Coconut Palms*1; ind:Banana Trees*1"
SE_MANGROVES = "~ind:Southeast Asian Mangroves*4"
put([253, 242], MALABAR + "; ind:Deccan Plateau Tropical Dry Forest*1")
put([254, 271, 270], MALABAR)
put([287], "ind:Ganges River area rainforest*4; ind:North-Central Indian Tropical Dry Forest*2; ~ind:Southeast Asian Riparian & Wetland Flora*2")
put([274], "ind:Malabar Coast Moist Tropical Forest*3; aus:Coconut Palms*1; ind:Betel Nut Palms*1")
put([275], "ind:Malabar Coast Moist Tropical Forest*3")
put([228, 261], "ind:North-Central Indian Tropical Dry Forest*2; ind:Ganges River area rainforest*2; ind:Deccan Plateau Tropical Dry Forest*1")
put([233], "ind:Himalayan Broadleaf and Pine Forest*3; ind:Ganges River area rainforest*1; ^ind:Himalayan Coniferous Forest*2")
put([282, 238], "ind:Ganges River area rainforest*4; ind:Southeast Asian Riparian & Wetland Flora*2; ind:Borassus Palms*1; aus:Coconut Palms*1; ind:Banana Trees*1")
put([244, 249, 226, 259, 222], "ind:Southeast Asia Rainforest*3; ind:Ganges River area rainforest*2; ^ind:Southeast Asian Subtropical Mountain Forest*2; ^ind:Himalayan Broadleaf and Pine Forest*1")
put([218, 252], "ind:Southeast Asia Rainforest*3; aus:Pacific Island Beach Forest*1; aus:Coconut Palms*1")
put([250, 235, 234], "ind:Irrawaddy Valley Tropical Deciduous Forest*4; ind:Southeast Asia Rainforest*1; ~ind:Southeast Asian Riparian & Wetland Flora*2")
put([237, 239, 256, 258], SE_MONTANE)
put([279, 288, 229], "ind:Sumatra-Java-Lesser Sunda Islands Forests*3; ind:Southeast Asian Subtropical Mountain Forest*2")
put([263], "ind:Malay Peninsula Rainforest*2; ind:Southeast Asian Subtropical Mountain Forest*2")
put([245, 278, 277, 280], "ind:Sumatra-Java-Lesser Sunda Islands Forests*4; ind:Malay Peninsula Rainforest*1; ~ind:Southeast Asian Riparian & Wetland Flora*2")
put([284], "ind:Southeast Asia Rainforest*3; ind:Malay Peninsula Rainforest*2; aus:Coconut Palms*1")
put([224, 225], "ind:Southeast Asian Riparian & Wetland Flora*3; ind:Southeast Asia Rainforest*2; ind:Southeast Asia Tropical Dry Forest*1; ind:Banana Trees*1; aus:Coconut Palms*1; ind:Borassus Palms*1")
put([264, 265], "ind:Malay Peninsula Rainforest*5; aus:Coconut Palms*1; ~ind:Southeast Asian Riparian & Wetland Flora*2")
put([257], "ind:Southeast Asia Tropical Dry Forest*3; ind:Southeast Asia Rainforest*1; ind:Borassus Palms*1")
put([223], "ind:Southeast Asia Rainforest*4")
put([285, 286], "ind:Southeast Asian Riparian & Wetland Flora*3; ind:Southeast Asia Tropical Dry Forest*1; ind:Borassus Palms*2; aus:Coconut Palms*1")
put([255, 260], "ind:South China and Vietnam Subtropical Rainforest*4; ind:Southeast Asia Rainforest*1")
put([266], "ind:South China and Vietnam Subtropical Rainforest*3; ind:Southeast Asian Riparian & Wetland Flora*2; ind:Banana Trees*1")
put([272], "ind:Southeast Asian Subtropical Mountain Forest*2; ind:Himalayan Broadleaf and Pine Forest*2; ind:Southeast Asia Rainforest*1")
put([227], "aus:Pacific Island Beach Forest*2; aus:Coconut Palms*2; ind:Southeast Asia Rainforest*1")
put([268], "ind:South China and Vietnam Subtropical Rainforest*5; ind:Banana Trees*1; ~ind:Chinese Water Cypress*2; ^ind:South China Montane Laurel Forest*2")
put([289, 230], SUNDA)
put([232], "ind:South China and Vietnam Subtropical Rainforest*3; aus:Coconut Palms*1")
put([219, 281, 273, 221], "ind:Borneo Rainforest*5; ~ind:Southeast Asian Riparian & Wetland Flora*2")
put([220], "ind:Borneo Rainforest*3; ^ind:Southeast Asian Subtropical Mountain Forest*2")
put([236], "ind:Jiangnan Subtropical Laurel Forest*5; ind:South China and Vietnam Subtropical Rainforest*1; ~ind:Chinese Water Cypress*2; ^ind:South China Montane Laurel Forest*2")
put([267], "aus:Pacific Island Beach Forest*2; aus:Coconut Palms*1")
put([262, 241, 248, 276, 231, 247], PHILIPPINES)
put([240, 246], "ind:Philippines and Maluku Islands Forest*3; ^ind:Southeast Asian Subtropical Mountain Forest*1; ^ind:Himalayan Broadleaf and Pine Forest*1")
put([243], "aus:Coconut Palms*3; aus:Pacific Island Beach Forest*2")
put([269, 283], "ind:South China and Vietnam Subtropical Rainforest*2; ind:Jiangnan Subtropical Laurel Forest*2; asn:Southern Japanese Forest*1; ^ind:South China Montane Laurel Forest*2")
put([251], "asn:Southern Japanese Forest*3; ind:South China and Vietnam Subtropical Rainforest*1; aus:Pacific Island Beach Forest*1")
put([296, 290, 298, 297, 292], DECCAN)
put([295], "ind:Deccan Plateau Tropical Dry Forest*2; ind:Deccan Plateau Thornscrub*2; ind:North-Central Indian Tropical Dry Forest*1")
put([293, 301], "ind:Sri Lanka-Tamil Nadu Tropical Dry Forest*4; ind:Borassus Palms*1; aus:Coconut Palms*1")
put([294], "ind:Southeast Asia Tropical Dry Forest*3; ind:Irrawaddy Valley Tropical Deciduous Forest*1; ind:Borassus Palms*1")
put([291, 300], "ind:Southeast Asia Tropical Dry Forest*4; ind:Borassus Palms*1")
put([299], "ind:Southeast Asia Tropical Dry Forest*4; ind:Southeast Asia Rainforest*1; ind:Borassus Palms*1")
put([302], "ind:Himalayan Broadleaf and Pine Forest*5; ^ind:Himalayan Coniferous Forest*1")
put([304], "ind:Himalayan Broadleaf and Pine Forest*3; ind:Southeast Asian Subtropical Mountain Forest*1")
put([305], "ind:Himalayan Broadleaf and Pine Forest*2; ind:Sumatra-Java-Lesser Sunda Islands Forests*2")
put([303], "ind:Himalayan Broadleaf and Pine Forest*3; ind:Philippines and Maluku Islands Forest*1")
put([308], "ind:Himalayan Broadleaf and Pine Forest*2; ind:Southeast Asian Subtropical Mountain Forest*1; ind:Himalayan Coniferous Forest*1; ~asn:Weeping Willows*2; ^ind:Himalayan Coniferous Forest*2")
put([306, 307], "ind:Southeast Asian Subtropical Mountain Forest*2; ind:Himalayan Broadleaf and Pine Forest*2; ind:Himalayan Coniferous Forest*1; ^ind:Himalayan Coniferous Forest*2")
put([310, 309], "ind:Himalayan Coniferous Forest*5")
put([311], "ind:Ganges River area rainforest*3; ind:Southeast Asian Riparian & Wetland Flora*1")
put([312], "ind:Thar Desert Scrub*3; ind:Deccan Plateau Thornscrub*1")
put([313], "ind:Southeast Asian Subtropical Mountain Forest*2; ind:Borneo Rainforest*1")
put([314], "ind:Deccan Plateau Thornscrub*3; ind:Thar Desert Scrub*1; ind:Deccan Plateau Tropical Dry Forest*1")
put([318, 317], "ind:Thar Desert Scrub*3; ind:Deccan Plateau Thornscrub*1; ind:Deccan Plateau Tropical Dry Forest*1")
put([315], "ind:Deccan Plateau Thornscrub*4; ind:Deccan Plateau Tropical Dry Forest*1; ind:Borassus Palms*1")
put([316], "ind:Deccan Plateau Tropical Dry Forest*2; ind:Borassus Palms*1; aus:Coconut Palms*1; " + SE_MANGROVES)
put([320], "ind:Thar Desert Scrub*2; afr:Canary Island Date Palms*1; aus:Coconut Palms*1; ~ind:Southeast Asian Mangroves!Nypa*4")
put([323], "ind:Ganges River area rainforest*2; ind:Southeast Asian Riparian & Wetland Flora*2; " + SE_MANGROVES)
put([321], "ind:Irrawaddy Valley Tropical Deciduous Forest*2; ind:Southeast Asian Riparian & Wetland Flora*1; " + SE_MANGROVES)
put([319], "ind:Southeast Asian Riparian & Wetland Flora*2; ind:Southeast Asia Tropical Dry Forest*1; aus:Coconut Palms*1; " + SE_MANGROVES)
put([322], "ind:Borneo Rainforest*2; aus:Coconut Palms*1; " + SE_MANGROVES)

# Australasia
WALLACEA = "ind:Philippines and Maluku Islands Forest*5; aus:Coconut Palms*1"
NEW_GUINEA = "ind:New Guinea Lowland Rainforest*5; aus:Coconut Palms*1"
NG_MONTANE = "ind:New Guinea Lowland Rainforest*2; aus:Queensland Wet Tropics Rainforest*1; ^aus:Queensland Wet Tropics Rainforest*2; ^ind:Southeast Asian Subtropical Mountain Forest*1"
NT_SAVANNA = "aus:Northern Territory Savanna*5; aus:Riverside trees*1; ~aus:Riverside trees*2"
MALLEE = "aus:Mallee and Scrub*4; aus:Riverside trees*1"
OUTBACK = "aus:Central Desert*3; aus:Mulga Shrubland*2; ~aus:Riverside trees*2"
put([156, 138, 140, 151, 136], WALLACEA)
put([157], "ind:Philippines and Maluku Islands Forest*3; ^ind:Southeast Asian Subtropical Mountain Forest*1")
put([161, 137, 162, 154, 155, 148, 135, 153, 144, 158, 143], NEW_GUINEA)
put([160, 149, 139, 141, 145], NG_MONTANE)
put([152], "ind:New Guinea Lowland Rainforest*3; aus:Pacific Island Beach Forest*1; aus:Coconut Palms*1")
put([150], "aus:Queensland Wet Tropics Rainforest*5; aus:Subtropical Coastal Rainforest*1; aus:Coconut Palms*1")
put([142], "aus:Subtropical Coastal Rainforest*3; aus:Pacific Island Beach Forest*1")
put([146], "aus:Queensland Wet Tropics Rainforest*2; aus:Fiji*1; aus:Pacific Island Beach Forest*1")
put([164], "aus:Pacific Island Beach Forest*2; aus:Fiji*1; aus:Queensland Wet Tropics Rainforest*1")
put([147], "aus:Queensland Wet Tropics Rainforest*2; aus:Subtropical Coastal Rainforest*1; aus:Pacific Island Beach Forest*1")
put([159], "aus:Fiji*3; aus:Pacific Island Beach Forest*1; aus:Coconut Palms*1")
put([165, 163, 166], "ind:Sumatra-Java-Lesser Sunda Islands Forests*2; aus:Northern Territory Savanna!Adansonia*2; ind:Borassus Palms*1")
put([167], "aus:New Zealand South Island*2; aus:New Zealand North Island*1")
put([176], "aus:Temperate Eucalypt Forest (Victoria)*4; aus:Riverside trees*1; ~aus:Riverside trees*2; ^aus:Mountain Ash Temperate Rainforest*2")
put([178], "aus:Tasmanian Lowland Temperate Rainforest*3; aus:Temperate Eucalypt Forest (Victoria)*2; ^aus:Tasmanian Highlands*2")
put([168], "aus:Temperate Eucalypt Forest (New South Wales)*3; aus:Sydney-area Subtropical Forest*2; aus:Subtropical Coastal Rainforest*2; ~aus:Riverside trees*2")
put([179], "aus:Tasmanian Lowland Temperate Rainforest*3; aus:Tasmanian Highlands*2; ^aus:Tasmanian Highlands*2")
put([177], "aus:Tasmanian Highlands*4; aus:Tasmanian Lowland Temperate Rainforest*1")
put([169, 174, 172, 180, 170, 175], "aus:New Zealand South Island*5")
put([173], "aus:New Zealand Kauri Forest*3; aus:New Zealand North Island*3")
put([171], "aus:New Zealand North Island*5; aus:New Zealand Kauri Forest*1")
put([186, 189, 181, 184, 183, 185], NT_SAVANNA)
put([188], "aus:Northern Territory Savanna!Adansonia*2; ind:New Guinea Lowland Rainforest*1")
put([187], "aus:Riverside trees*2; aus:Mulga Shrubland*1; aus:Queensland Brigalow Savanna*1")
put([182], "aus:Queensland Brigalow Savanna*3; aus:Temperate Eucalypt Forest (New South Wales)*1; ~aus:Riverside trees*2")
put([191], "aus:Mulga Shrubland*4; aus:Riverside trees*1")
put([192], "aus:Temperate Eucalypt Forest (New South Wales)*2; aus:Riverside trees*2; aus:Mallee and Scrub*1; ~aus:Riverside trees*2")
put([190], "aus:New Zealand South Island*2; eur:Misc. Deciduous forest*1; eur:Swamp and riparian forest*1")
put([195], "ind:New Guinea Lowland Rainforest*1; aus:Queensland Wet Tropics Rainforest*1")
put([193], "aus:Mountain Ash Temperate Rainforest*2; aus:Tasmanian Highlands*1")
put([194, 196], "aus:New Zealand South Island*2")
put([202], "aus:Jarrah-Karri Temperate Rainforest*5; aus:Swan Coast Scrublands*1")
put([206], "aus:Swan Coast Scrublands*4; aus:Jarrah-Karri Temperate Rainforest*2; ~aus:Riverside trees*2")
put([205], "aus:Swan Coast Scrublands*2; aus:Mallee and Scrub*1; aus:Mulga Shrubland*1")
put([198, 199, 201], MALLEE)
put([203], "aus:Mallee and Scrub*3; aus:Riverside trees*2; ~aus:Riverside trees*3")
put([197], "aus:Mallee and Scrub*2; aus:Mulga Shrubland*1; aus:Central Desert*1")
put([200], "aus:Temperate Eucalypt Forest (Victoria)*3; aus:Riverside trees*2; aus:Mallee and Scrub*1; ~aus:Riverside trees*2")
put([204], "aus:Temperate Eucalypt Forest (Victoria)*3; aus:Riverside trees*1")
put([207, 213, 209, 210, 211, 208, 215, 214], OUTBACK)
put([216], "aus:Mulga Shrubland*4; aus:Central Desert*1")
put([212], "aus:Mallee and Scrub*2; aus:Mulga Shrubland*1")
put([217], "ind:New Guinea Lowland Rainforest*2; aus:Coconut Palms*1; ~aus:Grey Mangroves*2; ~ind:Southeast Asian Mangroves*2")

# Oceania
POLYNESIA = "aus:Fiji*3; aus:Pacific Island Beach Forest*2; aus:Coconut Palms*2"
ATOLL = "aus:Coconut Palms*3; aus:Pacific Island Beach Forest*2"
put([624], "aus:New Zealand North Island*2; aus:Pacific Island Beach Forest*1")
put([629, 634, 631, 620, 633, 630, 625, 627, 618, 622, 638, 637, 635], POLYNESIA)
put([632, 621, 619], ATOLL)
put([623], "aus:Hawaiian Rainforest*4; aus:Coconut Palms*1; aus:Pacific Island Beach Forest*1")
put([636], "aus:Hawaiian Rainforest*2; aus:Coconut Palms*2; aus:Pacific Island Beach Forest*2")
put([641, 640], "aus:Pacific Island Beach Forest*2; aus:Hawaiian Rainforest*1; aus:Coconut Palms*1")
put([639], "aus:Hawaiian Rainforest*2")
put([628], "aus:Pacific Island Beach Forest*1; aus:Coconut Palms*1")
put([626], "asn:Southern Japanese Forest*2; aus:Pacific Island Beach Forest*1")


def community_names():
    names = {}
    for pack in sorted(os.listdir(PACKS)):
        path = os.path.join(PACKS, pack, "region.json")
        if not os.path.isfile(path):
            continue
        with open(path, encoding="utf-8") as f:
            m = json.load(f)
        for c in m["communities"]:
            full = c["name"]
            short = re.sub(r"^[A-Z+]+\s*-\s*", "", full)
            species = [s["name"] for s in c["species"]]
            names[(pack, short)] = (full, species)
            names[(pack, full)] = (full, species)
    return names


def resolve_mix(eco_id, mix, names):
    out, weights = [], {}
    for raw in mix.split(";"):
        entry = raw.strip()
        kind = ""
        if entry[0] in "^~":
            kind, entry = entry[0], entry[1:]
        body, weight = entry.rsplit("*", 1)
        pack, rest = body.split(":", 1)
        community, *excluded = rest.split("!")
        key = (pack, community.strip())
        if key not in names:
            sys.exit(f"ecoregion {eco_id}: unknown community {pack}:{community}")
        full, species = names[key]
        kept = [s for s in species if not any(s == x or s.split("_")[0] == x for x in excluded)]
        if excluded and (len(kept) == len(species) or not kept):
            sys.exit(f"ecoregion {eco_id}: exclusion {excluded} does not fit {full}")
        w = int(weight)
        assert 1 <= w <= 9, (eco_id, entry)
        if not kind:
            weights[pack] = weights.get(pack, 0) + w
        out.append(f"{kind}{pack}:{full}{''.join('!' + x for x in excluded)}*{w}")
    primary = max(weights, key=lambda p: (weights[p], -list(weights).index(p)))
    return primary, ";".join(out)


def write_grid(path, grid, cell_deg):
    rows, cols = grid.shape
    tile = round(TILE_DEG / cell_deg)
    assert rows % tile == 0 and cols % tile == 0
    cx = zstd.ZstdCompressor(level=19)
    frames, offsets, pos = [], [0], 0
    for r in range(0, rows, tile):
        for c in range(0, cols, tile):
            block = np.ascontiguousarray(grid[r:r + tile, c:c + tile])
            frame = cx.compress(block.astype("<u%d" % grid.itemsize).tobytes()) if block.any() else b""
            frames.append(frame)
            pos += len(frame)
            offsets.append(pos)
    header = b"AGRD" + struct.pack("<BBHIId", 1, grid.itemsize, tile, cols, rows, cell_deg)
    with open(path, "wb") as f:
        f.write(header)
        f.write(struct.pack("<%dI" % len(offsets), *offsets))
        for frame in frames:
            f.write(frame)
    print(f"{os.path.basename(path)}: {cols}x{rows}, {len(frames)} tiles, {os.path.getsize(path) / 1e6:.2f} MB")


def build_koppen(src):
    if src.lower().endswith(".tif"):
        import rasterio

        with rasterio.open(src) as ds:
            grid = ds.read(1).astype(np.uint8)
    else:
        grid = np.fromfile(src, dtype=np.uint8).reshape(1800, 3600)
    assert grid.shape == (1800, 3600), grid.shape
    write_grid(os.path.join(HERE, "koppen.grid"), grid, 0.1)


def build_ecoregions(src):
    import geopandas as gpd
    from rasterio.features import rasterize
    from rasterio.transform import from_origin

    if not os.path.exists(src):
        print(f"downloading {ECO_URL}")
        urllib.request.urlretrieve(ECO_URL, src)
    gdf = gpd.read_file(f"zip://{src}!Ecoregions2017.shp", engine="pyogrio")
    gdf = gdf[gdf.ECO_ID > 0]
    res = 1 / 60
    grid = rasterize(
        zip(gdf.geometry, gdf.ECO_ID.astype(int)),
        out_shape=(10800, 21600),
        transform=from_origin(-180, 90, res, res),
        fill=0,
        dtype="uint16",
    )
    # Coastal cells fall between polygon and shoreline; three passes reach about 5 km out.
    for _ in range(3):
        empty = grid == 0
        for dr, dc in ((0, 1), (0, -1), (1, 0), (-1, 0)):
            shifted = np.roll(grid, (dr, dc), axis=(0, 1))
            take = empty & (grid == 0) & (shifted != 0)
            grid[take] = shifted[take]
    write_grid(os.path.join(HERE, "ecoregions.grid"), grid, res)

    names = community_names()
    missing = []
    lines = []
    for row in gdf.sort_values("ECO_ID").itertuples():
        realm = REALMS.get(row.REALM, "")
        mix, pack = "", ""
        if row.ECO_ID in MIX:
            pack, mix = resolve_mix(row.ECO_ID, MIX[row.ECO_ID], names)
        elif realm != "AN":
            missing.append(f"{row.ECO_ID} {row.ECO_NAME}")
        lines.append(f"{row.ECO_ID}\t{int(row.BIOME_NUM)}\t{realm}\t{pack}\t{mix}\t{row.ECO_NAME}")
    if missing:
        sys.exit("ecoregions without a tree mix:\n  " + "\n  ".join(missing))
    stale = set(MIX) - set(int(i) for i in gdf.ECO_ID)
    if stale:
        sys.exit(f"tree mixes for unknown ecoregions: {sorted(stale)}")
    with open(os.path.join(HERE, "ecoregions.tsv"), "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines) + "\n")
    print(f"ecoregions.tsv: {len(lines)} ecoregions")


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--ecoregions", default=os.path.join(tempfile.gettempdir(), "Ecoregions2017.zip"))
    ap.add_argument("--koppen", default=os.path.join(HERE, "koppen_0p1.bin"))
    args = ap.parse_args()
    if os.path.exists(args.koppen):
        build_koppen(args.koppen)
    else:
        print(f"skipping koppen.grid, {args.koppen} not found")
    build_ecoregions(args.ecoregions)
