#!/usr/bin/env python

# Copyright 2022 by louis-e, https://github.com/louis-e/.
# MIT License
# Please see the LICENSE file that should have been included as part of this package.

from math import floor
from random import randint
from tqdm import tqdm
import anvil
import argparse
import gc
import numpy as np
import os
import sys
import time

from .getData import getData
from .processData import processData

parser = argparse.ArgumentParser(
    description="Arnis - Generate cities from real life in Minecraft"
)
parser.add_argument("--bbox", dest="bbox", help="Bounding box of the area")
parser.add_argument("--city", dest="city", help="Name of the city (Experimental)")
parser.add_argument("--state", dest="state", help="Name of the state (Experimental)")
parser.add_argument(
    "--country", dest="country", help="Name of the country (Experimental)"
)
parser.add_argument("--file", dest="file", help="JSON file containing OSM data")
parser.add_argument(
    "--path", dest="path", required=True, help="Path to the minecraft world"
)
parser.add_argument(
    "--downloader",
    dest="downloader",
    choices=["requests", "curl", "wget"],
    default="requests",
    help="Downloader method (requests/curl/wget)",
)
parser.add_argument(
    "--debug",
    dest="debug",
    default=False,
    action="store_true",
    help="Enable debug mode",
)
args = parser.parse_args()

# Ensure either bbox or city/state/country is provided
if not args.bbox and not (args.city and args.state and args.country):
    print(
        """Error! You must provide either a bounding box (bbox) or city/state/country \
(experimental) information."""
    )
    os._exit(1)

# Ensure file argument is handled correctly
if args.file and args.bbox:
    print("Error! You cannot provide both a bounding box (bbox) and a file.")
    os._exit(1)

gc.collect()
np.seterr(all="raise")
np.set_printoptions(threshold=sys.maxsize)

processStartTime = time.time()
air = anvil.Block("minecraft", "air")
glass = anvil.Block("minecraft", "glass_pane")
brick = anvil.Block("minecraft", "bricks")
stone = anvil.Block("minecraft", "stone")
grass_block = anvil.Block("minecraft", "grass_block")
spruce_log = anvil.Block("minecraft", "spruce_log")
birch_leaves = anvil.Block("minecraft", "birch_leaves")
dirt = anvil.Block("minecraft", "dirt")
sand = anvil.Block("minecraft", "sand")
podzol = anvil.Block.from_numeric_id(3, 2)
grass = anvil.Block.from_numeric_id(175, 2)
farmland = anvil.Block("minecraft", "farmland")
water = anvil.Block("minecraft", "water")
wheat = anvil.Block("minecraft", "wheat", {"age": 7})
carrots = anvil.Block("minecraft", "carrots", {"age": 7})
potatoes = anvil.Block("minecraft", "potatoes", {"age": 7})
cobblestone = anvil.Block("minecraft", "cobblestone")
iron_block = anvil.Block("minecraft", "iron_block")
oak_log = anvil.Block.from_numeric_id(17)
oak_leaves = anvil.Block("minecraft", "oak_leaves")
birch_log = anvil.Block("minecraft", "birch_log")
white_stained_glass = anvil.Block("minecraft", "white_stained_glass")
dark_oak_door_lower = anvil.Block("minecraft", "dark_oak_door", {"half": "lower"})
dark_oak_door_upper = anvil.Block("minecraft", "dark_oak_door", {"half": "upper"})
cobblestone_wall = anvil.Block("minecraft", "cobblestone_wall")
stone_brick_slab = anvil.Block.from_numeric_id(44, 5)
rail = anvil.Block("minecraft", "rail")

red_flower = anvil.Block.from_numeric_id(38)
yellow_flower = anvil.Block("minecraft", "dandelion")
blue_flower = anvil.Block("minecraft", "blue_orchid")
white_flower = anvil.Block("minecraft", "azure_bluet")

white_concrete = anvil.Block("minecraft", "white_concrete")
black_concrete = anvil.Block("minecraft", "black_concrete")
gray_concrete = anvil.Block("minecraft", "gray_concrete")
light_gray_concrete = anvil.Block("minecraft", "light_gray_concrete")
green_stained_hardened_clay = anvil.Block.from_numeric_id(159, 5)
dirt = anvil.Block("minecraft", "dirt")
glowstone = anvil.Block("minecraft", "glowstone")
sponge = anvil.Block("minecraft", "sponge")
hay_bale = anvil.Block("minecraft", "hay_block")

regions = {}
for x in range(0, 3):
    for z in range(0, 3):
        regions["r." + str(x) + "." + str(z)] = anvil.EmptyRegion(0, 0)


def setBlock(block, x, y, z):
    flooredX = floor(x / 512)
    flooredZ = floor(z / 512)
    identifier = "r." + str(flooredX) + "." + str(flooredZ)
    if identifier not in regions:
        regions[identifier] = anvil.EmptyRegion(0, 0)
    regions[identifier].set_block(block, x - flooredX * 512, y, z - flooredZ * 512)


def fillBlocks(block, x1, y1, z1, x2, y2, z2):
    for x in range(x1, x2 + 1):
        for y in range(y1, y2 + 1):
            for z in range(z1, z2 + 1):
                if not (x == x2 + 1 or y == y2 + 1 or z == z2 + 1):
                    setBlock(block, x, y, z)


mcWorldPath = args.path
if mcWorldPath[-1] == "/":
    mcWorldPath = mcWorldPath[:-1]


def saveRegion(region="all"):
    if region == "all":
        region_keys = list(regions.keys())
        for key in tqdm(
            region_keys,
            desc="Saving minecraft world",
            unit="region",
            bar_format="{l_bar}{bar}| {n_fmt}/{total_fmt}",
        ):
            regions[key].save(mcWorldPath + "/region/" + key + ".mca")
    else:
        regions[region].save(mcWorldPath + "/region/" + region + ".mca")
        print(f"Saved {region}")


from .tree import createTree  # noqa: E402


def run():
    print(
        """\n
        ▄████████    ▄████████ ███▄▄▄▄    ▄█     ▄████████
        ███    ███   ███    ███ ███▀▀▀██▄ ███    ███    ███
        ███    ███   ███    ███ ███   ███ ███▌   ███    █▀
        ███    ███  ▄███▄▄▄▄██▀ ███   ███ ███▌   ███
      ▀███████████ ▀▀███▀▀▀▀▀   ███   ███ ███▌ ▀███████████
        ███    ███ ▀███████████ ███   ███ ███           ███
        ███    ███   ███    ███ ███   ███ ███     ▄█    ███
        ███    █▀    ███    ███  ▀█   █▀  █▀    ▄████████▀
                     ███    ███

                https://github.com/louis-e/arnis
          """
    )

    if not (os.path.exists(mcWorldPath + "/region")):
        print("Error! No Minecraft world found at given path")
        os._exit(1)

    rawdata = getData(
        args.city,
        args.state,
        args.country,
        args.bbox,
        args.file,
        args.debug,
        args.downloader,
    )
    imgarray = processData(rawdata, args)

    # Generate Minecraft world
    x = 0
    z = 0
    doorIncrement = 0
    ElementIncr = 0
    ElementsLen = len(imgarray)
    for i in tqdm(
        imgarray,
        desc="Generating pixels",
        unit=" pixels",
        total=ElementsLen,
        bar_format="{l_bar}{bar}| {n_fmt}/{total_fmt} [{elapsed}<{remaining}, {rate_fmt}]",
    ):
        z = 0
        for j in i:
            setBlock(dirt, x, 0, z)
            if j == 0:  # Ground
                setBlock(grass_block, x, 1, z)
            elif j == 10:  # Street
                setBlock(black_concrete, x, 1, z)
                setBlock(air, x, 2, z)
            elif j == 11:  # Footway
                setBlock(gray_concrete, x, 1, z)
                setBlock(air, x, 2, z)
            elif j == 12:  # Natural path
                setBlock(cobblestone, x, 1, z)
            elif j == 13:  # Bridge
                setBlock(light_gray_concrete, x, 2, z)
                setBlock(light_gray_concrete, x - 1, 2, z - 1)
                setBlock(light_gray_concrete, x + 1, 2, z - 1)
                setBlock(light_gray_concrete, x + 1, 2, z + 1)
                setBlock(light_gray_concrete, x - 1, 2, z + 1)
            elif j == 14:  # Railway
                setBlock(iron_block, x, 1, z)
                setBlock(rail, x, 2, z)
            elif j == 20:  # Parking
                setBlock(gray_concrete, x, 1, z)
            elif j == 21:  # Fountain border
                setBlock(light_gray_concrete, x, 2, z)
                setBlock(white_concrete, x, 1, z)
            elif j >= 22 and j <= 24:  # Fence
                if str(j)[-1] == "2" or int(str(j[0])[-1]) == 2:
                    setBlock(cobblestone_wall, x, 2, z)
                else:
                    fillBlocks(cobblestone, x, 2, z, x, int(str(j[0])[-1]), z)

                setBlock(grass_block, x, 1, z)
            elif j == 30:  # Meadow
                setBlock(grass_block, x, 1, z)
                randomChoice = randint(0, 2)
                if randomChoice == 0 or randomChoice == 1:
                    setBlock(grass, x, 2, z)
            elif j == 31:  # Farmland
                if x % 15 == 0 or z % 15 == 0:  # Place water every 8 blocks
                    setBlock(water, x, 1, z)
                else:
                    setBlock(farmland, x, 1, z)
                    if (
                        randint(0, 75) == 0
                    ):  # Rarely place trees, hay bales, or leaf blocks
                        special_choice = randint(1, 10)
                        if special_choice <= 1:  # 20% chance
                            createTree(x, z, randint(1, 3))
                        elif special_choice <= 6:  # 40% chance
                            setBlock(hay_bale, x, 2, z)
                            if randint(0, 2) == 0:
                                setBlock(hay_bale, x, 3, z)
                                setBlock(hay_bale, x - 1, 2, z)
                                setBlock(hay_bale, x, 2, z - 1)
                            else:
                                setBlock(hay_bale, x, 3, z)
                                setBlock(hay_bale, x + 1, 2, z)
                                setBlock(hay_bale, x, 2, z + 1)
                        else:  # Remaining 40% chance
                            setBlock(oak_leaves, x, 2, z)
                    else:
                        crop_choice = randint(0, 2)
                        crops = [wheat, carrots, potatoes]
                        setBlock(crops[crop_choice], x, 2, z)
            elif j == 32:  # Forest
                setBlock(grass_block, x, 1, z)
                randomChoice = randint(0, 20)
                randomTree = randint(1, 3)
                randomFlower = randint(1, 4)
                if randomChoice == 20:
                    createTree(x, z, randomTree)
                elif randomChoice == 2:
                    if randomFlower == 1:
                        setBlock(red_flower, x, 2, z)
                    elif randomFlower == 2:
                        setBlock(blue_flower, x, 2, z)
                    elif randomFlower == 3:
                        setBlock(yellow_flower, x, 2, z)
                    else:
                        setBlock(white_flower, x, 2, z)
                elif randomChoice == 0 or randomChoice == 1:
                    setBlock(grass, x, 2, z)
            elif j == 33:  # Cemetery
                # Spawn chances
                grave_chance = 25
                flower_chance = 15
                tree_chance = 5

                setBlock(podzol, x, 1, z)
                if (x % 3 == 0) and (z % 3 == 0):
                    randomChoice = randint(0, 100)
                    if randomChoice < grave_chance:
                        if randint(0, 1) == 0:
                            setBlock(cobblestone, x - 1, 2, z)
                            setBlock(stone_brick_slab, x - 1, 3, z)
                            setBlock(stone_brick_slab, x, 2, z)
                            setBlock(stone_brick_slab, x + 1, 2, z)
                        else:
                            setBlock(cobblestone, x, 2, z - 1)
                            setBlock(stone_brick_slab, x, 3, z - 1)
                            setBlock(stone_brick_slab, x, 2, z)
                            setBlock(stone_brick_slab, x, 2, z + 1)
                    elif randomChoice < grave_chance + flower_chance:
                        setBlock(red_flower, x, 2, z)
                    elif randomChoice < grave_chance + flower_chance + tree_chance:
                        createTree(x, z, randint(1, 3))
            elif j == 34:  # Beach
                setBlock(sand, x, 1, z)
            elif j == 35:  # Wetland
                randomChoice = randint(0, 2)
                if randomChoice == 0:
                    setBlock(grass_block, x, 1, z)
                else:
                    setBlock(water, x, 1, z)
            elif j == 36:  # Pitch
                setBlock(green_stained_hardened_clay, x, 1, z)
            elif j == 37:  # Swimming pool
                setBlock(water, x, 1, z)
                setBlock(white_concrete, x, 0, z)
            elif j == 38:  # Water
                setBlock(water, x, 1, z)
            elif j == 39:  # Raw grass
                setBlock(grass_block, x, 1, z)
            elif j >= 50 and j <= 59:  # House corner
                building_height = 5
                if j == 51:
                    building_height = 8
                elif j == 52:
                    building_height = 11
                elif j == 53:
                    building_height = 14
                elif j == 54:
                    building_height = 17
                elif j == 55:
                    building_height = 20
                elif j == 56:
                    building_height = 23
                elif j == 57:
                    building_height = 26
                elif j == 58:
                    building_height = 29
                elif j == 59:
                    building_height = 32

                fillBlocks(white_concrete, x, 1, z, x, building_height, z)
            elif j >= 60 and j <= 69:  # House wall
                building_height = 4
                if j == 61:
                    building_height = 7
                elif j == 62:
                    building_height = 10
                elif j == 63:
                    building_height = 13
                elif j == 64:
                    building_height = 16
                elif j == 65:
                    building_height = 19
                elif j == 66:
                    building_height = 22
                elif j == 67:
                    building_height = 25
                elif j == 68:
                    building_height = 28
                elif j == 69:
                    building_height = 31

                if doorIncrement == 25:
                    fillBlocks(white_stained_glass, x, 4, z, x, building_height, z)
                    setBlock(white_concrete, x, 1, z)
                    setBlock(dark_oak_door_lower, x, 2, z)
                    setBlock(dark_oak_door_upper, x, 3, z)
                    doorIncrement = 0
                else:
                    fillBlocks(white_concrete, x, 1, z, x, 2, z)
                    fillBlocks(white_stained_glass, x, 3, z, x, building_height, z)
                doorIncrement += 1
                setBlock(white_concrete, x, building_height + 1, z)
            elif j >= 70 and j <= 79:  # House interior
                if j >= 70:
                    setBlock(white_concrete, x, 5, z)
                    if j >= 71:
                        setBlock(white_concrete, x, 8, z)
                        if j >= 72:
                            setBlock(white_concrete, x, 11, z)
                            if j >= 73:
                                setBlock(white_concrete, x, 14, z)
                                if j >= 74:
                                    setBlock(white_concrete, x, 17, z)
                                    if j >= 75:
                                        setBlock(white_concrete, x, 20, z)
                                        if j >= 76:
                                            setBlock(white_concrete, x, 23, z)
                                            if j >= 77:
                                                setBlock(white_concrete, x, 26, z)
                                                if j >= 78:
                                                    setBlock(white_concrete, x, 29, z)
                                                    if j >= 78:
                                                        setBlock(
                                                            white_concrete, x, 32, z
                                                        )

                setBlock(white_concrete, x, 1, z)

            z += 1
        x += 1
        ElementIncr += 1

    print(
        f"Generation finished in {(time.time() - processStartTime):.2f} "
        + f"seconds ({((time.time() - processStartTime) / 60):.2f} minutes)"
    )
    # Saving minecraft world
    saveRegion()
    print(
        f"Done! Finished in {(time.time() - processStartTime):.2f} "
        + f"seconds ({((time.time() - processStartTime) / 60):.2f} minutes)"
    )
    os._exit(0)
